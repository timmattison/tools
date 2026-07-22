//! install-bin — install a locally built binary onto a fresh inode without
//! tripping macOS's per-vnode code-signature cache, then exec it once to prove
//! the kernel will actually run it.

use std::path::{Path, PathBuf};
use std::process;

use buildinfo::version_string;
use clap::Parser;
use install_bin::{
    install_binary, verify_exec, ExecVerdict, InstallResult, DEFAULT_VERIFY_TIMEOUT,
};

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
    #[arg(long, default_value = "--version", allow_hyphen_values = true)]
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

    if args.no_verify {
        return;
    }

    let mut verdict = verify_exec(&dest, &args.verify_arg, DEFAULT_VERIFY_TIMEOUT);
    // A SIGKILL at exec on macOS is the code-signature-cache rejection: re-sign
    // ad-hoc and give it exactly one more chance before failing.
    if verdict.is_sigkill() {
        if let Some(retry) = resign_and_reverify(&dest, &args.verify_arg) {
            verdict = retry;
        }
    }

    match verdict {
        ExecVerdict::Ok { exit_code } => {
            println!(
                "verified: `{} {}` execs cleanly (exit {exit_code})",
                installed_name(&args),
                args.verify_arg
            );
            if exit_code != 0 {
                println!(
                    "note: nonzero exit from the verify arg — exec itself worked, so the binary is not signature-blocked"
                );
            }
        }
        ExecVerdict::Signal { signal, hint } => {
            fail_exec(&dest, &format!("signal {signal}"), &hint)
        }
        ExecVerdict::Timeout { hint } => fail_exec(&dest, "timeout", &hint),
        ExecVerdict::SpawnError { hint } => fail_exec(&dest, "spawn error", &hint),
    }
}

/// Re-sign the installed binary ad-hoc and exec it once more. Returns the new
/// verdict on macOS (where the code-signature cache is the culprit); returns
/// `None` on other platforms, where there is nothing to re-sign.
fn resign_and_reverify(dest: &Path, verify_arg: &str) -> Option<ExecVerdict> {
    #[cfg(target_os = "macos")]
    {
        eprintln!("exec check got SIGKILL; re-signing ad-hoc and retrying once…");
        let _ = process::Command::new("codesign")
            .arg("-f")
            .arg("-s")
            .arg("-")
            .arg(dest)
            .status();
        Some(verify_exec(dest, verify_arg, DEFAULT_VERIFY_TIMEOUT))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (dest, verify_arg);
        None
    }
}

/// Report a failed exec check and exit 1: the installed binary is booby-trapped
/// (won't survive exec), so leaving it on `PATH` silently would be worse.
fn fail_exec(dest: &Path, descriptor: &str, hint: &str) -> ! {
    eprintln!(
        "FAILED: {} does not survive exec ({descriptor})",
        dest.display()
    );
    eprintln!("{hint}");
    process::exit(1);
}
