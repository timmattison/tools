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

#[test]
fn standard_output_that_is_not_a_terminal_is_refused_before_any_drawing() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let run = run_ha(&home, &["timmattison/tools"]);
    assert_eq!(run.code, Some(2), "{}", run.stderr);
    assert!(run.stdout.is_empty(), "{:?}", run.stdout);
    assert!(
        run.stderr.contains("standard output is not a terminal"),
        "{}",
        run.stderr
    );
}

/// A `gh` stub for the test of a first refresh that reads no target. It answers
/// the preflight (`rate_limit`) with a good status and the three rate headers.
/// It answers every other call with a 404 and exit status 1, as `gh api -i`
/// does.
#[cfg(unix)]
const GH_STUB_404: &str = "#!/bin/sh
for argument in \"$@\"; do
  if [ \"$argument\" = rate_limit ]; then
    printf 'HTTP/2.0 200 OK\\r\\n'
    printf 'X-Ratelimit-Limit: 5000\\r\\n'
    printf 'X-Ratelimit-Remaining: 4999\\r\\n'
    printf 'X-Ratelimit-Reset: 4102444800\\r\\n'
    printf 'Content-Type: application/json\\r\\n'
    printf '\\r\\n'
    printf '{}'
    exit 0
  fi
done
printf 'HTTP/2.0 404 Not Found\\r\\n'
printf 'Content-Type: application/json\\r\\n'
printf '\\r\\n'
printf '{\"message\":\"Not Found\"}'
exit 1
";

/// Runs the program that follows it: the first argument is a number of seconds,
/// and the rest is the command. It sets a kernel alarm, puts standard input and
/// standard error on the terminal, and replaces itself with the command.
///
/// The alarm bounds the child. A broken `ha` never ends by itself, and a child
/// that a killed `cargo` leaves alive reparents to `launchd` and runs forever.
/// The alarm survives `exec` and the death of the test, so the kernel stops the
/// child at the bound.
///
/// Standard input goes to the terminal because of `crossterm`. When standard
/// input is not a terminal, `crossterm` opens `/dev/tty`, and on macOS the
/// event source cannot poll that device. A real session has standard input on
/// the terminal, so the test starts `ha` the same way.
///
/// Standard error goes to the terminal too. The test can then prove the order:
/// the error comes after the terminal is restored, the same as a person sees
/// it.
#[cfg(unix)]
const BOUND_ON_THE_TERMINAL: &str = r#"alarm(shift @ARGV); open(STDIN, '<&', \*STDOUT) or die "stdin: $!"; open(STDERR, '>&', \*STDOUT) or die "stderr: $!"; exec { $ARGV[0] } @ARGV or die "exec: $!";"#;

/// The bytes that leave the alternate screen. `ratatui::restore` writes them.
#[cfg(unix)]
const LEAVE_ALTERNATE_SCREEN: &str = "\x1b[?1049l";

/// The header of the error of a first refresh that reads no target.
#[cfg(unix)]
const NO_TARGET_HEADER: &str = "the first refresh could not read any target";

#[cfg(unix)]
#[test]
fn a_first_refresh_that_reads_no_target_exits_1_with_the_error_after_the_terminal_is_restored() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().expect("a temporary directory");
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).expect("the bin directory is made");
    let gh = bin.join("gh");
    std::fs::write(&gh, GH_STUB_404).expect("the gh stub is written");
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755))
        .expect("the gh stub is executable");

    let mut command = Command::new("perl");
    command
        .arg("-e")
        .arg(BOUND_ON_THE_TERMINAL)
        .arg("60")
        .arg(env!("CARGO_BIN_EXE_ha"))
        .arg("does-not-exist/nope")
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("TERM", "xterm-256color");

    let output = gitscratch::testing::pty::Pty::open(100).run_with_stdout_on_terminal(command);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();

    assert_eq!(
        output.status.code(),
        Some(1),
        "ha must exit with status 1. A child that the alarm stopped has no exit code. status: {:?}, terminal: {text:?}",
        output.status
    );
    assert!(
        text.contains(LEAVE_ALTERNATE_SCREEN),
        "the terminal must be restored. terminal: {text:?}"
    );
    let after_restore = text
        .rsplit_once(LEAVE_ALTERNATE_SCREEN)
        .map_or("", |(_, tail)| tail);
    let expected = "ha: the first refresh could not read any target:\n  does-not-exist/nope: GitHub answered 404: Not Found\nGitHub answers 404 for a repository that does not exist, and for a private repository that this gh account cannot read.\nRun gh auth status to see the account.\n";
    assert!(
        after_restore.contains(expected),
        "the full error must follow the restore. terminal: {text:?}"
    );
    assert_eq!(
        text.matches(NO_TARGET_HEADER).count(),
        1,
        "the header must occur one time. terminal: {text:?}"
    );
}
