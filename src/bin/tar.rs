use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tar::{Archive, Builder};
use std::fs::File;
use std::io::{self, Read, Write};
use flate2::write::GzEncoder;
use flate2::read::GzDecoder;
use flate2::Compression;

#[derive(Parser)]
#[command(name = "tar")]
#[command(about = "Archive and extract files using tar format")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    #[arg(short = 'v', long = "verbose", help = "Enable verbose output")]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    #[command(short_flag = 'c')]
    Create {
        #[arg(help = "File or directory to archive")]
        input: PathBuf,
        #[arg(short = 'o', help = "Location of archive")]
        output: PathBuf,
        #[arg(short = 'z', help = "Enable gzip compression")]
        gzip: bool,
    },
    #[command(short_flag = 'x')]
    Extract {
        #[arg(help = "Location of archive")]
        archive: PathBuf,
        #[arg(short = 'o', help = "Output directory")]
        output: PathBuf,
    },
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

fn handle_error(err: std::io::Error) -> ! {
    eprintln!("Error: {}", err);
    std::process::exit(1);
}

fn run() -> std::io::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Create { input, output, gzip } => {
            let file = File::create(output)?;
            let writer: Box<dyn Write> = if gzip {
                Box::new(CompressedWriter::new(file))
            } else {
                Box::new(file)
            };
            let mut builder = Builder::new(writer);
            if input.is_dir() {
                builder.append_dir_all(".", input)?;
            } else {
                builder.append_path(input)?;
            }
            builder.finish()?;
        }
        Commands::Extract { archive, output } => {
            let file = File::open(archive)?;
            let reader: Box<dyn Read> = if archive.extension().map_or(false, |ext| ext == "gz") {
                Box::new(GzDecoder::new(file))
            } else {
                Box::new(file)
            };
            let mut archive = Archive::new(reader);
            archive.unpack(output)?;
        }
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        handle_error(e);
    }
}
