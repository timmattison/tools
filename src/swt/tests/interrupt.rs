//! What a signal does to a `swt` that is in the middle of something.
//!
//! `swt create` builds a worktree before it verifies it, and the verification is
//! a full build-and-test that can run for minutes. Whoever ends the run inside
//! that window owes the user a teardown — including a signal, which by default
//! kills the process outright and leaves the worktree, the branch, or a merge
//! lock behind with nobody left to remove them. These are the cases only a
//! subprocess can pin: a directory that survives or does not, a lock file that
//! outlives its owner, and the status a killed run reports.
//!
//! **Every signal here is aimed at the `swt` process itself, never at its
//! process group.** That is deliberate, and it is what makes the assertions
//! deterministic. A group signal also kills the green check `swt` is blocked on,
//! after which `swt`'s ordinary path wakes up, calls the check red and tears the
//! worktree down itself — the same teardown, but racing the signal handling for
//! which of them gets to report the exit status. Signalling `swt` alone leaves
//! the check running, so the run has exactly one way out and the guarantee under
//! test is the only thing that can produce it.
//!
//! Unix only: signals, process groups and `sh` fixtures all are.
#![cfg(unix)]

mod support;

use std::fs::{self, File};
use std::os::raw::c_int;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use support::{git, swt_command, unique, write_swt_check, TestRepo};
use swt::green_check::shell_quote;
use tempfile::TempDir;

/// Ceiling on every wait for a child `swt`, so a wedged run fails the test
/// instead of parking the suite.
const DEADLINE: Duration = Duration::from_secs(60);

/// How often a wait re-checks. Short enough to keep the interval between "the
/// check has started" and the signal small, long enough not to spin a core.
const POLL: Duration = Duration::from_millis(10);

/// How long a check stalls once it has announced itself. Only a floor matters:
/// it has to outlast everything the test does between noticing the announcement
/// and the run ending, so the signal cannot land after the check is over.
const CHECK_HOLD_SECONDS: u32 = 30;

/// How long the shimmed teardown git stalls, holding the teardown open for the
/// second interrupt to land inside it.
const TEARDOWN_HOLD_SECONDS: u32 = 2;

/// Basename of the lock file inside a repository's shared git directory.
const LOCK_FILE: &str = "swt.lock";

/// Status a run interrupted with SIGINT must report: 128 + 2.
const SIGINT_STATUS: i32 = 130;

/// Status a run terminated with SIGTERM must report: 128 + 15.
const SIGTERM_STATUS: i32 = 143;

/// The body of a `.swt-check` that announces itself by creating `marker` and
/// then stalls, so a test can signal `swt` while the check is demonstrably
/// running rather than hoping it timed a sleep correctly.
fn stalling_check(marker: &Path) -> String {
    format!(
        "#!/bin/sh\n: > {}\nsleep {CHECK_HOLD_SECONDS}\n",
        shell_quote(&marker.to_string_lossy())
    )
}

/// The body of a `.swt-check` that stalls on its `nth` invocation and passes
/// every other time, counting the invocations in a file of its own.
///
/// `counter` and `marker` are absolute paths outside both worktrees: the check
/// runs in a different directory each time it is called, and anything it wrote
/// inside a worktree would be dirt the merge guards refuse.
fn stalling_on_nth_check(counter: &Path, marker: &Path, nth: u32) -> String {
    let counter = shell_quote(&counter.to_string_lossy());
    let marker = shell_quote(&marker.to_string_lossy());
    format!(
        "#!/bin/sh\n\
         n=0\n\
         if [ -f {counter} ]; then n=$(cat {counter}); fi\n\
         n=$((n + 1))\n\
         echo \"$n\" > {counter}\n\
         if [ \"$n\" -eq {nth} ]; then : > {marker}; sleep {CHECK_HOLD_SECONDS}; fi\n\
         exit 0\n"
    )
}

/// A private directory for one test's markers, counters and logs.
///
/// Deliberately not [`TestRepo::siblings`]: that is where anything `swt` leaves
/// behind would land, and these tests assert on exactly what is sitting there.
fn scratch() -> TempDir {
    tempfile::Builder::new()
        .prefix("swt-interrupt-")
        .tempdir()
        .expect("scratch temp dir")
}

