//! `workit --version` names the build it came from.
//!
//! Every tool in this workspace reports the crate version together with the
//! git hash it was built from and whether that working tree was clean, so a
//! bug report identifies an exact build. The string comes from
//! `buildinfo::version_string!()`, which the build script resolves at compile
//! time.

use regex::Regex;
use std::process::Command;

/// The shape clap prints for `version = version_string!()`: the binary name,
/// the crate version, then the seven-character git hash and the state of the
/// working tree, in parentheses. Both halves inside the parentheses read
/// `unknown` when the build could not reach git.
const VERSION_PATTERN: &str =
    r"^workit \d+\.\d+\.\d+ \((?:[0-9a-f]{7}|unknown), (?:clean|dirty|unknown)\)$";

#[test]
fn version_reports_the_git_hash_and_the_dirty_status() {
    let output = Command::new(env!("CARGO_BIN_EXE_workit"))
        .arg("--version")
        .output()
        .expect("the workit binary runs");

    assert!(
        output.status.success(),
        "`workit --version` exited with {}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let printed = String::from_utf8(output.stdout).expect("`--version` writes UTF-8");
    let line = printed.trim_end();

    let pattern = Regex::new(VERSION_PATTERN).expect("the version pattern compiles");
    assert!(
        pattern.is_match(line),
        "`workit --version` printed {line:?}, which carries no git hash and no dirty status; \
         it must match {VERSION_PATTERN}"
    );
}
