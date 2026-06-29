use buildinfo::version_string;
use clap::Parser;
use std::env;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "portplz")]
#[command(version = version_string!())]
#[command(about = "Generate a port number from the git repo root and branch name", long_about = None)]
struct Cli {
    #[arg(help = "Directory path (defaults to current directory)")]
    path: Option<String>,

    #[arg(
        short,
        long,
        help = "Print verbose output with repo/directory name and branch"
    )]
    verbose: bool,

    #[arg(long, help = "Disable git branch detection")]
    no_git: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let path: PathBuf = match cli.path {
        Some(p) => PathBuf::from(p),
        None => env::current_dir()?,
    };

    let derivation = portplz_core::derive(&path, cli.no_git)?;

    if cli.verbose {
        println!("{}", derivation.source.describe(derivation.port));
    } else {
        println!("{}", derivation.port.get());
    }

    Ok(())
}
