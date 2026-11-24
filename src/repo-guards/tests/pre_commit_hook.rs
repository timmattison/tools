//! Integration tests that run the repo's real `.husky/pre-commit` hook against
//! throwaway fixtures and assert on its exit status and its output.
//!
//! Most of these tests cover *existing* correct behavior of the hook, so a TDD
//! red phase is impossible for them — they pass immediately against the current
//! hook. The mutation check that substitutes for red (remove the `cargo fmt`
//! step from the copy of `scripts/rust-checks.sh` that each fixture runs, watch
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

use gitscratch::testing::executable::write_executable;

/// A deliberately misformatted Rust source file. `cargo fmt --check` rejects it
/// because of the run-together braces, doubled spaces, and stray whitespace.
const MISFORMATTED_MAIN_RS: &str = "fn main()    {println!(\"hi\" ) ;}\n";

/// Absolute, canonical path to the real hook under test.
fn hook_path() -> PathBuf {
    let hook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.husky/pre-commit");
    fs::canonicalize(&hook)
        .unwrap_or_else(|e| panic!("cannot canonicalize hook path {}: {e}", hook.display()))
}

/// Absolute, canonical path to the real check script. The hook starts it, and
/// each fixture holds a copy of it.
fn rust_checks_path() -> PathBuf {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/rust-checks.sh");
    fs::canonicalize(&script).unwrap_or_else(|e| {
        panic!(
            "cannot canonicalize rust-checks path {}: {e}",
            script.display()
        )
    })
}

