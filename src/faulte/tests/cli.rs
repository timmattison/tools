//! Tests that run the built `faulte` binary.

use std::process::{Command, Output};

/// The path of the binary that cargo built for these tests.
const BIN: &str = env!("CARGO_BIN_EXE_faulte");

/// Runs the binary with `args` and returns what it wrote and how it ended.
fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .output()
        .expect("the faulte binary starts")
}

/// `--version` gives the name, the package version, the git hash, and the
/// state of the tree, in the format that every tool of this repository uses.
#[test]
fn version_names_the_tool_the_release_and_the_build() {
    let output = run(&["--version"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "faulte --version must exit 0, it ended with {:?} and wrote {stderr:?}",
        output.status
    );
    assert!(
        stdout.starts_with("faulte 0.1.0 ("),
        "the version line must name the tool and the release: {stdout:?}"
    );
    assert!(
        stdout.ends_with(")\n"),
        "the version line must end with the build facts in parentheses: {stdout:?}"
    );
}
