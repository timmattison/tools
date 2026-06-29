use buildinfo::version_string;
use clap::Parser;
use std::env;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "portplz")]
#[command(version = version_string!())]
#[command(about = "Generate a port number from the git repo root, branch, and current user", long_about = None)]
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

fn main() {
    // Render any error via Display (not the default `Termination` Debug form) so
    // the user sees the helpful message — e.g. a malformed `PORTPLZ_UID` reports
    // "PORTPLZ_UID must be a non-negative integer, ..." instead of an opaque
    // `InvalidUidOverride("abc")` — then exit non-zero.
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let path: PathBuf = match cli.path {
        Some(p) => PathBuf::from(p),
        None => env::current_dir()?,
    };

    let user = portplz_core::UserSalt::current()?;
    let derivation = portplz_core::derive(&path, cli.no_git, &user)?;

    if cli.verbose {
        println!("{}", derivation.describe());
    } else {
        println!("{}", derivation.port.get());
    }

    Ok(())
}
