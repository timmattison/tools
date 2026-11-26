use buildinfo::version_string;
use clap::Parser;

/// Finds files by name across a whole machine.
#[derive(Parser)]
#[command(name = "ff", version = version_string!())]
struct Cli {
    /// The glob that each file name must match.
    pattern: String,
}

fn main() {
    let _cli = Cli::parse();
}
