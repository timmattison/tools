//! CLI characterization tests for the `portplz` binary.
//!
//! These lock in the externally-observable output of the thin-shell CLI so the
//! refactor onto `portplz_core::derive` cannot silently change a derived port or
//! the `--verbose` wording. The `--no-git` directory case is fully deterministic
//! (the port arithmetic is frozen and no filesystem access occurs for `--no-git`),
//! so the expected values are stable constants and the test is parallel-safe.

use std::process::Command;

/// `portplz <path> --no-git` derives the port from the path's basename only,
/// so `/tmp` (basename `tmp`) always yields this port.
const TMP_PORT: &str = "53008";

fn run(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_portplz"))
        .args(args)
        .output()
        .expect("run portplz binary");
    assert!(
        output.status.success(),
        "portplz {args:?} exited with failure: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("portplz stdout is valid UTF-8")
        .trim_end()
        .to_string()
}

#[test]
fn no_git_prints_just_the_port() {
    assert_eq!(run(&["/tmp", "--no-git"]), TMP_PORT);
}

#[test]
fn no_git_verbose_prints_the_directory_description() {
    assert_eq!(
        run(&["/tmp", "--no-git", "--verbose"]),
        format!("Port {TMP_PORT} for directory 'tmp' (no git repo)")
    );
}
