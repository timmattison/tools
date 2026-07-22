//! install-bin — install a locally built binary onto a fresh inode without
//! tripping macOS's per-vnode code-signature cache, then exec it once to prove
//! the kernel will actually run it.

use std::path::PathBuf;

use buildinfo::version_string;
use clap::Parser;

/// Install a locally built binary onto `PATH` without tripping macOS's
/// code-signature cache.
#[derive(Parser)]
#[command(
    name = "install-bin",
    version = version_string!(),
    about = "Install a locally built binary onto a fresh inode without tripping macOS's code-signature cache"
)]
struct Args {
    /// The locally built binary to install.
    source: PathBuf,

    /// Installed name (defaults to the source file name).
    name: Option<String>,

    /// Destination directory (default: ~/.local/bin).
    #[arg(long)]
    dest: Option<PathBuf>,

    /// Argument passed to the post-install exec check.
    #[arg(long, default_value = "--version")]
    verify_arg: String,

    /// Skip the post-install exec check.
    #[arg(long)]
    no_verify: bool,
}

fn main() {
    // The install orchestration is not wired up yet; this cycle only proves the
    // CLI parses and (once the version attribute is added) reports its version.
    let _args = Args::parse();
}
