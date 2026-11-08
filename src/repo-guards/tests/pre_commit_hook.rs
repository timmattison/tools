//! Integration tests that run the repo's real `.husky/pre-commit` hook against
//! throwaway fixtures and assert on its exit status.
//!
//! Most of these tests cover *existing* correct behavior of the hook, so a TDD
//! red phase is impossible for them — they pass immediately against the current
//! hook. The mutation check that substitutes for red (neuter the hook, watch
//! `staged_misformatted_rust_file_fails_the_gate` fail, restore, watch it pass)
//! is documented in the commit body, not committed as code. The tests of the
//! `GIT_` variables are different: each one failed first, in a red commit.
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
    // Shed the whole inherited `GIT_` family so the fixture repo is the one git
    // operates on, even when this runs from inside the repo's own hook. The
    // rule is the prefix and never a list of names: `GIT_DIR` overrides `-C`,
    // and so does a variable nobody here thought to name.
    let mut command = childenv::fixture_command("git");

    let output = command
        .arg("-C")
        .arg(dir)
        .args(args)
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

/// Both halves of one run of the hook. A test reads the half it needs, or both
/// halves, from the single run that produced them.
struct HookRun {
    /// True when the hook exited zero.
    passed: bool,
    /// What the hook printed: its standard output, and then its standard error.
    ///
    /// The status alone cannot tell which block ran. A fixture repo is not this
    /// workspace, so a block naming a package of this workspace fails there for
    /// a reason that has nothing to do with the trigger under test. Each block
    /// announces itself before it runs, so the announcement is what proves the
    /// trigger fired.
    output: String,
}

/// Run the real hook once with `dir` as CWD, shedding the inherited `GIT_`
/// family so the hook operates on the fixture repo rather than this test's
/// repo. `configure` changes the command before it starts. Gives back the exit
/// status and the printed output of that one run.
fn run_hook_configured(dir: &Path, configure: impl FnOnce(&mut Command)) -> HookRun {
    let mut command = childenv::fixture_command("bash");
    configure(&mut command);

    let completed = command
        .arg(hook_path())
        .current_dir(dir)
        .output()
        .expect("failed to spawn pre-commit hook");
    HookRun {
        passed: completed.status.success(),
        output: format!(
            "{}{}",
            String::from_utf8_lossy(&completed.stdout),
            String::from_utf8_lossy(&completed.stderr)
        ),
    }
}

/// Run the real hook once with `dir` as CWD and no change to its command.
fn run_hook(dir: &Path) -> HookRun {
    run_hook_configured(dir, |_| {})
}

/// The line the hook prints before it runs the tool-index guard.
const INDEX_GUARD_ANNOUNCEMENT: &str =
    "pre-commit: running cargo test -p repo-guards --test tool_index_coverage";

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

    let HookRun { passed, .. } = run_hook(&dir);
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

        let HookRun { passed, .. } = run_hook(&dir);
        cleanup(&dir);

        assert!(
            !passed,
            "hook should run the gate when {config_file} is staged, but it exited zero (gate skipped)"
        );
    }
}

/// When only a file that no trigger names is staged, every gate is skipped — so
/// the hook exits zero even though a misformatted `src/main.rs` sits on disk
/// (unstaged). This proves the trigger, not the formatter, is what runs: if the
/// gate fired anyway it would catch the file and fail.
///
/// The staged file is `NOTES.md` and not `README.md`. `README.md` is one of the
/// two tool indexes, and a third block names those directly, so a fixture
/// staging one of them no longer skips everything. See
/// [`staged_tool_index_alone_triggers_the_index_guard`].
#[test]
fn staged_unnamed_file_skips_every_gate() {
    let dir = unique_fixture_dir();
    write_fixture_package(&dir);
    fs::write(dir.join("NOTES.md"), "# fixture\n").expect("write NOTES.md");

    // Only the notes are staged; the misformatted main.rs stays unstaged.
    git_init_and_stage(&dir, &["NOTES.md"]);

    let HookRun { passed, output } = run_hook(&dir);
    cleanup(&dir);

    assert!(
        passed,
        "hook should skip every gate when no named file is staged, but it exited non-zero: {output}"
    );
    assert!(
        !output.contains(INDEX_GUARD_ANNOUNCEMENT),
        "an unrelated Markdown file must not run the tool-index guard: {output}"
    );
}

