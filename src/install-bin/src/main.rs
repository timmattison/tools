//! install-bin — install a locally built binary onto a fresh inode without
//! tripping macOS's per-vnode code-signature cache, then exec it once to prove
//! the kernel will actually run it.

use std::path::PathBuf;
use std::process;

use buildinfo::version_string;
use clap::Parser;
use install_bin::{install_binary, InstallResult};

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

/// The default destination directory, `~/.local/bin`, matching the TS tool.
/// Exits with a diagnostic if the home directory can't be determined.
fn default_dest_dir() -> PathBuf {
    match dirs::home_dir() {
        Some(home) => home.join(".local").join("bin"),
        None => {
            eprintln!("install-bin: could not determine your home directory; pass --dest <dir>");
            process::exit(1);
        }
    }
}

/// The installed file name: the explicit `name` argument, else the source's own
/// file name. UTF-8-safe via `to_string_lossy`.
fn installed_name(args: &Args) -> String {
    if let Some(name) = &args.name {
        return name.clone();
    }
    args.source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn main() {
    let args = Args::parse();

    let dest_dir = args.dest.clone().unwrap_or_else(default_dest_dir);
    let dest = dest_dir.join(installed_name(&args));

    let InstallResult {
        replaced_existing, ..
    } = match install_binary(&args.source, &dest) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("install-bin: {err}");
            process::exit(1);
        }
    };

    let note = if replaced_existing {
        " (replaced existing, fresh inode)"
    } else {
        ""
    };
    println!(
        "installed {} → {}{note}",
        args.source.display(),
        dest.display()
    );
}
