use clap::Parser;
use log::{debug, error, info};
use simple_logger::SimpleLogger;
use std::fs::{canonicalize, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};

#[derive(Parser)]
#[command(version, author, about, dont_collapse_args_in_usage = true)]
struct Args {
    /// Temporary directory to use
    #[arg(long)]
    tempdir: Option<PathBuf>,

    /// Input file path
    input_file: PathBuf,

    /// Output file path
    output_file: PathBuf,
}

fn main() {
    SimpleLogger::new().init().unwrap_or_default();
    let args = Args::parse();

    if args.output_file.exists() {
        error!("{:?} already exists", args.output_file);
        exit(1)
    }

    info!("Create temporary directory");
    let tempdir = args
        .tempdir
        .as_ref()
        .map_or_else(tempfile::tempdir, tempfile::tempdir_in)
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
    let input_file = canonicalize(&args.input_file).unwrap_or_else(|err| {
        error!(
            "Failed to get the canonical path of {:?}: {}",
            args.input_file, err
        );
        exit(err.raw_os_error().unwrap_or(1))
    });

    info!("Detect silences");
    let mut ffmpeg = Command::new("ffmpeg")
        .arg("-i")
        .arg(&input_file)
        .args(["-af", "silencedetect=n=0.03:d=0.1"])
        .args(["-f", "null"])
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
        for (uniq, line) in output.lines().map_while(Result::ok).enumerate() {
            eprintln!("{}", &line);
            if let Some(pos) = line.find("silence_start: ") {
                if let Some(silence_start) = line[pos..].split_whitespace().nth(1) {
                    if let Ok(silence_start) = silence_start.parse::<f32>() {
                        if (silence_start - silence_end).abs() > 0.01 {
                            debug!("keep {}-{}", silence_end, silence_start);
                            let mut piece = tempdir.path().to_owned();
                            piece.push(format!("piece-{uniq:08x}.mkv"));
                            writeln!(concat_script, "file {}", piece.to_string_lossy())
                                .expect("Failed to write");
                            slice(silence_end, silence_start - silence_end, &input_file, piece);
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
    concatenate(concat_script_path, args.output_file);
}

fn slice<I, O>(timestamp: f32, duration: f32, input: I, output: O)
where
    I: AsRef<Path>,
    O: AsRef<Path>,
{
    let status = Command::new("ffmpeg")
        .args(["-ss", &timestamp.to_string()])
        .args(["-t", &duration.to_string()])
        .arg("-i")
        .arg(input.as_ref())
        .arg(output.as_ref())
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

fn concatenate<I, O>(input: I, output: O)
where
    I: AsRef<Path>,
    O: AsRef<Path>,
{
    let status = Command::new("ffmpeg")
        .args(["-f", "concat", "-safe", "0"])
        .arg("-i")
        .arg(input.as_ref())
        .args(["-c", "copy"])
        .arg(output.as_ref())
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
