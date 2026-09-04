//! `prcp` offers no shell integration.
//!
//! A copy of a file is work a child process does. `prcp` thus has no reason to
//! write into the shell config of a user, and issue #265 removed the
//! `--shell-setup` flag that did so. The `prmv` function that flag installed
//! was a shorthand for `prcp --rm`. A user who wants the shorthand writes
//! `alias prmv='prcp --rm'`.
//!
//! Two tests run the binary, and each of them gives it a temporary home
//! directory. A build that still holds the integration thus writes its block
//! there, and the real shell config of the user stays untouched.
//!
//! Each of those two tests asserts through a function of its own, and each
//! function holds one assertion that only a run of the real binary satisfies.
//! Absences alone make no guard here, because a `prcp` that fails to start
//! writes no shell config and no help, and that satisfies every absence. A
//! third test feeds the two functions such a silent stand-in and asserts that
//! both of them panic.

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

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
#[cfg(unix)]
use std::panic;
use std::path::Path;
#[cfg(unix)]
use std::process::ExitStatus;
use std::process::{Command, Output};
use tempfile::TempDir;

/// The shell config files `shellsetup` writes, named relative to the home
/// directory.
const SHELL_CONFIG_FILES: [&str; 3] = [".zshrc", ".bashrc", ".bash_profile"];

/// The phrase clap writes when it meets an argument it does not know.
///
/// The assertion on this phrase is the positive control of
/// [`assert_shell_setup_rejected`], because only a binary that ran and read the
/// flag writes it. A clap upgrade that changes the words breaks that assertion,
/// and the name of this constant says where to look.
const CLAP_UNEXPECTED_ARGUMENT: &str = "unexpected argument '--shell-setup' found";

/// A flag the help of `prcp` keeps.
///
/// `prcp --rm` is the interface that replaced the `prmv` function, so this flag
/// stays. The assertion on it is the positive control of
/// [`assert_help_offers_no_shell_integration`], because an empty help fails it.
const RM_FLAG: &str = "--rm";

/// The raw wait status of a process that exited with the code 1.
///
/// [`ExitStatusExt::from_raw`] takes the wait status `waitpid` reports, and
/// that status holds the exit code in the high byte of its low 16 bits.
#[cfg(unix)]
const EXIT_CODE_ONE_WAIT_STATUS: i32 = 1 << 8;

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

/// Assert that a run of `prcp --shell-setup` rejected the flag and wrote no
/// shell config.
///
/// `output` holds the result of that run, and `home` names the home directory
/// the run got. The assertions live in a function of their own, apart from the
/// run, so a test feeds them a result the binary never made.
///
/// # Panics
///
/// Panics when the run accepted the flag, when the run named no unexpected
/// argument, or when the run left a shell config behind.
fn assert_shell_setup_rejected(output: &Output, home: &Path) {
    let visible = visible_output(output);

    assert!(
        !output.status.success(),
        "prcp must reject --shell-setup, but the run succeeded:\n{visible}"
    );
    assert!(
        visible.contains(CLAP_UNEXPECTED_ARGUMENT),
        "prcp did not name --shell-setup as an unexpected argument. A binary that never ran satisfies every other assertion here:\n{visible}"
    );

    for name in SHELL_CONFIG_FILES {
        let path = home.join(name);
        assert!(
            !path.exists(),
            "prcp wrote the shell config {}. It must write no shell config at all",
            path.display()
        );
    }
}

/// Assert that the help of `prcp` offers no shell integration.
///
/// `help` holds the visible glyphs of a run of `prcp --help`. The assertions
/// live in a function of their own, apart from the run, so a test feeds them a
/// text the binary never wrote.
///
/// # Panics
///
/// Panics when the help offers no `--rm`, when the help still offers the
/// removed flag, or when it still names the function that flag installed.
fn assert_help_offers_no_shell_integration(help: &str) {
    assert!(
        help.contains(RM_FLAG),
        "the help of prcp offers no --rm. A help that says nothing satisfies every other assertion here:\n{help}"
    );
    assert!(
        !help.contains("--shell-setup"),
        "the help of prcp still offers --shell-setup:\n{help}"
    );
    assert!(
        !help.contains("prmv"),
        "the help of prcp still names prmv:\n{help}"
    );
}

#[test]
fn shell_setup_flag_is_rejected() {
    let (output, home) = run_with_isolated_home(&["--shell-setup"]);

    assert_shell_setup_rejected(&output, home.path());
}

#[test]
fn help_offers_no_shell_integration() {
    let (output, _home) = run_with_isolated_home(&["--help"]);

    assert_help_offers_no_shell_integration(&visible_output(&output));
}

/// The two assertion functions must reject a `prcp` that says nothing.
///
/// A `prcp` that fails to start exits non-zero, writes nothing, and thus writes
/// no shell config either. Every absence assertion above holds for such a run,
/// so a guard built out of absences alone reports the removal of the shell
/// integration as verified while it verifies nothing. This test builds that
/// silent stand-in and asserts that each assertion function panics on it.
///
/// The default panic hook stays in place through the two calls. That hook is
/// process-global, and `cargo test` runs the tests of one binary on many
/// threads, so a hook that prints nothing also hides the message of a sibling
/// test that fails at the same time. `cargo test` captures the output of each
/// test and prints it only for a test that fails, so the two panics of a run
/// that passes stay out of sight. A run with `--nocapture` prints them.
///
/// Unix only: [`ExitStatusExt::from_raw`] builds an [`ExitStatus`] out of a raw
/// wait status, and that is a unix extension.
#[cfg(unix)]
#[test]
fn the_assertions_reject_a_binary_that_says_nothing() {
    let home = tempfile::tempdir().expect("the test makes a temporary home directory");
    let silent = Output {
        status: ExitStatus::from_raw(EXIT_CODE_ONE_WAIT_STATUS),
        stdout: Vec::new(),
        stderr: Vec::new(),
    };

    let flag_verdict = panic::catch_unwind(|| assert_shell_setup_rejected(&silent, home.path()));
    let help_verdict = panic::catch_unwind(|| assert_help_offers_no_shell_integration(""));

    assert!(
        flag_verdict.is_err(),
        "assert_shell_setup_rejected accepted a prcp that exited 1 and wrote nothing. It needs an assertion that only a run of the real binary satisfies"
    );
    assert!(
        help_verdict.is_err(),
        "assert_help_offers_no_shell_integration accepted an empty help. It needs an assertion that only the real help satisfies"
    );
}