/// Sorted names of everything sitting beside a fixture repository, so an
/// orphaned worktree cannot hide by being merely un-asserted-about.
fn beside_the_repo(repo: &TestRepo) -> Vec<String> {
    let mut entries: Vec<String> = fs::read_dir(repo.siblings())
        .expect("the fixture's sibling directory should be readable")
        .map(|entry| {
            entry
                .expect("sibling directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    entries.sort();
    entries
}

/// The `git branch --list` pattern matching every branch a `swt create <name>`
/// could have left behind.
fn branch_pattern(name: &str) -> String {
    format!("swt/{name}-*")
}

/// A PATH-shadowing `git` that announces `swt`'s teardown and then holds its
/// first command open.
///
/// The second interrupt under test has to land *inside* teardown, and teardown
/// is two back-to-back git commands that together take tens of milliseconds — so
/// timing a signal into that window with a sleep is a coin flip, and a flaky
/// test for a flaky bug proves nothing. Making the first teardown command its
/// own synchronization point removes the guesswork: it touches a sentinel the
/// test waits on, then stalls before handing over to the real git. Every other
/// git invocation is passed straight through, so nothing else about the run
/// changes.
struct TeardownShim {
    /// Directory to prepend to PATH.
    dir: PathBuf,
    /// File the shim creates the moment teardown's first command runs.
    sentinel: PathBuf,
}

impl TeardownShim {
    /// Materializes the shim inside `scratch`.
    fn new(scratch: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt;

        // Resolved before the shim can shadow it, so the shim can hand over.
        let resolved = std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v git")
            .output()
            .expect("could not look for git");
        assert!(resolved.status.success(), "could not resolve the real git");
        let real_git = String::from_utf8_lossy(&resolved.stdout).trim().to_string();

        let dir = scratch.join(unique("git-shim"));
        fs::create_dir_all(&dir).expect("shim directory");
        let sentinel = dir.join("teardown-started");
        let shim = dir.join("git");
        fs::write(
            &shim,
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"worktree\" ] && [ \"$2\" = \"remove\" ]; then\n\
                 \x20 : > {sentinel}\n\
                 \x20 sleep {TEARDOWN_HOLD_SECONDS}\n\
                 fi\n\
                 exec {real_git} \"$@\"\n",
                sentinel = shell_quote(&sentinel.to_string_lossy()),
                real_git = shell_quote(&real_git),
            ),
        )
        .expect("shim script");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).expect("chmod shim");

        Self { dir, sentinel }
    }
}

/// A `swt` run that can be signalled and then inspected.
///
/// It runs in a process group of its own so the cleanup below can sweep up every
/// child it left — the stalled check outlives the run by design, since these
/// tests deliberately do not signal it.
struct RunningSwt {
    /// The `swt` process, and the leader of its process group.
    child: Child,
    /// File the run's stderr is being written to. A file rather than a pipe: the
    /// run has to be signalled *while* it is producing output, and nobody is
    /// draining a pipe in the meantime.
    log: PathBuf,
    /// Keeps the log alive for as long as the run is inspectable.
    _logs: TempDir,
}

impl RunningSwt {
    /// Spawns `swt` in `cwd`, optionally with `shim` prepended to its PATH.
    fn spawn(cwd: &Path, args: &[&str], shim: Option<&Path>) -> Self {
        let logs = scratch();
        let log = logs.path().join("stderr");
        let stderr = File::create(&log).expect("stderr log");

        let mut command = swt_command(cwd);
        command
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr));
        if let Some(dir) = shim {
            let inherited = std::env::var("PATH").unwrap_or_default();
            command.env("PATH", format!("{}:{inherited}", dir.display()));
        }
        // Its own process group: a signal aimed at the run can then never reach
        // the test runner, and the cleanup in `Drop` can name the whole group.
        command.process_group(0);

        // SAFETY: the closure runs in the forked child between `fork` and
        // `exec`, where only async-signal-safe calls are legal. `signal(2)` is
        // one, and it touches nothing else in the child.
        unsafe {
            command.pre_exec(|| {
                // Every assertion here is stated against the *default*
                // disposition, so each run has to start from it. A shell sets
                // SIGINT and SIGQUIT to ignore for a background job, and a
                // disposition of "ignore" survives `exec` — so a suite run in
                // the background would otherwise hand `swt` a SIGINT it can
                // never feel, and the test would measure the harness instead of
                // the tool.
                for signal in [libc::SIGINT, libc::SIGTERM] {
                    if libc::signal(signal, libc::SIG_DFL) == libc::SIG_ERR {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }

        Self {
            child: command.spawn().expect("spawn swt"),
            log,
            _logs: logs,
        }
    }

    /// The run's pid, which is also its process group id.
    fn pid(&self) -> i32 {
        i32::try_from(self.child.id()).expect("a pid fits in an i32")
    }

    /// Sends `signal` to the `swt` process alone — never to its group, so the
    /// green check it is blocked on keeps running and cannot end the run for it.
    fn signal(&self, signal: c_int) {
        // SAFETY: `kill` is a plain syscall with no memory safety implications;
        // the pid is this test's own child, which has not been reaped yet.
        let sent = unsafe { libc::kill(self.pid(), signal) };
        assert_eq!(sent, 0, "could not signal swt: {}", last_os_error());
    }

    /// Polls until `path` exists, failing fast if the run ends first.
    fn wait_for(&mut self, path: &Path, what: &str) {
        let deadline = Instant::now() + DEADLINE;
        while !path.exists() {
            if let Some(status) = self.child.try_wait().expect("poll swt") {
                panic!(
                    "swt exited ({status:?}) before {what}:\n{}",
                    self.stderr_text()
                );
            }
            assert!(
                Instant::now() < deadline,
                "timed out after {DEADLINE:?} waiting for {what}:\n{}",
                self.stderr_text()
            );
            thread::sleep(POLL);
        }
    }

    /// Waits for the run to end, failing the test rather than parking the suite.
    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll swt") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "swt did not exit within {DEADLINE:?}:\n{}",
                self.stderr_text()
            );
            thread::sleep(POLL);
        }
    }

    /// Everything the run has written to stderr so far, for failure messages.
    fn stderr_text(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for RunningSwt {
    /// Never leaves a stalled check — or a stalled shim — behind, whatever went
    /// wrong above. The whole group goes, because the check `swt` was blocked on
    /// is deliberately not signalled by the tests themselves.
    fn drop(&mut self) {
        // SAFETY: `kill` is a plain syscall with no memory safety implications.
        // A negative pid names the process group this run leads; the group id
        // stays reserved for as long as any member of it is alive, so this
        // cannot reach a group that has since been recycled.
        unsafe { libc::kill(-self.pid(), libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

/// The reason the last libc call failed, for a message worth reading.
fn last_os_error() -> String {
    std::io::Error::last_os_error().to_string()
}

/// Runs a `swt create` whose check stalls, signals it, and asserts that nothing
/// survives and that the run reports `expected_status`.
fn interrupted_create_leaves_nothing_behind(signal: c_int, expected_status: i32) {
    let repo = TestRepo::new();
    let scratch = scratch();
    let started = scratch.path().join("check-started");
    write_swt_check(repo.path(), &stalling_check(&started));
    let name = unique("interrupted");
    let worktree = repo.siblings().join(format!("{name}.swt"));

    let mut run = RunningSwt::spawn(repo.path(), &["create", &name], None);
    run.wait_for(&started, "the green check to start");
    assert!(
        worktree.is_dir(),
        "precondition: create must build the worktree before checking it"
    );

    run.signal(signal);
    let status = run.wait();
    let stderr = run.stderr_text();

    assert_eq!(
        status.code(),
        Some(expected_status),
        "an interrupted run must exit 128 + signal, not die where it stood: {status:?}\n{stderr}"
    );
    assert!(
        !worktree.exists(),
        "signal {signal} left an orphaned worktree at {}:\n{stderr}",
        worktree.display()
    );
    assert!(
        repo.branches(&branch_pattern(&name)).is_empty(),
        "signal {signal} left an orphaned branch:\n{stderr}"
    );
    assert_eq!(
        beside_the_repo(&repo),
        vec!["repo".to_string()],
        "signal {signal} left something beside the repository:\n{stderr}"
    );
}

// The window `swt create` opens by building before it verifies: a user who gives
// up on a long check must not be the one paying for that ordering.
#[test]
fn an_interrupted_green_check_leaves_no_worktree_and_no_branch() {
    interrupted_create_leaves_nothing_behind(libc::SIGINT, SIGINT_STATUS);
}

// The same guarantee for the polite kill a supervisor sends, which a run under
// `timeout` or a harness is far more likely to see than a Ctrl-C.
#[test]
fn a_terminated_green_check_leaves_no_worktree_and_no_branch() {
    interrupted_create_leaves_nothing_behind(libc::SIGTERM, SIGTERM_STATUS);
}

// One interrupt asks `swt` to stop; a second, while it is still stopping, must
// not undo the stopping. Teardown is two git commands, and cut between them it
// leaves the worst possible state: a worktree that survived and a branch that
// cannot be deleted while it does — both orphaned by the very interrupt that
// asked for them to go away. The shim makes "the second signal arrived
// mid-teardown" a fact rather than a hope.
#[test]
fn a_second_interrupt_cannot_truncate_the_teardown_the_first_asked_for() {
    let repo = TestRepo::new();
    let scratch = scratch();
    let started = scratch.path().join("check-started");
    write_swt_check(repo.path(), &stalling_check(&started));
    let shim = TeardownShim::new(scratch.path());
    let name = unique("reinterrupted");
    let worktree = repo.siblings().join(format!("{name}.swt"));

    let mut run = RunningSwt::spawn(repo.path(), &["create", &name], Some(&shim.dir));
    run.wait_for(&started, "the green check to start");
    assert!(
        worktree.is_dir(),
        "precondition: create must build the worktree before checking it"
    );

    run.signal(libc::SIGINT);
    run.wait_for(&shim.sentinel, "teardown to start");
    // The burst a held-down Ctrl-C sends, every one of it landing inside the
    // teardown the first one asked for.
    for _ in 0..3 {
        run.signal(libc::SIGINT);
    }

    let status = run.wait();
    let stderr = run.stderr_text();

    assert_eq!(
        status.code(),
        Some(SIGINT_STATUS),
        "a repeated interrupt changed how the run ended: {status:?}\n{stderr}"
    );
    assert!(
        !worktree.exists(),
        "a second interrupt truncated teardown, orphaning the worktree at {}:\n{stderr}",
        worktree.display()
    );
    assert!(
        repo.branches(&branch_pattern(&name)).is_empty(),
        "a second interrupt truncated teardown, orphaning the branch:\n{stderr}"
    );
}

// The scope of the whole mechanism. `swt` only gets to alter what a signal means
// while it actually owns something that would otherwise be orphaned; a signal
// arriving before it owns anything must kill the process the way it always would
// have. `swt merge` verifies the parent worktree before it creates or locks
// anything, so that check is the one window where the answer is provably
// nothing.
#[test]
fn a_signal_with_nothing_at_risk_kills_swt_the_way_the_default_disposition_would() {
    let repo = TestRepo::new();
    let subagent = repo.add_worktree("bystander");
    let scratch = scratch();
    let started = scratch.path().join("parent-check-started");
    write_swt_check(repo.path(), &stalling_check(&started));
    let lock = repo.path().join(".git").join(LOCK_FILE);

    let mut run = RunningSwt::spawn(
        repo.path(),
        &["merge", &subagent.path.to_string_lossy()],
        None,
    );
    run.wait_for(&started, "the parent green check to start");
    assert!(
        !lock.exists(),
        "precondition: the parent check runs before anything is locked"
    );

    run.signal(libc::SIGINT);
    let status = run.wait();
    let stderr = run.stderr_text();

    assert_eq!(
        status.code(),
        None,
        "swt turned a signal it owned nothing for into an ordinary exit: {status:?}\n{stderr}"
    );
    assert_eq!(
        status.signal(),
        Some(libc::SIGINT),
        "the run should have died by the signal, as it would have without swt: {status:?}\n{stderr}"
    );
}

// The other thing a signal orphans, and the more expensive one: a merge lock
// left behind blocks every later merge in that repository until the staleness
// reap an hour later. The post-rebase re-verification is the one long-running
// step that happens *inside* the locked region, so a check that stalls on
// exactly that invocation puts the signal where it has to land.
#[test]
fn a_lock_held_when_the_signal_arrives_does_not_outlive_the_run() {
    let repo = TestRepo::new();
    let subagent = repo.add_worktree("locked");
    let scratch = scratch();
    let counter = scratch.path().join("check-invocations");
    let started = scratch.path().join("rebase-check-started");
    // Invocation 1 is the parent's check, 2 the subagent's, 3 the
    // re-verification after the rebase — the only one under the lock.
    write_swt_check(repo.path(), &stalling_on_nth_check(&counter, &started, 3));

    // Work on both sides, so the branches have diverged and the merge has to
    // rebase — which is what makes a third check happen at all.
    fs::write(subagent.path.join("subagent.txt"), "subagent work\n").expect("subagent work");
    git(&subagent.path, &["add", "--", "subagent.txt"]);
    git(
        &subagent.path,
        &["commit", "--quiet", "-m", "subagent work"],
    );
    repo.commit_file("parent.txt", "parent work\n");

    let lock = repo.path().join(".git").join(LOCK_FILE);
    let mut run = RunningSwt::spawn(
        repo.path(),
        &["merge", &subagent.path.to_string_lossy()],
        None,
    );
    run.wait_for(&started, "the post-rebase green check to start");
    assert!(
        lock.exists(),
        "precondition: the re-verification must run inside the locked region, \
         so a lock should exist at {}",
        lock.display()
    );

    run.signal(libc::SIGINT);
    let status = run.wait();
    let stderr = run.stderr_text();

    assert_eq!(
        status.code(),
        Some(SIGINT_STATUS),
        "an interrupted merge must exit 128 + signal: {status:?}\n{stderr}"
    );
    assert!(
        !lock.exists(),
        "the merge lock at {} outlived the run that held it, blocking every \
         later merge until the staleness reap:\n{stderr}",
        lock.display()
    );
}
