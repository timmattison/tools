//! `krt` (Knights of the Round Trip) records the network path to a
//! destination, hop by hop.
//!
//! This slice builds the crate and the build string. Later slices add the
//! command line flags, the tracer, the file writer, and the table.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use buildinfo::version_string;
use clap::Parser;

/// The command line of `krt`.
///
/// The flags arrive in a later slice. `--version` and `-V` work now, because
/// `clap` reads the build string that `buildinfo` made at compile time.
#[derive(Parser, Debug)]
#[command(
    name = "krt",
    version = version_string!(),
    about = "Knights of the Round Trip: record the network path to a destination"
)]
struct Cli {}

fn main() {
    // The parse handles `--version`, `-V`, and `--help` on its own. A later
    // slice prints the resolved configuration.
    Cli::parse();
}
