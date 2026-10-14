//! The `ha` binary.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use clap::Parser;

/// The command line of `ha`.
#[derive(Parser)]
#[command(
    name = "ha",
    version = buildinfo::version_string!(),
    about = "Hot action: a self-refreshing terminal view of GitHub Actions runs across repositories and owners"
)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
