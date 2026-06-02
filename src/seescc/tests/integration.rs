//! End-to-end tests driving the real `seescc` binary against a stub `sccache`
//! placed on a per-test PATH, so no live sccache server is needed. Each test
//! uses its own unique tempdir (parallel-safe), and stdout is captured (a
//! pipe), which is the scripting/one-shot path.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

/// The captured sccache 0.15.0 payload (Rust hits 1718, misses 963, etc.).
const FIXTURE: &str = include_str!("fixtures/sccache-0.15.0.json");

/// Write an executable `sccache` stub into `dir` with the given shell body.
fn write_stub(dir: &Path, body: &str) {
    let path = dir.join("sccache");
    fs::write(&path, body).expect("write stub");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// Run seescc with `path_value` as its PATH and `iso` as both `HOME` and
/// `XDG_CONFIG_HOME`, forwarding any extra `args`, and capturing the output.
///
/// Pointing `HOME`/`XDG_CONFIG_HOME` at a fresh empty tempdir isolates the run
/// from the developer's real `~/.config/seescc/config.toml` (or the macOS
/// `~/Library/Application Support/seescc/config.toml`), so `config::load(None)`
/// always falls back to the built-in defaults unless a `--config` arg overrides.
fn run_seescc_iso(path_value: &str, iso: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_seescc"))
        .env("PATH", path_value)
        .env("HOME", iso)
        .env("XDG_CONFIG_HOME", iso)
        .args(args)
        .output()
        .expect("invoke seescc")
}

/// PATH that finds the stub first, then the real system PATH (so the stub's
/// own `/bin/sh` and `cat` resolve).
fn path_with_stub(dir: &Path) -> String {
    let real = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", dir.display(), real)
}

/// Write the fixture into `dir` and install a `sccache` stub that `cat`s it.
fn write_fixture_stub(dir: &Path) {
    fs::write(dir.join("fixture.json"), FIXTURE).expect("write fixture");
    write_stub(
        dir,
        &format!("#!/bin/sh\ncat \"{}/fixture.json\"\n", dir.display()),
    );
}

#[test]
fn happy_path_shows_rust_only_metrics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let iso = tempfile::tempdir().expect("iso tempdir");
    write_fixture_stub(dir.path());

    let out = run_seescc_iso(&path_with_stub(dir.path()), iso.path(), &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "seescc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Header + the five default metrics with formatted Rust values.
    assert!(
        stdout.contains("sccache · Rust"),
        "missing header: {stdout}"
    );
    assert!(stdout.contains("Compile requests"));
    assert!(stdout.contains("4,786"));
    assert!(stdout.contains("Requests executed"));
    assert!(stdout.contains("3,880"));
    assert!(stdout.contains("Cache hits"));
    assert!(stdout.contains("1,718")); // Rust hits only
    assert!(stdout.contains("Cache misses"));
    assert!(stdout.contains("963")); // Rust misses only
    assert!(stdout.contains("Hit rate"));
    assert!(stdout.contains("64.1%"));

    // Rust-only: C/C++ and Assembler numbers must NOT leak in, and we must not
    // be summing across languages.
    assert!(!stdout.contains("516"), "C/C++ hits leaked: {stdout}");
    assert!(
        !stdout.contains("2,430"),
        "summed across all languages: {stdout}"
    );
    assert!(!stdout.contains("C/C++"));
    assert!(!stdout.contains("Assembler"));
}

#[test]
fn garbled_json_exits_nonzero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let iso = tempfile::tempdir().expect("iso tempdir");
    write_stub(dir.path(), "#!/bin/sh\nprintf 'this is not json'\n");

    let out = run_seescc_iso(&path_with_stub(dir.path()), iso.path(), &[]);
    assert!(!out.status.success(), "expected failure on garbled JSON");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("parse"),
        "stderr should mention parse failure: {stderr}"
    );
}

#[test]
fn sccache_nonzero_exit_is_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let iso = tempfile::tempdir().expect("iso tempdir");
    write_stub(dir.path(), "#!/bin/sh\necho boom >&2\nexit 2\n");

    let out = run_seescc_iso(&path_with_stub(dir.path()), iso.path(), &[]);
    assert!(
        !out.status.success(),
        "expected failure when sccache exits non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("failed"),
        "stderr should report the failed poll: {stderr}"
    );
}

