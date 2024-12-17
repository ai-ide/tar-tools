use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tar::{Archive, Builder};
use std::fs::File;

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

fn handle_error(err: std::io::Error) -> ! {
    eprintln!("Error: {}", err);
    std::process::exit(1);
}

fn run() -> std::io::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Create { input, output } => {
            let file = File::create(output)?;
            let mut builder = Builder::new(file);
            if input.is_dir() {
                builder.append_dir_all(".", input)?;
            } else {
                builder.append_path(input)?;
            }
            builder.finish()?;
        }
        Commands::Extract { archive, output } => {
            let file = File::open(archive)?;
            let mut archive = Archive::new(file);
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
