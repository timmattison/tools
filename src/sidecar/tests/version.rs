#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! The `--version` line of the binary.

/// True when `text` is `<hash>, <state>` with a 7 digit hex hash or `unknown`,
/// and a state of `clean`, `dirty`, or `unknown`.
fn is_build_info(text: &str) -> bool {
    let Some((hash, state)) = text.split_once(", ") else {
        return false;
    };
    let hash_ok =
        hash == "unknown" || (hash.len() == 7 && hash.chars().all(|c| c.is_ascii_hexdigit()));
    hash_ok && matches!(state, "clean" | "dirty" | "unknown")
}

#[test]
fn version_line_holds_the_build_information() {
    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_sidecar"));
    let output = command.arg("--version").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let line = stdout.trim_end();
    let info = line
        .strip_prefix("sidecar 0.1.0 (")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_else(|| panic!("unexpected version line: {line:?}"));
    assert!(
        is_build_info(info),
        "unexpected build information: {info:?}"
    );
}

#[test]
fn build_information_check_refuses_bad_text() {
    for bad in [
        "",
        "abc1234",
        "abc123, clean",
        "abc1234, tidy",
        "ghijklm, clean",
    ] {
        assert!(!is_build_info(bad), "{bad:?}");
    }
    for good in ["abc1234, clean", "0123456, dirty", "unknown, unknown"] {
        assert!(is_build_info(good), "{good:?}");
    }
}