#[test]
fn missing_sccache_exits_nonzero_with_clear_error() {
    let empty = tempfile::tempdir().expect("tempdir"); // empty dir, no sccache
    let iso = tempfile::tempdir().expect("iso tempdir");
    let out = run_seescc_iso(&empty.path().display().to_string(), iso.path(), &[]);
    assert!(
        !out.status.success(),
        "expected failure when sccache absent"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("sccache"),
        "stderr should name sccache: {stderr}"
    );
    assert!(
        stderr.contains("not found"),
        "stderr should say not found: {stderr}"
    );
}

/// A custom config (`--config`) must drive the rendered output: an empty
/// `languages` list sums across all languages, and arbitrary global metrics
/// render. Proves the binary honors `--config` rather than the built-in defaults.
#[test]
fn custom_config_changes_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let iso = tempfile::tempdir().expect("iso tempdir");
    write_fixture_stub(dir.path());

    let config_path = dir.path().join("custom.toml");
    fs::write(
        &config_path,
        r#"languages = []
metrics = [
  { key = "cache_hits", label = "Cache hits" },
  { key = "cache_writes", label = "Cache writes" },
]
"#,
    )
    .expect("write custom config");

    let out = run_seescc_iso(
        &path_with_stub(dir.path()),
        iso.path(),
        &["--config", &config_path.display().to_string()],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "seescc --config failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Empty `languages` ⇒ summed across all languages: 196 + 1718 + 516 = 2430.
    assert!(
        stdout.contains("2,430"),
        "all-language cache_hits (2,430) must appear: {stdout}"
    );
    // Must NOT be the Rust-only value.
    assert!(
        !stdout.contains("1,718"),
        "Rust-only cache_hits (1,718) leaked — languages = [] was ignored: {stdout}"
    );
    // An arbitrary global metric must render.
    assert!(
        stdout.contains("Cache writes"),
        "the configured Cache writes row must render: {stdout}"
    );
    assert!(
        stdout.contains("1,373"),
        "cache_writes value (1,373) must render: {stdout}"
    );
    // Empty languages ⇒ the header label reads "all".
    assert!(
        stdout.contains("sccache · all"),
        "empty languages must produce the `all` header label: {stdout}"
    );
}

/// `--write-default-config` must write the annotated defaults to the target
/// path (creating missing parents), refuse to clobber without `--force`, and
/// overwrite with `--force`. It must not require an sccache on PATH.
#[test]
fn write_default_config_roundtrip() {
    let iso = tempfile::tempdir().expect("iso tempdir");
    // A nested path whose parents do not yet exist.
    let target = iso.path().join("sub").join("config.toml");
    let target_arg = target.display().to_string();
    assert!(!target.exists(), "precondition: target must not exist");

    // No sccache stub on PATH: the write path must run before the which() check.
    let bare_path = iso.path().display().to_string();

    let first = run_seescc_iso(
        &bare_path,
        iso.path(),
        &["--write-default-config", "--config", &target_arg],
    );
    assert!(
        first.status.success(),
        "first --write-default-config must succeed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(target.exists(), "the config file must exist after writing");
    let written = fs::read_to_string(&target).expect("read back written config");
    assert!(
        written.contains(r#"languages = ["Rust"]"#),
        "written config must contain the default languages line: {written}"
    );

    // Without --force, a second write must refuse and say so.
    let second = run_seescc_iso(
        &bare_path,
        iso.path(),
        &["--write-default-config", "--config", &target_arg],
    );
    assert!(
        !second.status.success(),
        "second --write-default-config without --force must fail"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("already exists"),
        "stderr must mention the file already exists: {stderr}"
    );

    // With --force, the overwrite must succeed.
    let forced = run_seescc_iso(
        &bare_path,
        iso.path(),
        &["--write-default-config", "--force", "--config", &target_arg],
    );
    assert!(
        forced.status.success(),
        "--write-default-config --force must overwrite: {}",
        String::from_utf8_lossy(&forced.stderr)
    );
}
