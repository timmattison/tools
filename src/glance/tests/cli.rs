#![deny(unsafe_code)]
//! Tests that run the built `glance` binary.

#[cfg(target_os = "macos")]
use std::io::{BufRead, BufReader, Read};
use std::process::Output;
#[cfg(target_os = "macos")]
use std::process::Stdio;

/// The exit code that clap gives for a usage error.
const USAGE_ERROR: i32 = 2;

/// The tolerance of the sum of the three CPU shares.
#[cfg(target_os = "macos")]
const SUM_TOLERANCE: f64 = 1e-9;

/// The number of keys in the JSON line of `--sample`.
#[cfg(target_os = "macos")]
const KEY_COUNT: usize = 7;

/// Runs the binary with `args` and returns what it wrote and how it ended.
fn run(args: &[&str]) -> Output {
    childenv::fixture_command(env!("CARGO_BIN_EXE_glance"))
        .args(args)
        .output()
        .expect("the glance binary starts")
}

#[cfg(target_os = "macos")]
#[test]
fn sample_prints_one_json_line() {
    let output = run(&["--sample", "--interval", "0.25"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(stdout.ends_with('\n'), "{stdout:?}");
    assert_eq!(stdout.lines().count(), 1, "{stdout:?}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(value.as_object().expect("an object").len(), KEY_COUNT);
    let host = value["host"].as_str().expect("host");
    assert!(!host.is_empty() && !host.contains('.'), "{host}");
    let fraction = |key: &str| {
        let number = value[key].as_f64().expect(key);
        assert!((0.0..=1.0).contains(&number), "{key} {number}");
        number
    };
    let sum = fraction("user") + fraction("system") + fraction("idle");
    assert!((sum - 1.0).abs() < SUM_TOLERANCE, "sum {sum}");
    let name = value["pressure_level"].as_str().expect("pressure_level");
    assert!(["normal", "warn", "critical"].contains(&name), "{name}");
    let raw = value["pressure_level_raw"].as_i64().expect("raw level");
    assert!([1, 2, 4].contains(&raw), "{raw}");
    let percent = value["pressure_percent"].as_u64().expect("percent");
    assert!(percent <= 100, "{percent}");
}

/// The number of lines that the stream test asks for.
#[cfg(target_os = "macos")]
const STREAM_LINES: usize = 3;

#[cfg(target_os = "macos")]
#[test]
fn stream_prints_one_sample_line_for_each_interval() {
    let hostname = childenv::fixture_command("hostname")
        .arg("-s")
        .output()
        .expect("hostname starts");
    let expected = String::from_utf8(hostname.stdout).expect("hostname is UTF-8");
    let count = STREAM_LINES.to_string();
    let output = run(&["--stream", "--count", &count, "--interval", "0.25"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert_eq!(stdout.lines().count(), STREAM_LINES, "{stdout:?}");
    for line in stdout.lines() {
        match glance::text::parse_line(line).expect("a stream line") {
            glance::text::StreamLine::Sample { host, .. } => {
                assert_eq!(host.as_str(), expected.trim());
            }
            other => panic!("not a sample: {other:?}"),
        }
    }
    let first = stdout.lines().next().expect("a first line");
    let value: serde_json::Value = serde_json::from_str(first).expect("JSON");
    assert_eq!(value.as_object().expect("an object").len(), KEY_COUNT);
}

/// The number of lines that the closed pipe test asks for, as a bound.
#[cfg(target_os = "macos")]
const BOUND_LINES: &str = "40";

#[cfg(target_os = "macos")]
#[test]
fn a_closed_pipe_ends_the_stream_with_status_0_and_no_text() {
    // The first start of a fresh binary can be slow, so start it once here.
    let _ = run(&["--version"]);
    let mut child = childenv::fixture_command(env!("CARGO_BIN_EXE_glance"))
        .args(["--stream", "--count", BOUND_LINES, "--interval", "0.25"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the glance binary starts");
    let stdout = child.stdout.take().expect("a piped stdout");
    let mut first = String::new();
    BufReader::new(stdout)
        .read_line(&mut first)
        .expect("a first line");
    assert!(first.ends_with('\n'), "{first:?}");
    // The reader is gone at the end of the statement above, so the pipe is closed.
    let mut stderr_text = String::new();
    child
        .stderr
        .take()
        .expect("a piped stderr")
        .read_to_string(&mut stderr_text)
        .expect("stderr is UTF-8");
    let status = child.wait().expect("the child ends");
    assert_eq!(status.code(), Some(0), "{status:?} {stderr_text:?}");
    assert_eq!(stderr_text, "");
}

#[test]
fn stream_and_sample_together_are_a_usage_error() {
    let output = run(&["--stream", "--sample"]);
    assert_eq!(output.status.code(), Some(USAGE_ERROR), "{output:?}");
}

#[test]
fn count_without_stream_is_a_usage_error() {
    let output = run(&["--count", "3"]);
    assert_eq!(output.status.code(), Some(USAGE_ERROR), "{output:?}");
}

/// An SSH destination that the tests give.
const DESTINATION: &str = "m1-ts";

/// A destination that `ssh` reads as an option.
const DASH_DESTINATION: &str = "-oProxyCommand=x";

/// Asserts that `args` is a usage error whose text holds `words`.
fn assert_usage_error(args: &[&str], words: &str) {
    let output = run(args);
    assert_eq!(
        output.status.code(),
        Some(USAGE_ERROR),
        "{args:?} {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(words), "{args:?} {stderr}");
}

#[test]
fn host_with_stream_or_sample_is_a_usage_error() {
    assert_usage_error(&["--host", DESTINATION, "--stream"], "--stream");
    assert_usage_error(&["--host", DESTINATION, "--sample"], "--sample");
}

#[test]
fn a_destination_that_starts_with_a_dash_is_a_usage_error() {
    // Apart from its flag, the value reaches clap as a flag of its own.
    assert_usage_error(&["--host", DASH_DESTINATION], "unexpected argument '-o'");
    let joined = format!("--host={DASH_DESTINATION}");
    assert_usage_error(
        &[joined.as_str()],
        &format!("must not start with \"-\", and \"{DASH_DESTINATION}\" does"),
    );
}

#[test]
fn an_empty_destination_is_a_usage_error() {
    assert_usage_error(&["--host", ""], "the destination must not be empty");
}

#[test]
fn remote_binary_without_host_is_a_usage_error() {
    assert_usage_error(&["--remote-binary", "glance"], "--host");
}

#[test]
fn an_empty_remote_binary_is_a_usage_error() {
    assert_usage_error(
        &["--host", DESTINATION, "--remote-binary", ""],
        "the remote binary must not be empty",
    );
}

#[cfg(target_os = "macos")]
#[test]
fn an_interval_below_the_minimum_is_a_usage_error() {
    let output = run(&["--sample", "--interval", "0.1"]);
    assert_eq!(output.status.code(), Some(USAGE_ERROR), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("0.25"), "{stderr}");
}

#[test]
fn version_names_the_build() {
    let output = run(&["--version"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(stdout.starts_with("glance 0.1.0 ("), "{stdout:?}");
    assert!(
        [", clean)\n", ", dirty)\n", ", unknown)\n"]
            .iter()
            .any(|end| stdout.ends_with(end)),
        "{stdout:?}"
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn another_platform_refuses_sample() {
    let output = run(&["--sample"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "glance supports macOS only.\n"
    );
}
