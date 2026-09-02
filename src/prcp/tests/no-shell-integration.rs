//! `prcp` offers no shell integration.
//!
//! A copy of a file is work a child process does. `prcp` thus has no reason to
//! write into the shell config of a user, and issue #265 removed the
//! `--shell-setup` flag that did so. The `prmv` function that flag installed
//! was a shorthand for `prcp --rm`. A user who wants the shorthand writes
//! `alias prmv='prcp --rm'`.
//!
//! Each test gives the binary a temporary home directory. A build that still
//! holds the integration thus writes its block there, and the real shell config
//! of the user stays untouched.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![warn(clippy::panic)]
#![deny(clippy::unimplemented)]
#![warn(clippy::cast_possible_truncation)]
#![warn(clippy::cast_sign_loss)]
#![warn(clippy::cast_precision_loss)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "each unwrap and expect here acts on the temporary directory the test just made, or on the spawn of the freshly built binary. A failure of either is a broken harness, not the behavior under test"
)]

use std::process::{Command, Output};
use tempfile::TempDir;

/// The shell config files `shellsetup` writes, named relative to the home
/// directory.
const SHELL_CONFIG_FILES: [&str; 3] = [".zshrc", ".bashrc", ".bash_profile"];

/// Run the freshly built `prcp` binary against a home directory of its own.
///
/// `SHELL` names zsh, which `shellsetup` supports, so a build that still holds
/// the integration gets as far as the write instead of stopping at an
/// unsupported shell. The returned [`TempDir`] owns that home directory, and it
/// must stay alive while the caller reads it.
fn run_with_isolated_home(args: &[&str]) -> (Output, TempDir) {
    let home = tempfile::tempdir().expect("the test makes a temporary home directory");
    let output = Command::new(env!("CARGO_BIN_EXE_prcp"))
        .env("HOME", home.path())
        .env("SHELL", "/bin/zsh")
        .args(args)
        .output()
        .expect("the prcp binary runs");
    (output, home)
}

/// The visible glyphs of both output streams of the run, as one string.
fn visible_output(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    testcolor::strip_ansi(&text)
}

#[test]
fn shell_setup_flag_is_rejected() {
    let (output, home) = run_with_isolated_home(&["--shell-setup"]);

    assert!(
        !output.status.success(),
        "prcp must reject --shell-setup, but the run succeeded:\n{}",
        visible_output(&output)
    );

    for name in SHELL_CONFIG_FILES {
        let path = home.path().join(name);
        assert!(
            !path.exists(),
            "prcp wrote the shell config {}. It must write no shell config at all",
            path.display()
        );
    }
}

#[test]
fn help_offers_no_shell_integration() {
    let (output, _home) = run_with_isolated_home(&["--help"]);
    let help = visible_output(&output);

    assert!(
        !help.contains("--shell-setup"),
        "the help of prcp still offers --shell-setup:\n{help}"
    );
    assert!(
        !help.contains("prmv"),
        "the help of prcp still names prmv:\n{help}"
    );
}
