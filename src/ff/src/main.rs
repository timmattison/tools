use std::path::PathBuf;

use buildinfo::version_string;
use clap::Parser;

/// Finds files by name across a whole machine. An interrupted system call is
/// done again, thus the walk never ends early.
#[derive(Parser)]
#[command(name = "ff", version = version_string!())]
struct Cli {
    /// The glob that each file name must match, for example '120.mp4' or
    /// '*.MP4'. It ignores case unless -s is given.
    pattern: String,

    /// The directories to search. With none, ff searches each mounted volume
    /// that holds user files (macOS only).
    paths: Vec<PathBuf>,

    /// Match the case of each letter.
    #[arg(short = 's', long)]
    case_sensitive: bool,

    /// Take a regular expression instead of a glob. It matches anywhere in the
    /// name.
    #[arg(short = 'e', long)]
    regex: bool,

    /// Stop at each mount point.
    #[arg(short = 'x', long)]
    one_file_system: bool,

    /// Also search the File Provider directories: each directory in
    /// ~/Library/CloudStorage, and ~/Library/Mobile Documents.
    #[arg(long)]
    include_cloud: bool,

    /// Name each subtree that the search could not read, and each directory
    /// and volume that it skipped.
    #[arg(long)]
    show_skipped: bool,

    /// End each match with a NUL instead of a line end, for xargs -0.
    #[arg(short = '0', long)]
    print0: bool,
}

fn main() {
    let _cli = Cli::parse();
}
