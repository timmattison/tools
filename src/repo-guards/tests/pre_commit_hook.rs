//! Integration tests that run the repo's real `.husky/pre-commit` hook against
//! throwaway fixtures and assert on its exit status.
//!
//! These cover *existing* correct behavior of the hook, so a TDD red phase is
//! impossible — they pass immediately against the current hook. The mutation
//! check that substitutes for red (neuter the hook, watch
//! `staged_misformatted_rust_file_fails_the_gate` fail, restore, watch it pass)
//! is documented in the commit body, not committed as code.
//!
//! Parallel safety: a `bacon` loop runs `cargo test` concurrently with the
//! pre-commit hook's own `cargo test`, so two copies of each test can execute
//! at once. Every fixture directory is keyed on the process id, a nanosecond
//! timestamp, AND a process-wide atomic counter. The counter is load-bearing:
//! `cargo` runs these tests on parallel threads, and pid+nanos alone collides
//! when two threads sample the clock in the same tick — two tests then share a
//! dir and the second `git init` aborts with a template-copy "File exists".

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A deliberately misformatted Rust source file. `cargo fmt --check` rejects it
/// because of the run-together braces, doubled spaces, and stray whitespace.
const MISFORMATTED_MAIN_RS: &str = "fn main()    {println!(\"hi\" ) ;}\n";

/// Absolute, canonical path to the real hook under test.
fn hook_path() -> PathBuf {
    let hook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.husky/pre-commit");
    fs::canonicalize(&hook)
        .unwrap_or_else(|e| panic!("cannot canonicalize hook path {}: {e}", hook.display()))
}

/// Path to the repo's pinned toolchain file, copied into each fixture so
/// `cargo fmt` resolves the same rustfmt as the rest of the workspace.
fn rust_toolchain_path() -> PathBuf {
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rust-toolchain.toml");
    fs::canonicalize(&toolchain).unwrap_or_else(|e| {
        panic!(
            "cannot canonicalize rust-toolchain path {}: {e}",
            toolchain.display()
        )
    })
}

/// Monotonic per-process counter that disambiguates fixture dirs created within
/// the same clock tick. Without it, two parallel test threads can produce the
/// same pid+nanos name and collide.
static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Create a process-unique fixture directory.
fn unique_fixture_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before UNIX_EPOCH")
        .as_nanos();
    let seq = FIXTURE_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("repo-guards-{}-{nanos}-{seq}", std::process::id()));
    fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

/// Build a minimal cargo package inside `dir` whose `src/main.rs` is
/// misformatted. The empty `[workspace]` table stops cargo from walking up into
/// any parent manifest, and the copied `rust-toolchain.toml` pins rustfmt.
fn write_fixture_package(dir: &Path) {
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .expect("write fixture Cargo.toml");

    fs::create_dir_all(dir.join("src")).expect("create fixture src dir");
    fs::write(dir.join("src/main.rs"), MISFORMATTED_MAIN_RS).expect("write misformatted main.rs");

    fs::copy(rust_toolchain_path(), dir.join("rust-toolchain.toml"))
        .expect("copy rust-toolchain.toml into fixture");
}

/// `git init` the fixture and stage `paths`. No commit (hence no identity
/// config) is needed: `git diff --cached` compares the index to the empty tree.
fn git_init_and_stage(dir: &Path, paths: &[&str]) {
    run_git(dir, &["init", "-q"]);
    let mut add = vec!["add"];
    add.extend_from_slice(paths);
    run_git(dir, &add);
}

/// Run `git -C <dir> <args>` and assert it succeeded.
fn run_git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        // Scrub inherited git env so the fixture repo is the one git operates
        // on, even when invoked from inside this repo's own git hooks/tests.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn git {args:?}: {e}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Run the real hook with `dir` as CWD, scrubbing inherited git env vars so the
/// hook operates on the fixture repo rather than this test's repo. Returns true
/// if the hook exited zero.
fn run_hook(dir: &Path) -> bool {
    Command::new("bash")
        .arg(hook_path())
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .status()
        .expect("failed to spawn pre-commit hook")
        .success()
}

/// Best-effort cleanup; the per-process+nanos path is the real isolation, so a
/// failed removal cannot collide with another run.
fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Mutation test for the gate: a staged, misformatted `.rs` file must make the
/// hook exit non-zero. If this ever passes with a broken hook, the guard is
/// dead — that is exactly what we are protecting against.
#[test]
fn staged_misformatted_rust_file_fails_the_gate() {
    let dir = unique_fixture_dir();
    write_fixture_package(&dir);
    git_init_and_stage(&dir, &["src/main.rs", "Cargo.toml", "rust-toolchain.toml"]);

    let passed = run_hook(&dir);
    cleanup(&dir);

    assert!(
        !passed,
        "hook should reject a staged misformatted .rs file, but it exited zero"
    );
}

/// A minimal `rustfmt.toml` whose only job is to be a staged file that matches
/// the gate's trigger regex. The pinned style edition mirrors the workspace.
const RUSTFMT_TOML: &str = "style_edition = \"2021\"\n";

/// Staging only a toolchain/style config file (and nothing else) must still
/// trigger the gate, because such a commit is the exact scenario most likely to
/// introduce formatting/lint drift. We prove the gate fires by leaving a
/// misformatted `src/main.rs` unstaged on disk: if the gate runs, `cargo fmt
/// --check` sees the dirty tree and the hook exits non-zero; if the gate is
/// skipped, the hook exits zero and this test fails.
#[test]
fn staged_toolchain_config_alone_triggers_the_gate() {
    // `rust-toolchain.toml` is already written by `write_fixture_package`, so it
    // only needs staging; `rustfmt.toml` is written here first.
    for config_file in ["rustfmt.toml", "rust-toolchain.toml"] {
        let dir = unique_fixture_dir();
        write_fixture_package(&dir);
        fs::write(dir.join("rustfmt.toml"), RUSTFMT_TOML).expect("write rustfmt.toml");

        // Stage ONLY the config file. The misformatted main.rs stays unstaged,
        // so the gate must fire on the config file alone to catch it.
        git_init_and_stage(&dir, &[config_file]);

        let passed = run_hook(&dir);
        cleanup(&dir);

        assert!(
            !passed,
            "hook should run the gate when {config_file} is staged, but it exited zero (gate skipped)"
        );
    }
}

/// When only a non-Rust file is staged, the trigger condition is false and the
/// gate is skipped — so the hook exits zero even though a misformatted
/// `src/main.rs` sits on disk (unstaged). This proves the trigger, not the
/// formatter, is what runs: if the gate fired anyway it would catch the file
/// and fail.
#[test]
fn staged_non_rust_file_skips_the_gate() {
    let dir = unique_fixture_dir();
    write_fixture_package(&dir);
    fs::write(dir.join("README.md"), "# fixture\n").expect("write README.md");

    // Only the README is staged; the misformatted main.rs stays unstaged.
    git_init_and_stage(&dir, &["README.md"]);

    let passed = run_hook(&dir);
    cleanup(&dir);

    assert!(
        passed,
        "hook should skip the gate when no Rust/Cargo files are staged, but it exited non-zero"
    );
}
