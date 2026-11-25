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

/// Run `git -C <dir> <args>`, assert it succeeded, and give back its standard
/// output.
fn run_git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout).into_owned()
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

/// What a fake program recorded. Each call appends its environment, then one
/// separator line that holds its arguments.
struct FakeRecording {
    /// The recording file, which lives inside the fixture directory.
    log: PathBuf,
    /// The directory that holds the fake program. It goes first on `PATH`.
    bin_dir: PathBuf,
}

/// Prefix of the separator line that a fake program writes after each call.
const CALL_MARKER: &str = "--- call: ";

/// Make a fake `program` in a new directory. It records its environment and its
/// arguments. It exits 1 when its arguments start with `fails_on`, and else
/// zero.
fn install_fake_program(dir: &Path, program: &str, fails_on: Option<&str>) -> FakeRecording {
    use std::os::unix::fs::PermissionsExt;

    let bin_dir = dir.join("fake-bin");
    fs::create_dir_all(&bin_dir).expect("create fake bin dir");
    let log = dir.join(format!("{program}-recording.txt"));
    let failure = fails_on
        .map(|prefix| format!("case \"$*\" in '{prefix}'*) exit 1 ;; esac\n"))
        .unwrap_or_default();
    let script = format!(
        "#!/bin/sh\nenv >> '{log}'\necho '{CALL_MARKER}'\"$*\" >> '{log}'\n{failure}exit 0\n",
        log = log.display()
    );
    let fake = bin_dir.join(program);
    fs::write(&fake, script).unwrap_or_else(|e| panic!("write fake {program}: {e}"));
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|e| panic!("chmod fake {program}: {e}"));
    FakeRecording { log, bin_dir }
}

/// Make a fake `cargo`. It exits zero for `cargo install` too, so the
/// `cargo-machete` branch of the hook cannot reach the network.
fn install_fake_cargo(dir: &Path) -> FakeRecording {
    install_fake_program(dir, "cargo", None)
}

/// The `PATH` for a child: the fake directory first, the real `PATH` after it.
fn path_with_fake_first(bin_dir: &Path) -> std::ffi::OsString {
    let mut paths = vec![bin_dir.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).expect("join PATH")
}

/// Give `command` the fake program and three `GIT_` variables, after the sweep.
/// One variable is a name that git has never invented.
fn plant_fake_and_git_variables(command: &mut std::process::Command, fake: &FakeRecording) {
    command
        .env("PATH", path_with_fake_first(&fake.bin_dir))
        .env("GIT_CONFIG_PARAMETERS", "'hook.envleakprobe=leaked'")
        .env("GIT_AUTHOR_NAME", "Leaked Author")
        .env("GIT_SOMETHING_GIT_INVENTS_LATER", "leaked");
}

/// The recorded calls, as the arguments of each, and the lines of the
/// recording that start with `GIT_`.
struct RecordedCalls {
    arguments: Vec<String>,
    git_lines: Vec<String>,
}