/// Staging either tool index alone must run the tool-index guard.
///
/// This is the one direction the guard cannot otherwise reach. Adding a crate
/// touches a `Cargo.toml`, which fires the Rust gate and runs the whole suite;
/// REMOVING a tool's entry from an index touches Markdown alone, and before the
/// third block existed such a commit ran no gate at all.
///
/// The assertion reads the announcement rather than the exit status. A fixture
/// repo holds no `repo-guards` package, so `cargo test -p repo-guards` fails
/// there whatever the trigger did, and a status assertion would pass for that
/// reason instead of for the trigger. Proven able to fail by mutation: against
/// the hook as it stood before the third block, both index files produce no
/// announcement and this test fails.
#[test]
fn staged_tool_index_alone_triggers_the_index_guard() {
    for index in ["README.md", "TLDR.md"] {
        let dir = unique_fixture_dir();
        write_fixture_package(&dir);
        fs::write(dir.join(index), "# fixture\n").expect("write the index");

        // Only the index is staged; the misformatted main.rs stays unstaged, so
        // the Rust gate cannot be what fires.
        git_init_and_stage(&dir, &[index]);

        let HookRun { output, .. } = run_hook(&dir);
        cleanup(&dir);

        assert!(
            output.contains(INDEX_GUARD_ANNOUNCEMENT),
            "staging {index} must run the tool-index guard: {output}"
        );
    }
}

/// What a fake `cargo` recorded. Each call appends its environment, then one
/// separator line that holds its arguments.
struct CargoRecording {
    /// The recording file, which lives inside the fake directory.
    log: PathBuf,
    /// The directory that holds the fake `cargo`. It goes first on `PATH`.
    bin_dir: PathBuf,
}

/// Prefix of the separator line that a fake `cargo` writes after each call.
const CARGO_CALL_MARKER: &str = "--- cargo call: ";

/// Make a fake `cargo` in a new directory. It records its environment and its
/// arguments, then exits zero. It exits zero for `cargo install` too, so the
/// `cargo-machete` branch of the hook cannot reach the network.
fn install_fake_cargo(dir: &Path) -> CargoRecording {
    use std::os::unix::fs::PermissionsExt;

    let bin_dir = dir.join("fake-bin");
    fs::create_dir_all(&bin_dir).expect("create fake bin dir");
    let log = dir.join("cargo-recording.txt");
    let script = format!(
        "#!/bin/sh\nenv >> '{log}'\necho '{CARGO_CALL_MARKER}'\"$*\" >> '{log}'\nexit 0\n",
        log = log.display()
    );
    let cargo = bin_dir.join("cargo");
    fs::write(&cargo, script).expect("write fake cargo");
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).expect("chmod fake cargo");
    CargoRecording { log, bin_dir }
}

/// The `PATH` for a child: the fake directory first, the real `PATH` after it.
fn path_with_fake_first(bin_dir: &Path) -> std::ffi::OsString {
    let mut paths = vec![bin_dir.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).expect("join PATH")
}

/// Give `command` the fake `cargo` and three `GIT_` variables, after the sweep.
/// One variable is a name that git has never invented.
fn plant_fake_cargo_and_git_variables(command: &mut std::process::Command, cargo: &CargoRecording) {
    command
        .env("PATH", path_with_fake_first(&cargo.bin_dir))
        .env("GIT_CONFIG_PARAMETERS", "'hook.envleakprobe=leaked'")
        .env("GIT_AUTHOR_NAME", "Leaked Author")
        .env("GIT_SOMETHING_GIT_INVENTS_LATER", "leaked");
}

