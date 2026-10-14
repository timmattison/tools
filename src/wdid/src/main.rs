//! `wdid` — "what do i do": counts which tools of this workspace the atuin
//! history uses.

use clap::Parser;

/// The one-line description that `--help` shows.
const ABOUT: &str = "What do I do: count which tools of this workspace the atuin history uses";

/// The command line of `wdid`.
#[derive(Parser)]
#[command(about = ABOUT)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
