use clap::{App, AppSettings, Arg};
use log::{debug, error, info};
use simple_logger::SimpleLogger;
use std::fs::{canonicalize, File};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{exit, Command, Stdio};

fn main() {
    SimpleLogger::new().init().unwrap_or_default();
    let app = App::new("jumpcutter")
        .setting(AppSettings::DontCollapseArgsInUsage)
        .arg(
            Arg::with_name("tempdir")
                .long("--tempdir")
                .takes_value(true),
        )
        .arg(Arg::with_name("input-file").required(true))
        .arg(Arg::with_name("output-file").required(true));
    let matches = app.get_matches();
    let input_file = matches.value_of_os("input-file").unwrap_or_default();
    let output_file = matches.value_of_os("output-file").unwrap_or_default();

    if Path::new(&output_file).exists() {
        error!("{:?} already exists", output_file);
        exit(1)
    }

    info!("Create temporary directory");
    let tempdir = matches
        .value_of_os("tempdir")
        .map(tempfile::tempdir_in)
        .unwrap_or_else(tempfile::tempdir)
        .unwrap_or_else(|err| {
            error!("Failed to create a temporary directory");
            exit(err.raw_os_error().unwrap_or(1))
        });

    info!("Create concat script");
    let mut concat_script_path = tempdir.path().to_owned();
    concat_script_path.push("concat.txt");
    let mut concat_script = File::create(&concat_script_path).unwrap_or_else(|err| {
        error!("Failed to create concat script: {}", err);
        exit(err.raw_os_error().unwrap_or(1))
    });
    writeln!(&concat_script, "ffconcat version 1.0").expect("Failed to write");
    let input_file = canonicalize(input_file).unwrap_or_else(|err| {
        error!(
            "Failed to get the canonical path of {:?}: {}",
            input_file, err
        );
        exit(err.raw_os_error().unwrap_or(1))
    });

    info!("Detect silences");
    let mut ffmpeg = Command::new("ffmpeg")
        .arg("-i")
        .arg(&input_file)
        .args(&["-af", "silencedetect=n=0.03:d=0.1"])
        .args(&["-f", "null"])
        .arg("-")
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| {
            error!("Failed to spawn ffmpeg: {}", err);
            exit(err.raw_os_error().unwrap_or(1))
        });
    if let Some(output) = ffmpeg.stderr.take() {
        let output = BufReader::new(output);
        let mut silence_end = 0.0;
        for (uniq, line) in output.lines().filter_map(Result::ok).enumerate() {
            eprintln!("{}", &line);
            if let Some(pos) = line.find("silence_start: ") {
                if let Some(silence_start) = line[pos..].split_whitespace().nth(1) {
                    if let Ok(silence_start) = silence_start.parse::<f32>() {
                        if (silence_start - silence_end).abs() > 0.01 {
                            debug!("keep {}-{}", silence_end, silence_start);
                            let mut piece = tempdir.path().to_owned();
                            piece.push(format!("piece-{:08x}.mkv", uniq));
                            writeln!(concat_script, "file {}", piece.to_string_lossy())
                                .expect("Failed to write");
                            let status = Command::new("ffmpeg")
                                .args(&["-ss", &silence_end.to_string()])
                                .args(&["-t", &(silence_start - silence_end).to_string()])
                                .arg("-i")
                                .arg(&input_file)
                                .arg(piece)
                                .status()
                                .unwrap_or_else(|err| {
                                    error!("Failed to extract sub-video: {}", err);
                                    exit(err.raw_os_error().unwrap_or(1))
                                });
                            if !status.success() {
                                error!("Failed to extract a piece");
                                exit(status.code().unwrap_or(1))
                            }
                        }
                    }
                }
            } else if let Some(pos) = line.find("silence_end: ") {
                if let Some(end) = line[pos..].split_whitespace().nth(1) {
                    if let Ok(end) = end.parse() {
                        silence_end = end;
                    }
                }
            }
        }
    }

    drop(concat_script); // Flush and close the script

    info!("Concatenate pieces");
    let status = Command::new("ffmpeg")
        .args(&["-f", "concat", "-safe", "0"])
        .arg("-i")
        .arg(&concat_script_path)
        .args(&["-c", "copy"])
        .arg(output_file)
        .status()
        .unwrap_or_else(|err| {
            error!("Failed to execute ffmpeg: {}", err);
            exit(err.raw_os_error().unwrap_or(1))
        });
    if !status.success() {
        error!("Failed to concatenate pieces");
        exit(status.code().unwrap_or(1))
    }
}
