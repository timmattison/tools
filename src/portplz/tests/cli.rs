//! CLI characterization tests for the `portplz` binary.
//!
//! These lock in the externally-observable output of the thin-shell CLI so the
//! refactor onto `portplz_core::derive` cannot silently change a derived port or
//! the `--verbose` wording. The `--no-git` directory case is fully deterministic
//! once the user is pinned: each test sets `PORTPLZ_UID` on the child process
//! (child-env only, so concurrent runs stay isolated and the result is
//! machine-independent), and `/tmp` (basename `tmp`) does no filesystem work, so
//! the expected values are stable constants.

use std::process::Command;

/// `portplz /tmp --no-git` with `PORTPLZ_UID=0` derives the port from
/// `"0\ntmp"`, which always yields this port.
const TMP_PORT_UID0: &str = "19642";

/// The same run with `--name api` frames the name at the head of that input,
/// giving `"\0api\00\ntmp"`, which always yields this port.
const TMP_API_PORT_UID0: &str = "34296";

/// Runs the binary with `PORTPLZ_UID` pinned so the derived port does not depend
/// on whoever runs the test suite.
fn run_with_uid(uid: &str, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_portplz"))
        .env("PORTPLZ_UID", uid)
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
    assert_eq!(run_with_uid("0", &["/tmp", "--no-git"]), TMP_PORT_UID0);
}

#[test]
fn no_git_verbose_prints_the_directory_description_with_user() {
    assert_eq!(
        run_with_uid("0", &["/tmp", "--no-git", "--verbose"]),
        format!("Port {TMP_PORT_UID0} for directory 'tmp' (no git repo) (uid 0)")
    );
}

/// A name gives one application of a project its own port, and the port is as
/// stable as the one the project gets without a name.
#[test]
fn a_name_gives_its_own_stable_port() {
    assert_eq!(
        run_with_uid("0", &["/tmp", "--no-git", "--name", "api"]),
        TMP_API_PORT_UID0
    );
    assert_ne!(
        run_with_uid("0", &["/tmp", "--no-git", "--name", "api"]),
        TMP_PORT_UID0,
        "a named run must not take the port of the run that carries no name"
    );
}

/// `-n` is the short spelling of `--name`, and nothing else.
#[test]
fn the_short_name_flag_is_the_long_one() {
    assert_eq!(
        run_with_uid("0", &["/tmp", "--no-git", "-n", "api"]),
        TMP_API_PORT_UID0
    );
}

/// The path keeps the one positional slot, so every command line that works
/// today still works.
#[test]
fn the_path_keeps_the_positional_slot_beside_a_name() {
    assert_eq!(
        run_with_uid("0", &["--name", "api", "/tmp", "--no-git"]),
        TMP_API_PORT_UID0,
        "the path must still be read as a path when a name is given too"
    );
}

#[test]
fn name_verbose_prints_the_name() {
    assert_eq!(
        run_with_uid("0", &["/tmp", "--no-git", "--name", "api", "--verbose"]),
        format!("Port {TMP_API_PORT_UID0} for directory 'tmp' (no git repo) named 'api' (uid 0)")
    );
}

/// An empty name is not the absence of a name, because it gives a third port.
/// A script that runs `portplz -n "$APP"` with `APP` unset must get a usage
/// error, not a port that plain `portplz` and `sirn` never use.
#[test]
fn an_empty_name_is_refused() {
    for flag in ["--name", "-n"] {
        let output = Command::new(env!("CARGO_BIN_EXE_portplz"))
            .env("PORTPLZ_UID", "0")
            .args(["/tmp", "--no-git", flag, ""])
            .output()
            .expect("run portplz binary");
        assert_eq!(
            output.status.code(),
            Some(2),
            "portplz {flag} \"\" must exit with the usage-error status"
        );
        assert!(
            output.stdout.is_empty(),
            "portplz {flag} \"\" must print no port, got: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8(output.stderr).expect("portplz stderr is valid UTF-8");
        assert!(
            stderr.contains("--name"),
            "stderr for {flag} \"\" must name --name, got: {stderr:?}"
        );
    }
}

#[test]
fn portplz_rejects_malformed_uid() {
    // A malformed PORTPLZ_UID must be a hard error, not silently ignored. Set the
    // env var on the child only so concurrent test runs stay isolated.
    let output = Command::new(env!("CARGO_BIN_EXE_portplz"))
        .env("PORTPLZ_UID", "abc")
        .args(["/tmp", "--no-git"])
        .output()
        .expect("run portplz binary");
    assert!(
        !output.status.success(),
        "portplz must exit non-zero on a malformed PORTPLZ_UID"
    );
    let stderr = String::from_utf8(output.stderr).expect("portplz stderr is valid UTF-8");
    assert!(
        stderr.contains("PORTPLZ_UID"),
        "stderr must mention PORTPLZ_UID, got: {stderr:?}"
    );
}

#[test]
fn portplz_uid_override_changes_the_port() {
    // Two different users derive different ports for the same location, so the
    // suite stays correct even when the real runner happens to be uid 0.
    let port_a = run_with_uid("0", &["/tmp", "--no-git"]);
    let port_b = run_with_uid("1", &["/tmp", "--no-git"]);
    assert_ne!(
        port_a, port_b,
        "PORTPLZ_UID must change the derived port for the same directory"
    );
}
