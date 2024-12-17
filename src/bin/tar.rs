use clap::{Parser};
use std::path::PathBuf;
use tar::{Archive, Builder};
use std::fs::File;
use std::io::{self, Read, Write};
use flate2::write::GzEncoder;
use flate2::read::GzDecoder;
use flate2::Compression;
use indicatif::{ProgressBar, ProgressStyle};

#[derive(Parser)]
#[command(name = "tar")]
#[command(about = "Archive and extract files using tar format")]
struct Cli {
    /// Enable verbose output
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,

    /// Create archive
    #[arg(short = 'c', group = "mode")]
    create: bool,

    /// Extract archive
    #[arg(short = 'x', group = "mode")]
    extract: bool,

    /// Enable gzip compression
    #[arg(short = 'z')]
    gzip: bool,

    /// Output location (file for create, directory for extract)
    #[arg(short = 'o')]
    output: PathBuf,

    /// Input (file/directory to archive for create, archive for extract)
    input: PathBuf,
}

struct CompressedWriter<W: Write> {
    inner: GzEncoder<W>,
}

impl<W: Write> CompressedWriter<W> {
    fn new(writer: W) -> Self {
        CompressedWriter {
            inner: GzEncoder::new(writer, Compression::default())
        }
    }
}

impl<W: Write> Write for CompressedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn create_progress_bar(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} [{elapsed_precise}] {msg}: {pos}")
            .unwrap()
    );
    pb.set_message(msg.to_string());
    pb
}

fn handle_error(err: std::io::Error) -> ! {
    eprintln!("Error: {}", err);
    std::process::exit(1);
}

fn run() -> std::io::Result<()> {
    let cli = Cli::parse();

    if cli.create {
        let pb = create_progress_bar("Creating archive");
        let file = File::create(&cli.output)?;
        let writer: Box<dyn Write> = if cli.gzip {
            if cli.verbose {
                println!("Using gzip compression");
            }
            Box::new(CompressedWriter::new(file))
        } else {
            Box::new(file)
        };
        let mut builder = Builder::new(writer);

        if cli.input.is_dir() {
            if cli.verbose {
                println!("Adding directory: {}", cli.input.display());
            }
            // Use the directory name itself as the base path
            let base_name = cli.input.file_name().unwrap_or_default();
            builder.append_dir_all(base_name, &cli.input)?;
        } else {
            if cli.verbose {
                println!("Adding file: {}", cli.input.display());
            }
            builder.append_path(&cli.input)?;
        }
        builder.finish()?;
        pb.finish_with_message("Archive created successfully");
    } else if cli.extract {
        let pb = create_progress_bar("Extracting archive");
        let file = File::open(&cli.input)?;
        let reader: Box<dyn Read> = if cli.input.extension().map_or(false, |ext| ext == "gz") {
            if cli.verbose {
                println!("Detected gzip compression");
            }
            Box::new(GzDecoder::new(file))
        } else {
            Box::new(file)
        };
        let mut archive = Archive::new(reader);
        if cli.verbose {
            println!("Extracting to: {}", cli.output.display());
        }
        archive.unpack(&cli.output)?;
        pb.finish_with_message("Archive extracted successfully");
    }

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        handle_error(e);
    }
}