/// Read what the fake program recorded.
fn read_recording(fake: &FakeRecording) -> RecordedCalls {
    let text = fs::read_to_string(&fake.log).unwrap_or_default();
    let arguments = text
        .lines()
        .filter_map(|line| line.strip_prefix(CALL_MARKER))
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

/// Run the real hook in `dir` with the fake program and the planted variables.
fn run_hook_with_planted_git_variables(dir: &Path, fake: &FakeRecording) -> HookRun {
    run_hook_configured(dir, |command| {
        plant_fake_and_git_variables(command, fake);
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
    plant_fake_and_git_variables(&mut command, &cargo);
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

/// The file of the plugin that the plugin fixtures stage.
const FIXTURE_PLUGIN_FILE: &str = "plugins/fixture/README.md";

/// The marketplace file at the root of the repository.
const MARKETPLACE_FILE: &str = ".claude-plugin/marketplace.json";

/// Write one plugin and the marketplace file into `dir`. The fake `claude`
/// reads neither, so their text does not matter.
fn write_fixture_plugin(dir: &Path) {
    write_files(dir, &[FIXTURE_PLUGIN_FILE, MARKETPLACE_FILE]);
}

/// The `claude` calls of the plugin block for the one fixture plugin, in order.
const EXPECTED_CLAUDE_CALLS: [&str; 3] = [
    "plugin validate .",
    "plugin validate plugins/fixture/",
    "plugin test plugins/fixture/",
];

/// A staged file of a plugin, or the staged marketplace file alone, must run
/// the checks of the claude CLI, and each `claude` command must run with no
/// `GIT_` variable, also a name that git has never invented.
#[test]
fn plugin_block_runs_claude_without_git_variables() {
    for staged in [FIXTURE_PLUGIN_FILE, MARKETPLACE_FILE] {
        let dir = unique_fixture_dir();
        write_fixture_plugin(&dir);
        git_init_and_stage(&dir, &[staged]);
        let claude = install_fake_program(&dir, "claude", None);

        let HookRun { passed, output } = run_hook_with_planted_git_variables(&dir, &claude);
        let calls = read_recording(&claude);
        cleanup(&dir);

        assert!(
            passed,
            "the hook must pass with a fake claude ({staged}): {output}"
        );
        assert_eq!(
            calls.arguments, EXPECTED_CLAUDE_CALLS,
            "staging {staged} must run each check of the plugin block: {output}"
        );
        assert!(
            calls.git_lines.is_empty(),
            "a claude command saw GIT_ variables ({staged}): {:?}",
            calls.git_lines
        );
    }
}

/// With no directory below `plugins/`, the loop of the plugin block must run no
/// claude command. An unmatched glob stays the text `plugins/*/`.
#[test]
fn plugin_block_without_a_plugin_directory_validates_only_the_marketplace() {
    let dir = unique_fixture_dir();
    write_fixture_plugin(&dir);
    fs::remove_dir_all(dir.join("plugins")).expect("remove the plugins directory");
    git_init_and_stage(&dir, &[MARKETPLACE_FILE]);
    let claude = install_fake_program(&dir, "claude", None);

    let HookRun { passed, output } = run_hook_configured(&dir, |command| {
        command.env("PATH", path_with_fake_first(&claude.bin_dir));
    });
    let calls = read_recording(&claude);
    cleanup(&dir);

    assert!(passed, "the hook must pass with a fake claude: {output}");
    assert_eq!(calls.arguments, ["plugin validate ."], "{output}");
}

/// The start of the line that the hook prints when no claude CLI is on `PATH`.
const NO_CLAUDE_REFUSAL: &str = "pre-commit: the claude CLI is not on PATH";

/// Make a directory that holds a link to each of `programs` and nothing else.
/// Each link points at the first match on the inherited `PATH`.
fn directory_of_links(dir: &Path, programs: &[&str]) -> PathBuf {
    let bin_dir = dir.join("links-bin");
    fs::create_dir_all(&bin_dir).expect("create links bin dir");
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    for program in programs {
        let target = std::env::split_paths(&inherited)
            .map(|path| path.join(program))
            .find(|candidate| candidate.is_file())
            .unwrap_or_else(|| panic!("{program} is not on PATH"));
        std::os::unix::fs::symlink(&target, bin_dir.join(program))
            .unwrap_or_else(|e| panic!("link {program} to {}: {e}", target.display()));
    }
    bin_dir
}

/// With no claude CLI on `PATH`, a staged file of a plugin must fail the commit
/// and name the fix. A gate that skips the checks would report a plugin that it
/// never read as clean.
///
/// `PATH` holds only the programs that the hook runs before its check for the
/// CLI, so a claude CLI that is installed on this machine cannot be found.
/// Proven able to fail by mutation: with `exit 0` in place of the `exit 1` of
/// the check, the hook passes and this test fails.
#[test]
fn plugin_block_without_claude_fails_the_commit() {
    let dir = unique_fixture_dir();
    write_fixture_plugin(&dir);
    git_init_and_stage(&dir, &[FIXTURE_PLUGIN_FILE]);
    let bin_dir = directory_of_links(&dir, &["bash", "dirname", "git", "grep"]);

    let HookRun { passed, output } = run_hook_configured(&dir, |command| {
        command.env("PATH", &bin_dir);
    });
    cleanup(&dir);

    assert!(
        !passed,
        "a staged plugin file with no claude CLI must fail the commit: {output}"
    );
    assert!(
        output.contains(NO_CLAUDE_REFUSAL),
        "the refusal must name the missing claude CLI: {output}"
    );
}

/// A failed `claude plugin test` must fail the commit. The fake `claude` passes
/// each validation and fails only the tests, so the hook reaches the last call
/// of the block. Proven able to fail by mutation: with `|| true` after the
/// `claude plugin test` line of the hook, the hook passes and this test fails.
#[test]
fn failed_plugin_test_fails_the_commit() {
    let dir = unique_fixture_dir();
    write_fixture_plugin(&dir);
    git_init_and_stage(&dir, &[FIXTURE_PLUGIN_FILE]);
    let claude = install_fake_program(&dir, "claude", Some("plugin test"));

    let HookRun { passed, output } = run_hook_configured(&dir, |command| {
        command.env("PATH", path_with_fake_first(&claude.bin_dir));
    });
    let calls = read_recording(&claude);
    cleanup(&dir);

    assert_eq!(
        calls.arguments, EXPECTED_CLAUDE_CALLS,
        "the hook must reach claude plugin test: {output}"
    );
    assert!(
        !passed,
        "a failed claude plugin test must fail the commit: {output}"
    );
}

/// The two files of `plugins/ic-inline` that the test
/// `emit_rgba_prints_the_json_that_the_plugin_tests_hold` of `ic` reads.
const IC_INLINE_FILES: [&str; 2] = [
    "plugins/ic-inline/tests/fixture.png",
    "plugins/ic-inline/tests/ic-inline.test.tsx",
];

/// The cargo call that holds the copy of the output of `ic` in the plugin
/// tests to the real output.
const IC_INLINE_CARGO_CALL: &str = "test -p ic --test emit-rgba";

/// Write each of `files` into `dir`. The fake programs read none of them, so
/// their text does not matter.
fn write_files(dir: &Path, files: &[&str]) {
    for file in files {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().expect("a fixture file has a parent"))
            .unwrap_or_else(|e| panic!("create the directory of {file}: {e}"));
        fs::write(&path, "{}\n").unwrap_or_else(|e| panic!("write {file}: {e}"));
    }
}

/// Run the real hook in `dir` with a fake `cargo` and a fake `claude`, under
/// the planted `GIT_` variables. Gives back the run and the calls of the fake
/// `cargo`.
fn run_hook_with_fake_cargo_and_claude(dir: &Path) -> (HookRun, RecordedCalls) {
    let cargo = install_fake_cargo(dir);
    install_fake_program(dir, "claude", None);
    let run = run_hook_with_planted_git_variables(dir, &cargo);
    let calls = read_recording(&cargo);
    (run, calls)
}

/// A staged file of `plugins/ic-inline` alone must run the test target of `ic`
/// that reads the plugin, with no `GIT_` variable. The sandbox of
/// `claude plugin test` runs no process, so the plugin block cannot see that
/// the copy of the output of `ic` is stale. Without this block, a stale copy
/// passes the commit and fails the next commit that stages a Rust file.
#[test]
fn staged_ic_inline_file_alone_runs_the_emit_rgba_tests() {
    for staged in IC_INLINE_FILES {
        let dir = unique_fixture_dir();
        write_files(&dir, &[staged]);
        git_init_and_stage(&dir, &[staged]);

        let (HookRun { passed, output }, calls) = run_hook_with_fake_cargo_and_claude(&dir);
        cleanup(&dir);

        assert!(
            passed,
            "the hook must pass with a fake cargo ({staged}): {output}"
        );
        assert_eq!(
            calls.arguments,
            [IC_INLINE_CARGO_CALL],
            "staging {staged} must run the emit-rgba tests of ic: {output}"
        );
        assert!(
            calls.git_lines.is_empty(),
            "a cargo command saw GIT_ variables ({staged}): {:?}",
            calls.git_lines
        );
    }
}

/// A commit that stages a Rust file and a file of `plugins/ic-inline` runs the
/// workspace tests, which hold the emit-rgba tests of `ic`. The block of the
/// plugin must then not run that target a second time.
#[test]
fn staged_ic_inline_file_with_rust_runs_the_emit_rgba_tests_once() {
    let staged = IC_INLINE_FILES[0];
    let dir = unique_fixture_dir();
    write_fixture_package(&dir);
    write_files(&dir, &[staged]);
    git_init_and_stage(&dir, &["src/main.rs", staged]);

    let (HookRun { passed, output }, calls) = run_hook_with_fake_cargo_and_claude(&dir);
    cleanup(&dir);

    assert!(passed, "the hook must pass with a fake cargo: {output}");
    assert!(
        calls
            .arguments
            .iter()
            .any(|args| args == "test --workspace"),
        "a staged Rust file must run the workspace tests: {:?}",
        calls.arguments
    );
    assert!(
        !calls
            .arguments
            .iter()
            .any(|args| args == IC_INLINE_CARGO_CALL),
        "the workspace tests hold the emit-rgba tests, so they must not run again: {:?}",
        calls.arguments
    );
}

/// The three ways that a commit takes a committed regular file away from its
/// path.
#[derive(Clone, Copy, Debug)]
enum Removal {
    /// `git mv` of the file to this path.
    MoveTo(&'static str),
    /// `git rm` of the file.
    Delete,
    /// A symbolic link in place of the file, staged with `git add`. `git diff`
    /// shows the path with the status `T`, a type change, and the
    /// staging asserts that status.
    ReplaceWithSymlink,
}

/// Each [`Removal`], with `destination` as the path of the move.
fn each_removal(destination: &'static str) -> [Removal; 3] {
    [
        Removal::MoveTo(destination),
        Removal::Delete,
        Removal::ReplaceWithSymlink,
    ]
}

/// The target of the symbolic link of [`Removal::ReplaceWithSymlink`]. Git
/// stages a link whose target does not exist, and no fake program reads it.
const SYMLINK_TARGET: &str = "elsewhere";

/// Commit `committed` in a new repository in `dir`, then stage its removal.
///
/// `git diff --cached` compares the index to `HEAD`, so a removal shows only
/// after a commit holds the file. The identity, the signature setting and the
/// hooks path go on the command line, so the commit reads none of them from the
/// configuration of this machine, and it writes no configuration.
fn commit_then_stage_removal(dir: &Path, committed: &str, removal: Removal) {
    git_init_and_stage(dir, &[committed]);
    run_git(
        dir,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-q",
            "--no-verify",
            "-m",
            "fixture",
        ],
    );
    match removal {
        Removal::MoveTo(destination) => {
            let parent = dir
                .join(destination)
                .parent()
                .expect("a destination has a parent")
                .to_path_buf();
            fs::create_dir_all(&parent)
                .unwrap_or_else(|e| panic!("create the directory of {destination}: {e}"));
            run_git(dir, &["mv", committed, destination]);
        }
        Removal::Delete => {
            run_git(dir, &["rm", "-q", committed]);
        }
        Removal::ReplaceWithSymlink => {
            let path = dir.join(committed);
            fs::remove_file(&path).unwrap_or_else(|e| panic!("remove {committed}: {e}"));
            std::os::unix::fs::symlink(SYMLINK_TARGET, &path)
                .unwrap_or_else(|e| panic!("link {committed} to {SYMLINK_TARGET}: {e}"));
            run_git(dir, &["add", committed]);
            let status = run_git(
                dir,
                &[
                    "diff",
                    "--cached",
                    "--name-status",
                    "--no-renames",
                    "--no-color",
                ],
            );
            assert_eq!(
                status,
                format!("T\t{committed}\n"),
                "a link in place of {committed} must stage as a type change"
            );
        }
    }
}

/// A move of a file of `plugins/ic-inline` to a different path, a deletion of
/// it, and a symbolic link in its place must each run the test target of `ic`
/// that reads the plugin. That target reads the old path as a regular file, so
/// such a commit breaks it. `git diff` finds renames by default, and
/// `--name-only` then prints only the new path, so a block that matches the
/// old path misses the move.
#[test]
fn removed_ic_inline_file_runs_the_emit_rgba_tests() {
    let committed = IC_INLINE_FILES[0];
    for removal in each_removal("plugins/shared/fixture.png") {
        let dir = unique_fixture_dir();
        write_files(&dir, &[committed]);
        commit_then_stage_removal(&dir, committed, removal);

        let (HookRun { passed, output }, calls) = run_hook_with_fake_cargo_and_claude(&dir);
        cleanup(&dir);

        assert!(
            passed,
            "the hook must pass with a fake cargo ({removal:?}): {output}"
        );
        assert_eq!(
            calls.arguments,
            [IC_INLINE_CARGO_CALL],
            "{removal:?} of {committed} must run the emit-rgba tests of ic: {output}"
        );
        assert!(
            calls.git_lines.is_empty(),
            "a cargo command saw GIT_ variables ({removal:?}): {:?}",
            calls.git_lines
        );
    }
}

/// The cargo call of the tool-index block.
const INDEX_GUARD_CARGO_CALL: &str = "test -p repo-guards --test tool_index_coverage";

/// A move of a tool index to a different path, a deletion of it, and a
/// symbolic link in its place must each run the tool-index guard. Each one
/// takes the index away from the root, which is the fault that the guard
/// catches.
#[test]
fn removed_tool_index_runs_the_index_guard() {
    for removal in each_removal("docs/README.md") {
        let dir = unique_fixture_dir();
        fs::write(dir.join("README.md"), "# fixture\n").expect("write README.md");
        commit_then_stage_removal(&dir, "README.md", removal);
        let cargo = install_fake_cargo(&dir);

        let HookRun { passed, output } = run_hook_with_planted_git_variables(&dir, &cargo);
        let calls = read_recording(&cargo);
        cleanup(&dir);

        assert!(
            passed,
            "the hook must pass with a fake cargo ({removal:?}): {output}"
        );
        assert_eq!(
            calls.arguments,
            [INDEX_GUARD_CARGO_CALL],
            "{removal:?} of README.md must run the tool-index guard: {output}"
        );
    }
}

/// A file of the fixture plugin that no fixture stages. It keeps the directory
/// of the plugin after a removal of [`FIXTURE_PLUGIN_FILE`], so the loop of the
/// plugin block still finds the plugin.
const FIXTURE_PLUGIN_MANIFEST: &str = "plugins/fixture/.claude-plugin/plugin.json";

/// A move of a file of a plugin out of `plugins/`, a deletion of it, and a
/// symbolic link in its place must each run the checks of the claude CLI. Each
/// one takes the file away from the plugin, and that can break the plugin as
/// surely as an edit can.
#[test]
fn removed_plugin_file_runs_the_plugin_checks() {
    for removal in each_removal("docs/README.md") {
        let dir = unique_fixture_dir();
        write_fixture_plugin(&dir);
        write_files(&dir, &[FIXTURE_PLUGIN_MANIFEST]);
        commit_then_stage_removal(&dir, FIXTURE_PLUGIN_FILE, removal);
        let claude = install_fake_program(&dir, "claude", None);

        let HookRun { passed, output } = run_hook_with_planted_git_variables(&dir, &claude);
        let calls = read_recording(&claude);
        cleanup(&dir);

        assert!(
            passed,
            "the hook must pass with a fake claude ({removal:?}): {output}"
        );
        assert_eq!(
            calls.arguments, EXPECTED_CLAUDE_CALLS,
            "{removal:?} of {FIXTURE_PLUGIN_FILE} must run each check of the plugin block: {output}"
        );
    }
}

/// The files of msgs that the msgs fixtures commit: one of the frontend, and
/// one of the scripts. The msgs block reads both folders.
const MSGS_FILES: [&str; 2] = ["src/msgs/frontend/src/a.ts", "src/msgs/scripts/a.ts"];

/// The `pnpm` calls of the msgs block, in order. Each one names the frontend
/// of msgs in this repository, through the folder of the hook.
fn expected_msgs_pnpm_calls() -> [String; 2] {
    let frontend = format!(
        "{}/../src/msgs/frontend",
        hook_path()
            .parent()
            .expect("the hook has a parent")
            .display()
    );
    [
        format!("--dir {frontend} install --frozen-lockfile --prefer-offline"),
        format!("--dir {frontend} run check"),
    ]
}

/// A move of a file of the msgs frontend or of the msgs scripts out of its
/// folder, a deletion of it, and a symbolic link in its place must each run the
/// type check and the tests of msgs, with no `GIT_` variable. Each one takes the
/// file away from the code that imports it, and that breaks the type check as
/// an edit can. `git diff` finds renames by default, and `--name-only` then
/// prints only the new path, so a block that matches the old path misses the
/// move.
#[test]
fn removed_msgs_file_runs_the_msgs_checks() {
    for committed in MSGS_FILES {
        for removal in each_removal("src/msgs/other/a.ts") {
            let dir = unique_fixture_dir();
            write_files(&dir, &[committed]);
            commit_then_stage_removal(&dir, committed, removal);
            let pnpm = install_fake_program(&dir, "pnpm", None);

            let HookRun { passed, output } = run_hook_with_planted_git_variables(&dir, &pnpm);
            let calls = read_recording(&pnpm);
            cleanup(&dir);

            assert!(
                passed,
                "the hook must pass with a fake pnpm ({removal:?} of {committed}): {output}"
            );
            assert_eq!(
                calls.arguments,
                expected_msgs_pnpm_calls(),
                "{removal:?} of {committed} must run the checks of msgs: {output}"
            );
            assert!(
                calls.git_lines.is_empty(),
                "a pnpm command saw GIT_ variables ({removal:?} of {committed}): {:?}",
                calls.git_lines
            );
        }
    }
}