/// The recorded cargo calls, as the arguments of each, and the lines of the
/// recording that start with `GIT_`.
struct RecordedCalls {
    arguments: Vec<String>,
    git_lines: Vec<String>,
}

/// Read what the fake `cargo` recorded.
fn read_recording(cargo: &CargoRecording) -> RecordedCalls {
    let text = fs::read_to_string(&cargo.log).unwrap_or_default();
    let arguments = text
        .lines()
        .filter_map(|line| line.strip_prefix(CARGO_CALL_MARKER))
        .map(str::to_owned)
        .collect();
    let git_lines = text
        .lines()
        .filter(|line| line.starts_with("GIT_"))
        .map(str::to_owned)
        .collect();
    RecordedCalls {
        arguments,
        git_lines,
    }
}

/// Run the real hook in `dir` with the fake `cargo` and the planted variables.
fn run_hook_with_planted_git_variables(dir: &Path, cargo: &CargoRecording) -> HookRun {
    run_hook_configured(dir, |command| {
        plant_fake_cargo_and_git_variables(command, cargo);
    })
}

/// Assert that the fake ran for each word in `expected` and saw no `GIT_` line.
fn assert_clean_recording(calls: &RecordedCalls, expected: &[&str], context: &str) {
    for word in expected {
        assert!(
            calls
                .arguments
                .iter()
                .any(|args| args.split_whitespace().next() == Some(*word)),
            "the fake cargo never ran `{word}` ({context}): {:?}",
            calls.arguments
        );
    }
    assert!(
        calls.git_lines.is_empty(),
        "a cargo command saw GIT_ variables ({context}): {:?}",
        calls.git_lines
    );
}

/// Every cargo command of the Rust block must run with no `GIT_` variable, also
/// a name that git has never invented.
#[test]
fn rust_block_runs_every_cargo_command_without_git_variables() {
    let dir = unique_fixture_dir();
    write_fixture_package(&dir);
    git_init_and_stage(&dir, &["src/main.rs"]);
    let cargo = install_fake_cargo(&dir);

    let HookRun { passed, output } = run_hook_with_planted_git_variables(&dir, &cargo);
    let calls = read_recording(&cargo);
    cleanup(&dir);

    assert!(passed, "the hook must pass with a fake cargo: {output}");
    assert_clean_recording(&calls, &["fmt", "clippy", "test"], "rust block");
}

/// The tool-index block must run its cargo command with no `GIT_` variable.
#[test]
fn tool_index_block_runs_cargo_without_git_variables() {
    let dir = unique_fixture_dir();
    write_fixture_package(&dir);
    fs::write(dir.join("README.md"), "# fixture\n").expect("write README.md");
    git_init_and_stage(&dir, &["README.md"]);
    let cargo = install_fake_cargo(&dir);

    let HookRun { passed, output } = run_hook_with_planted_git_variables(&dir, &cargo);
    let calls = read_recording(&cargo);
    cleanup(&dir);

    assert!(passed, "the hook must pass with a fake cargo: {output}");
    assert_clean_recording(&calls, &["test"], "tool-index block");
}

/// `test.sh` must run `cargo test` with no `GIT_` variable.
#[test]
fn test_sh_runs_cargo_without_git_variables() {
    let dir = unique_fixture_dir();
    let cargo = install_fake_cargo(&dir);
    let script = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test.sh"))
        .expect("canonicalize test.sh");

    let mut command = childenv::fixture_command("bash");
    plant_fake_cargo_and_git_variables(&mut command, &cargo);
    let completed = command
        .arg(&script)
        .current_dir(&dir)
        .output()
        .expect("failed to spawn test.sh");
    let calls = read_recording(&cargo);
    cleanup(&dir);

    assert!(
        completed.status.success(),
        "test.sh must pass with a fake cargo: {}",
        String::from_utf8_lossy(&completed.stderr)
    );
    assert_clean_recording(&calls, &["test"], "test.sh");
}