/// Absolute, canonical path to the real helper that the check script sources.
/// Each fixture holds a copy of it next to the copy of the check script.
fn without_git_environment_path() -> PathBuf {
    let helper =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/without-git-environment.sh");
    fs::canonicalize(&helper).unwrap_or_else(|e| {
        panic!(
            "cannot canonicalize without-git-environment path {}: {e}",
            helper.display()
        )
    })
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
///
/// It also writes the real `scripts/rust-checks.sh` into `scripts/`. The hook
/// starts `./scripts/rust-checks.sh` relative to its current directory, which
/// is the fixture. Without the script, the hook stops with exit 127 before any
/// check runs. The script is written with `write_executable` and not with
/// `fs::copy`, because on Linux a file that this process held open for writing
/// gives ETXTBSY when the hook runs it while another test thread forks.
fn write_fixture_package(dir: &Path) {
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .expect("write fixture Cargo.toml");

    fs::create_dir_all(dir.join("src")).expect("create fixture src dir");
    fs::write(dir.join("src/main.rs"), MISFORMATTED_MAIN_RS).expect("write misformatted main.rs");

    write_toolchain_and_check_script(dir);
}

/// Copy the pinned `rust-toolchain.toml` into `dir`, and write the real
/// `scripts/rust-checks.sh` into `dir/scripts/`. It also copies the real
/// `scripts/without-git-environment.sh`, because the check script sources it
/// from its own directory. No process runs the helper, thus a plain copy is
/// safe. Each fixture builder calls it.
fn write_toolchain_and_check_script(dir: &Path) {
    fs::copy(rust_toolchain_path(), dir.join("rust-toolchain.toml"))
        .expect("copy rust-toolchain.toml into fixture");

    fs::create_dir_all(dir.join("scripts")).expect("create fixture scripts dir");
    let script = fs::read(rust_checks_path()).expect("read scripts/rust-checks.sh");
    write_executable(dir.join("scripts/rust-checks.sh"), script)
        .expect("write rust-checks.sh into fixture");
    fs::copy(
        without_git_environment_path(),
        dir.join("scripts/without-git-environment.sh"),
    )
    .expect("copy without-git-environment.sh into fixture");
}

/// The text of a `Cargo.lock` that is stale. It lists the `fixture` package and
/// not the `helper` package that `Cargo.toml` now depends on. Cargo must add
/// `helper` to it, and a run with `--locked` refuses to do that.
const STALE_CARGO_LOCK: &str = "# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = \"fixture\"
version = \"0.1.0\"
";

/// Build a two-crate workspace in `dir` whose files are all correctly formatted
/// and free of lints, and whose `Cargo.lock` is stale. `fixture` depends on
/// `helper` and uses it, so `cargo machete` passes. The lock file does not list
/// `helper`. Then it writes the toolchain file and the check script.
fn write_stale_lock_fixture(dir: &Path) {
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = { path = \"helper\" }\n\n[workspace]\n",
    )
    .expect("write fixture Cargo.toml");
    fs::create_dir_all(dir.join("src")).expect("create fixture src dir");
    fs::write(
        dir.join("src/main.rs"),
        "fn main() {\n    helper::greet();\n}\n",
    )
    .expect("write main.rs");

    fs::create_dir_all(dir.join("helper/src")).expect("create helper src dir");
    fs::write(
        dir.join("helper/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write helper Cargo.toml");
    fs::write(
        dir.join("helper/src/lib.rs"),
        "/// Do nothing.\npub fn greet() {}\n",
    )
    .expect("write helper lib.rs");

    fs::write(dir.join("Cargo.lock"), STALE_CARGO_LOCK).expect("write stale Cargo.lock");

    write_toolchain_and_check_script(dir);
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

/// The directory, inside the fixture, that holds the stub `cargo-machete`.
const MACHETE_STUB_DIR: &str = "stub-bin";

/// The file name that cargo looks for when it runs `cargo machete`.
const MACHETE_BINARY_NAME: &str = "cargo-machete";

/// The text of the stub `cargo-machete`. It does nothing and exits zero.
const MACHETE_STUB_SCRIPT: &str = "#!/bin/sh\nexit 0\n";

/// The variable that names the cargo home.
const CARGO_HOME_VARIABLE: &str = "CARGO_HOME";

/// The cargo home when `CARGO_HOME` is not set, in the home directory.
const DEFAULT_CARGO_HOME: &str = ".cargo";

/// The directory, in the cargo home, where cargo looks for a subcommand.
const CARGO_HOME_BIN: &str = "bin";

/// The cargo home of a hook run, as an absolute path. It obeys the rule of
/// cargo: a `CARGO_HOME` that is set and not empty, else `.cargo` in the home
/// directory. Gives back `None` when neither one is known.
fn cargo_home() -> Option<PathBuf> {
    std::env::var_os(CARGO_HOME_VARIABLE)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(DEFAULT_CARGO_HOME)))
        .and_then(|home| std::path::absolute(home).ok())
}

/// Run the real hook once with `dir` as CWD, shedding the inherited `GIT_`
/// family so the hook operates on the fixture repo rather than this test's
/// repo. It sets `CARGO_TARGET_DIR` to `dir/target`, so the cargo in the fixture
/// never waits for the build-directory lock of the outer `cargo test` run.
///
/// It puts a stub `cargo-machete` that exits 0 first on PATH. The check script
/// installs the real tool into the real `~/.cargo/bin` when the tool is not on
/// PATH. A test must not write global state or need the network.
///
/// When PATH does not hold `$CARGO_HOME/bin`, cargo looks there first for a
/// subcommand, and a cargo that is not the rustup proxy then starts the real
/// tool. Thus it puts `$CARGO_HOME/bin` last on PATH, and it sets `CARGO_HOME`
/// to that same cargo home. Cargo then obeys the PATH order, and the stub is the
/// only `cargo-machete` that a hook run starts. The directory goes last, so the
/// `cargo` that the hook starts stays the first one on PATH.
///
/// `configure` changes the command after that setup and before it starts, so a
/// `PATH` that `configure` sets wins. Gives back the exit status and the printed
/// output of that one run.
fn run_hook_configured(dir: &Path, configure: impl FnOnce(&mut Command)) -> HookRun {
    let mut command = childenv::fixture_command("bash");
    let stub_dir = dir.join(MACHETE_STUB_DIR);
    fs::create_dir_all(&stub_dir).expect("failed to create the stub directory");
    write_executable(stub_dir.join(MACHETE_BINARY_NAME), MACHETE_STUB_SCRIPT)
        .expect("failed to write the stub cargo-machete");
    let mut path_dirs = vec![stub_dir];
    path_dirs.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    if let Some(cargo_home) = cargo_home() {
        path_dirs.push(cargo_home.join(CARGO_HOME_BIN));
        command.env(CARGO_HOME_VARIABLE, cargo_home);
    }
    let path =
        std::env::join_paths(path_dirs).expect("failed to join the stub directory onto PATH");
    command.env("PATH", path);
    command.env("CARGO_TARGET_DIR", dir.join("target"));
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

/// The line `scripts/rust-checks.sh` prints before it runs `cargo fmt`. Together
/// with [`MACHETE_ANNOUNCEMENT`], it proves that the script started and stopped
/// in the fmt step. An exit 127 from a missing script cannot print it.
const FMT_ANNOUNCEMENT: &str = "rust-checks: running cargo fmt --all -- --check";

/// The start of the line `scripts/rust-checks.sh` prints before it runs
/// `cargo machete`, the step after `cargo fmt`. A failed fmt step stops the
/// script before it prints this line.
const MACHETE_ANNOUNCEMENT: &str = "rust-checks: running cargo machete";

/// Best-effort cleanup; the per-process+nanos path is the real isolation, so a
/// failed removal cannot collide with another run.
fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Mutation test for the gate: a staged, misformatted `.rs` file must make the
/// hook exit non-zero. The hook must fail in the fmt step of the check script:
/// the output holds the fmt announcement and not the machete announcement. A
/// status alone is not enough, because a missing script also fails the hook.
/// If this ever passes with a broken hook, the guard is dead — that is exactly
/// what we are protecting against.
#[test]
fn staged_misformatted_rust_file_fails_the_gate() {
    let dir = unique_fixture_dir();
    write_fixture_package(&dir);
    git_init_and_stage(&dir, &["src/main.rs", "Cargo.toml", "rust-toolchain.toml"]);

    let HookRun { passed, output } = run_hook(&dir);
    cleanup(&dir);

    assert!(
        !passed,
        "hook should reject a staged misformatted .rs file, but it exited zero: {output}"
    );
    assert!(
        output.contains(FMT_ANNOUNCEMENT),
        "the check script must reach its cargo fmt step: {output}"
    );
    assert!(
        !output.contains(MACHETE_ANNOUNCEMENT),
        "the check script must stop in its cargo fmt step: {output}"
    );
}

/// A minimal `rustfmt.toml` whose only job is to be a staged file that matches
/// the gate's trigger regex. The pinned style edition mirrors the workspace.
const RUSTFMT_TOML: &str = "style_edition = \"2021\"\n";

/// Staging only one file that fires the gate on its own, and is not Rust
/// source, must still trigger the gate. These files are the style config, the
/// toolchain config, and the check script itself. A config commit is the exact
/// scenario most likely to introduce formatting/lint drift. A change to the
/// list of checks must run the changed list. We prove the gate fires by leaving a
/// misformatted `src/main.rs` unstaged on disk: if the gate runs, `cargo fmt
/// --check` sees the dirty tree and the hook exits non-zero; if the gate is
/// skipped, the hook exits zero and this test fails. The output must also show
/// that the check script reached its fmt step and stopped there, so a missing
/// script (exit 127) cannot satisfy the test.
#[test]
fn staged_trigger_file_alone_triggers_the_gate() {
    // `rust-toolchain.toml` and `scripts/rust-checks.sh` are already written by
    // `write_fixture_package`, so they only need staging. `rustfmt.toml` is
    // written here first.
    for trigger_file in [
        "rustfmt.toml",
        "rust-toolchain.toml",
        "scripts/rust-checks.sh",
    ] {
        let dir = unique_fixture_dir();
        write_fixture_package(&dir);
        fs::write(dir.join("rustfmt.toml"), RUSTFMT_TOML).expect("write rustfmt.toml");

        // Stage ONLY the trigger file. The misformatted main.rs stays unstaged,
        // so the gate must fire on the trigger file alone to catch it.
        git_init_and_stage(&dir, &[trigger_file]);

        let HookRun { passed, output } = run_hook(&dir);
        cleanup(&dir);

        assert!(
            !passed,
            "hook should run the gate when {trigger_file} is staged, but it exited zero (gate skipped): {output}"
        );
        assert!(
            output.contains(FMT_ANNOUNCEMENT),
            "the check script must reach its cargo fmt step when {trigger_file} is staged: {output}"
        );
        assert!(
            !output.contains(MACHETE_ANNOUNCEMENT),
            "the check script must stop in its cargo fmt step when {trigger_file} is staged: {output}"
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

/// The start of the line `scripts/rust-checks.sh` prints before it runs
/// `cargo clippy`. It is a prefix, so a change of the flags does not break it.
const CLIPPY_ANNOUNCEMENT: &str = "rust-checks: running cargo clippy";

/// The start of the line `scripts/rust-checks.sh` prints before it runs
/// `cargo test`, the step after `cargo clippy`. A failed clippy step stops the
/// script before it prints this line.
const TEST_ANNOUNCEMENT: &str = "rust-checks: running cargo test";

/// The check script must refuse a `Cargo.lock` that does not match the
/// manifests. It must refuse it in the clippy step, and it must not rewrite the
/// file. Without this rule, CI rewrites the lock file in its checkout and
/// passes, and `scripts/install-from-local.sh`, which installs with `--locked`,
/// then fails on `main`.
#[test]
fn stale_lock_file_fails_the_gate() {
    let dir = unique_fixture_dir();
    write_stale_lock_fixture(&dir);
    git_init_and_stage(
        &dir,
        &[
            "Cargo.toml",
            "Cargo.lock",
            "src/main.rs",
            "helper/Cargo.toml",
            "helper/src/lib.rs",
            "rust-toolchain.toml",
        ],
    );

    let lock_before = fs::read(dir.join("Cargo.lock")).expect("read Cargo.lock before the run");
    let HookRun { passed, output } = run_hook(&dir);
    let lock_after = fs::read(dir.join("Cargo.lock")).expect("read Cargo.lock after the run");
    cleanup(&dir);

    assert!(
        !passed,
        "the gate must fail on a stale Cargo.lock, but it exited zero: {output}"
    );
    assert!(
        output.contains(CLIPPY_ANNOUNCEMENT),
        "the check script must reach its cargo clippy step: {output}"
    );
    assert!(
        !output.contains(TEST_ANNOUNCEMENT),
        "the check script must stop in its cargo clippy step: {output}"
    );
    assert_eq!(
        lock_before, lock_after,
        "the gate must not rewrite Cargo.lock"
    );
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
