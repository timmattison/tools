//! End-to-end tests of the command line of `ha`.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::process::Command;

#[test]
fn version_prints_name_number_and_build() {
    let output = Command::new(env!("CARGO_BIN_EXE_ha"))
        .arg("--version")
        .output()
        .expect("ha starts");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(
        stdout.starts_with("ha 0.1.0 ("),
        "unexpected version text: {stdout:?}"
    );
    assert!(output.status.success());
}

/// The result of one run of the `ha` binary.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Runs `ha` with `args`. `HOME` and `XDG_CONFIG_HOME` point at `home`, an
/// empty directory that belongs to the test, so the run never reads the real
/// configuration of the user.
fn run_ha(home: &tempfile::TempDir, args: &[&str]) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_ha"))
        .args(args)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("COLUMNS", "200")
        .output()
        .expect("ha starts");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("stderr is UTF-8"),
    }
}

/// Writes `text` to `name` in `home` and returns the path as text.
fn write_config(home: &tempfile::TempDir, name: &str, text: &str) -> String {
    let path = home.path().join(name);
    std::fs::write(&path, text).expect("the config file is written");
    path.display().to_string()
}

#[test]
fn help_lists_the_options_and_shows_the_config_file() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let run = run_ha(&home, &["--help"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    for needle in [
        "--config",
        "--window",
        "--interval",
        "TARGET",
        "config.toml",
        "repos = [",
    ] {
        assert!(
            run.stdout.contains(needle),
            "missing {needle:?} in:\n{}",
            run.stdout
        );
    }
}

#[test]
fn an_empty_config_file_and_no_targets_is_a_usage_error() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let config = write_config(&home, "empty.toml", "");
    let run = run_ha(&home, &["--config", &config]);
    assert_eq!(run.code, Some(2), "{}", run.stderr);
    assert!(run.stderr.contains("no targets"), "{}", run.stderr);
    assert!(run.stderr.starts_with("ha: "), "{}", run.stderr);
}

#[test]
fn an_unknown_config_key_is_named() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let config = write_config(&home, "colour.toml", "colour = \"red\"\n");
    let run = run_ha(&home, &["--config", &config]);
    assert_eq!(run.code, Some(2), "{}", run.stderr);
    assert!(run.stderr.contains("colour"), "{}", run.stderr);
}

#[test]
fn a_config_file_that_does_not_exist_is_named() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let missing = home.path().join("missing.toml").display().to_string();
    let run = run_ha(&home, &["--config", &missing]);
    assert_eq!(run.code, Some(2), "{}", run.stderr);
    assert!(run.stderr.contains(&missing), "{}", run.stderr);
}

#[test]
fn a_malformed_target_is_quoted() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let run = run_ha(&home, &["a/b/c"]);
    assert_eq!(run.code, Some(2), "{}", run.stderr);
    assert!(run.stderr.contains("\"a/b/c\""), "{}", run.stderr);
}

#[test]
fn a_bad_window_is_a_usage_error_that_quotes_the_value() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let run = run_ha(&home, &["--window", "8x", "owner"]);
    assert_eq!(run.code, Some(2), "{}", run.stderr);
    assert!(run.stderr.contains("8x"), "{}", run.stderr);
}
