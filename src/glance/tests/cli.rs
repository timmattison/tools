#![deny(unsafe_code)]
//! Tests that run the built `glance` binary.

use std::process::Output;

/// The exit code that clap gives for a usage error.
#[cfg(target_os = "macos")]
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
