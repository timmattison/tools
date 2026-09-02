//! `subito` — the binary that subscribes to AWS IoT Core topics.
//!
//! The binary reads the command line that [`subito::cli`] states. The library
//! holds every other part of the tool.

#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]

use clap::Parser;
use subito::cli::Cli;

fn main() {
    let _cli = Cli::parse();
}
