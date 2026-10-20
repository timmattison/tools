//! The start branch of watch mode.
//!
//! This module holds `StartBranch`. Its own module makes the field private to
//! the rest of `watch`, tests included. The one question a caller can ask is
//! `StartBranch::is_shown_by`.

use std::path::Path;
use std::process::Command;

use crate::render::{Operation, Snapshot};

/// The words of the hint that the first press of Esc puts on the row. The
/// name of the start branch follows them, after one space.
const RETURN_HINT: &str = "Hit Escape again to go back to";

/// The branch that HEAD named when gsw started in watch mode: the start
/// branch.
///
/// **The field is private to this module, and no accessor gives the name out.**
/// The rest of `watch`, its tests included, thus cannot read the name. The one
/// question a caller may ask is [`StartBranch::is_shown_by`], so the rule for
/// "this frame is on the start branch" lives in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StartBranch(String);

impl StartBranch {
    /// Read the start branch from `repo`. A HEAD that names a branch gives
    /// that branch. A HEAD that a rebase detached gives the branch that the
    /// rebase rebases, because a user who starts gsw in a rebase has that
    /// branch to come back to. Any other detached HEAD names no branch, so
    /// the result is `None` and the session has no start branch to leave.
    ///
    /// The rule is [`StartBranch::branch_of`], which [`StartBranch::is_shown_by`]
    /// uses too.
    pub(super) fn read(repo: &gix::Repository) -> Option<Self> {
        // The conflict count is 0 here. It does not change the branch.
        Self::branch_of(
            &crate::repo::branch_name(repo),
            crate::repo::operation_state(repo, 0).as_ref(),
            repo,
        )
        .map(Self)
    }

    /// The branch that a HEAD named `branch` is on, with `operation` in
    /// progress in `repo`.
    ///
    /// A HEAD that names a branch is on that branch. A HEAD that a rebase
    /// detached is on the branch that the rebase rebases. That name comes from
    /// `head-name`, through [`crate::repo::operation_start_of`]. Any other
    /// detached HEAD is on no branch.
    fn branch_of(
        branch: &str,
        operation: Option<&Operation>,
        repo: &gix::Repository,
    ) -> Option<String> {
        if branch != crate::repo::DETACHED_HEAD {
            return Some(branch.to_string());
        }
        match operation {
            Some(operation @ Operation::Rebase { .. }) => {
                crate::repo::operation_start_of(repo, operation).branch
            }
            _ => None,
        }
    }

    /// Whether `snapshot`, which a walk of `repo` collected, shows the start
    /// branch.
    ///
    /// The names of the branches are compared, and never the header labels.
    ///
    /// Git detaches HEAD for the whole of a rebase, so the branch of the
    /// snapshot is then `HEAD`. A rebase of the start branch must still count
    /// as on it, because the `R` key of gsw makes such a rebase. The rule is
    /// [`StartBranch::branch_of`], which reads the rebased branch from
    /// `head-name`. A HEAD that names a branch is compared by that name alone,
    /// even when a rebase directory exists. A walk with no rebase reads
    /// nothing extra. A merge keeps HEAD on its branch and needs no rule.
    pub(super) fn is_shown_by(&self, snapshot: &Snapshot, repo: &gix::Repository) -> bool {
        Self::branch_of(&snapshot.branch, snapshot.operation.as_ref(), repo).as_deref()
            == Some(self.0.as_str())
    }

    /// The line that the first press of Esc puts on the row: it asks for a
    /// second press and names the start branch.
    ///
    /// The name is in the line, and the line is the only way it leaves this
    /// module as text.
    pub(super) fn return_hint(&self) -> String {
        format!("{RETURN_HINT} {}", self.0)
    }

    /// The git command that takes the work tree at `workdir` back to the start
    /// branch: `git switch --no-guess`, with the name as one argument.
    ///
    /// **`--no-guess` is necessary.** Without it, `git switch <name>` guesses
    /// when no local branch has that name and exactly one remote has one. It
    /// makes a new local branch that tracks the remote one. A start branch that
    /// was deleted, with an `origin/<branch>` of the same name, would then give
    /// a branch that is not the start branch, and the work tree would be
    /// "back" on a branch that did not exist when gsw started. With
    /// `--no-guess`, git refuses and says the reference is invalid.
    ///
    /// The command has no `--force`, no `--discard-changes`, and no `--merge`.
    /// A local change that the switch would overwrite stops it, and git says
    /// so.
    pub(super) fn switch_child(&self, workdir: &Path) -> Command {
        let mut command = Command::new("git");
        command
            .args(["switch", "--no-guess"])
            .arg(&self.0)
            .current_dir(workdir);
        command
    }

    /// Whether the work tree that `repo` was opened on is on the start branch.
    ///
    /// The rule is [`StartBranch::read`], so a HEAD that names the branch is on
    /// it, and so is a HEAD that a rebase of the branch detached. Any other
    /// detached HEAD is on no branch.
    pub(super) fn is_checked_out_in(&self, repo: &gix::Repository) -> bool {
        Self::read(repo).as_ref() == Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::push::{run_streamed, PushOutcome};
    use crate::testrepo::{
        diverged_repo, git, git_allowing_failure, git_output, git_stdout, init_repo,
        init_repo_with_worktree,
    };

    /// The hint names the start branch, in the words of the issue, and nothing
    /// more: no age, no punctuation at the end.
    #[test]
    fn the_return_hint_names_the_start_branch() {
        let branch = StartBranch("topic".to_string());
        assert_eq!(branch.return_hint(), "Hit Escape again to go back to topic");
    }

    /// A name with multi-byte characters goes into the line whole.
    #[test]
    fn the_return_hint_keeps_a_multi_byte_name_whole() {
        let branch = StartBranch("日本語-café-🎉".to_string());
        assert_eq!(
            branch.return_hint(),
            "Hit Escape again to go back to 日本語-café-🎉",
        );
    }

    /// Open the repository at `dir` the way the watch does: by path, with no
    /// search upward.
    fn open(dir: &std::path::Path) -> gix::Repository {
        gix::open(dir).expect("open the fixture")
    }

    /// A HEAD on the start branch is on it.
    #[test]
    fn a_work_tree_on_the_start_branch_is_on_it() {
        let dir = init_repo();
        git(dir.path(), &["checkout", "-q", "-b", "topic"]);
        let branch = StartBranch::read(&open(dir.path())).expect("a start branch");
        assert_eq!(branch, StartBranch("topic".to_string()));
        assert!(branch.is_checked_out_in(&open(dir.path())));
    }

    /// A HEAD on another branch is not on the start branch.
    #[test]
    fn a_work_tree_on_another_branch_is_not_on_the_start_branch() {
        let dir = init_repo();
        git(dir.path(), &["checkout", "-q", "-b", "topic"]);
        let branch = StartBranch::read(&open(dir.path())).expect("a start branch");
        git(dir.path(), &["checkout", "-q", "main"]);
        assert!(!branch.is_checked_out_in(&open(dir.path())));
    }

    /// A rebase of the start branch, stopped on a conflict, detaches HEAD. The
    /// work tree is still on the start branch.
    #[test]
    fn a_stopped_rebase_of_the_start_branch_is_on_it() {
        let dir = diverged_repo();
        let p = dir.path();
        let branch = StartBranch::read(&open(p)).expect("a start branch");
        assert_eq!(branch, StartBranch("feature".to_string()));

        git_allowing_failure(p, &["rebase", "main"]);

        let repo = open(p);
        assert_eq!(
            crate::repo::branch_name(&repo),
            crate::repo::DETACHED_HEAD,
            "git detaches HEAD in a rebase",
        );
        assert!(branch.is_checked_out_in(&repo));
    }

    /// A detached HEAD outside a rebase is on no branch.
    #[test]
    fn a_detached_head_outside_a_rebase_is_not_on_the_start_branch() {
        let dir = init_repo();
        git(dir.path(), &["checkout", "-q", "-b", "topic"]);
        let branch = StartBranch::read(&open(dir.path())).expect("a start branch");
        git(dir.path(), &["checkout", "-q", "--detach"]);
        assert!(!branch.is_checked_out_in(&open(dir.path())));
    }

    /// The name of the program in the failure lines of a run of the switch.
    const RUN_NAME: &str = "git switch";

    /// Run the switch child of `branch` in `dir`, through the streamed runner
    /// that the loop uses, and give the outcome.
    fn switched(branch: &StartBranch, dir: &Path) -> PushOutcome {
        run_streamed(branch.switch_child(dir), RUN_NAME, &|_| {})
    }

    /// The branch that HEAD of the work tree at `dir` names.
    fn head_of(dir: &Path) -> String {
        git_stdout(dir, &["symbolic-ref", "--short", "HEAD"])
    }

    /// Whether the local branch `name` exists in the repository at `dir`.
    fn branch_exists(dir: &Path, name: &str) -> bool {
        git_output(
            dir,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ],
        )
        .status
        .success()
    }

    /// Commit `file` with `content` on the branch that is checked out in `dir`.
    fn commit_file(dir: &Path, file: &str, content: &str) {
        std::fs::write(dir.join(file), content).expect("write the file");
        git(dir, &["add", file]);
        git(dir, &["commit", "-q", "-m", &format!("write {file}")]);
    }

    /// A repository whose `topic` branch holds one more commit than `main`,
    /// with HEAD on `main`. The change of `topic` is `a.txt`, which a test can
    /// make a local change collide with.
    fn repo_on_main_with_topic() -> tempfile::TempDir {
        let dir = init_repo();
        git(dir.path(), &["checkout", "-q", "-b", "topic"]);
        commit_file(dir.path(), "a.txt", "from topic\n");
        git(dir.path(), &["checkout", "-q", "main"]);
        dir
    }

    /// The plain case: home is on `main`, `topic` exists, and the switch takes
    /// HEAD to `topic`.
    #[test]
    fn the_switch_takes_a_clean_work_tree_to_the_start_branch() {
        let dir = repo_on_main_with_topic();
        let branch = StartBranch("topic".to_string());

        let outcome = switched(&branch, dir.path());

        assert!(outcome.success, "git said: {:?}", outcome.output);
        assert_eq!(head_of(dir.path()), "topic");
    }

    /// A local change that the switch would overwrite stops the switch. The
    /// run says so in git's words, and HEAD stays. The switch has no force.
    #[test]
    fn the_switch_refuses_when_it_would_overwrite_a_local_change() {
        let dir = repo_on_main_with_topic();
        std::fs::write(dir.path().join("a.txt"), "local edit\n").expect("edit a.txt");
        let branch = StartBranch("topic".to_string());

        let outcome = switched(&branch, dir.path());

        assert!(!outcome.success, "git said: {:?}", outcome.output);
        assert!(
            outcome.output.contains("would be overwritten"),
            "git said: {:?}",
            outcome.output,
        );
        assert_eq!(head_of(dir.path()), "main");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).expect("read a.txt"),
            "local edit\n",
            "the refused switch must keep the local change",
        );
    }

    /// A local change that the switch does not touch is no reason to refuse.
    /// The switch goes through, and the change comes along.
    #[test]
    fn the_switch_carries_a_local_change_that_it_does_not_touch() {
        let dir = init_repo();
        git(dir.path(), &["checkout", "-q", "-b", "topic"]);
        commit_file(dir.path(), "b.txt", "from topic\n");
        git(dir.path(), &["checkout", "-q", "main"]);
        std::fs::write(dir.path().join("a.txt"), "local edit\n").expect("edit a.txt");
        let branch = StartBranch("topic".to_string());

        let outcome = switched(&branch, dir.path());

        assert!(outcome.success, "git said: {:?}", outcome.output);
        assert_eq!(head_of(dir.path()), "topic");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).expect("read a.txt"),
            "local edit\n",
            "the local change must stay",
        );
    }

    /// A merge in progress, with a conflict, stops the switch. HEAD stays. The
    /// test does not pin the words of git, which differ between versions.
    #[test]
    fn the_switch_refuses_in_a_merge_in_progress() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "topic"]);
        commit_file(p, "a.txt", "from topic\n");
        git(p, &["checkout", "-q", "main"]);
        commit_file(p, "a.txt", "from main\n");
        git_allowing_failure(p, &["merge", "topic"]);
        assert!(
            p.join(".git").join("MERGE_HEAD").exists(),
            "the fixture must stop in a merge",
        );
        let branch = StartBranch("topic".to_string());

        let outcome = switched(&branch, p);

        assert!(!outcome.success, "git said: {:?}", outcome.output);
        assert_eq!(head_of(p), "main", "HEAD must not move");
    }

    /// A branch that another worktree holds cannot be checked out here. The
    /// run says so in git's words, and HEAD stays.
    #[test]
    fn the_switch_refuses_a_branch_that_another_worktree_holds() {
        let (repo, linked) = init_repo_with_worktree();
        let branch = StartBranch("linked".to_string());
        assert_eq!(head_of(&linked), "linked", "the fixture holds the branch");

        let outcome = switched(&branch, repo.path());

        assert!(!outcome.success, "git said: {:?}", outcome.output);
        assert!(
            outcome.output.contains("already used by worktree"),
            "git said: {:?}",
            outcome.output,
        );
        assert_eq!(head_of(repo.path()), "main");
    }

    /// **This test proves that the switch takes `--no-guess`.** The start
    /// branch is gone, and a remote branch of the same name exists. A plain
    /// `git switch topic` guesses what the user means, and makes a new local
    /// `topic` that tracks `origin/topic`. That branch is not the start branch.
    /// With `--no-guess`, git refuses, and no branch appears.
    #[test]
    fn the_switch_does_not_make_a_branch_out_of_a_remote_one() {
        let dir = repo_on_main_with_topic();
        let p = dir.path();
        let tip = git_stdout(p, &["rev-parse", "topic"]);
        git(p, &["branch", "-q", "-D", "topic"]);
        git(
            p,
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/none.git",
            ],
        );
        git(p, &["update-ref", "refs/remotes/origin/topic", &tip]);
        assert!(!branch_exists(p, "topic"), "the fixture deletes topic");
        let branch = StartBranch("topic".to_string());

        let outcome = switched(&branch, p);

        assert!(!outcome.success, "git said: {:?}", outcome.output);
        assert!(
            outcome.output.contains("invalid reference"),
            "git said: {:?}",
            outcome.output,
        );
        assert!(
            !branch_exists(p, "topic"),
            "the switch made a branch out of origin/topic",
        );
        assert_eq!(head_of(p), "main");
    }

    /// The tests that read the process that gsw starts, under a hostile
    /// environment. They start this test binary again, as the tests of the
    /// abort child do, and a recording git stands first on the `PATH` of that
    /// child.
    #[cfg(unix)]
    mod hostile {
        use super::*;
        use crate::shell::stub_shell::{
            a_child_with_a_recording_git_passes, recorded_git_calls, shed_git_lines, test_name,
            test_process_can_open_the_terminal, user_intent_lost, user_intent_value, RecordedCall,
            CHILD_RAN, HOSTILE_GIT_ENVIRONMENT, HOSTILE_MARKER, TTY_OPENED, TTY_REFUSED,
        };

        /// The calls of the recording git of this child that are the switch
        /// of `branch`.
        ///
        /// The arguments to look for come from the switch child itself, so the
        /// record and the switch cannot name two different commands.
        fn recorded_switches(branch: &StartBranch) -> Vec<RecordedCall> {
            let arguments: Vec<String> = branch
                .switch_child(Path::new("."))
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect();
            recorded_git_calls()
                .into_iter()
                .filter(|call| call.arguments == arguments)
                .collect()
        }

        /// The switch child sheds the git variables of gsw, keeps those of
        /// the user, and still does its work.
        ///
        /// **This test starts this test binary again, and the hostile
        /// environment goes on that child**, for the reason that the tests of
        /// the abort child give. A `gsw` that a pre-commit hook started holds
        /// `GIT_DIR`. A switch that obeyed it would move HEAD in the
        /// repository of the hook.
        ///
        /// **Two halves.** The run: the switch lands in the fixture although
        /// the child holds a decoy `GIT_DIR`, and nothing appears at a decoy.
        /// The record: the recording git, which the switch child starts by
        /// name, writes down the environment that it got.
        ///
        /// **The armed control comes first.** The child asserts that it holds
        /// each hostile variable and each variable of the user, that no decoy
        /// path exists on this machine, and that the recording git saw the
        /// calls of the fixture.
        #[test]
        fn the_switch_child_sheds_the_git_variables_of_gsw_and_keeps_those_of_the_user() {
            if std::env::var_os(HOSTILE_MARKER).is_none() {
                a_child_with_a_recording_git_passes(&test_name(
                    module_path!(),
                    "the_switch_child_sheds_the_git_variables_of_gsw_and_keeps_those_of_the_user",
                ));
                return;
            }

            for (name, _) in HOSTILE_GIT_ENVIRONMENT {
                assert!(
                    std::env::var_os(name).is_some(),
                    "the child must really hold {name}, or there is nothing here to remove and \
                     the assertions below are measured against nothing",
                );
            }
            for name in gitscratch::USER_INTENT_GIT_ENVIRONMENT {
                assert_eq!(
                    std::env::var(name).ok(),
                    Some(user_intent_value(name)),
                    "the child must really hold {name}, or there is nothing here to keep",
                );
            }
            let nothing_at = |path: &Path| {
                matches!(
                    std::fs::symlink_metadata(path),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound
                )
            };
            let decoys: Vec<&Path> = HOSTILE_GIT_ENVIRONMENT
                .iter()
                .map(|(_, value)| Path::new(*value))
                .filter(|path| path.is_absolute())
                .collect();
            for decoy in &decoys {
                assert!(
                    nothing_at(decoy),
                    "{} must name nothing on this machine, or a write there cannot be seen",
                    decoy.display(),
                );
            }

            let dir = repo_on_main_with_topic();
            assert!(
                !recorded_git_calls().is_empty(),
                "the fixture runs git by name, so the recording git must have recorded it. \
                 Without that, the record of the switch below is empty for a reason that is \
                 not the switch",
            );
            let branch = StartBranch("topic".to_string());

            let outcome = switched(&branch, dir.path());

            assert!(
                outcome.success,
                "the switch did not land in the work tree of the run. A switch that obeyed \
                 GIT_DIR goes to a repository that does not exist, and fails: {:?}",
                outcome.output,
            );
            assert_eq!(head_of(dir.path()), "topic");
            for decoy in &decoys {
                assert!(
                    nothing_at(decoy),
                    "a git child of gsw obeyed a hostile variable and wrote to {}",
                    decoy.display(),
                );
            }

            let switches = recorded_switches(&branch);
            let calls: Vec<String> = recorded_git_calls()
                .iter()
                .map(|call| call.arguments.join(" "))
                .collect();
            assert_eq!(
                switches.len(),
                1,
                "the run must switch once, through the git that the PATH finds: {calls:?}",
            );
            let switch = &switches[0];
            assert_eq!(
                switch.cwd,
                dir.path().canonicalize().expect("resolve the fixture"),
                "the switch must run in the work tree of the run",
            );
            let carried = shed_git_lines(&switch.environment);
            assert!(
                carried.is_empty(),
                "the switch child carried a git variable out of the environment of gsw. Each \
                 of these aims the switch, or configures it, somewhere the user never pointed \
                 it: {carried:?}",
            );
            let lost = user_intent_lost(&switch.environment, None);
            assert!(
                lost.is_empty(),
                "the switch child lost a git variable that the user states on purpose, so git \
                 switches with a configuration that the user did not choose: {lost:?}",
            );

            println!("{CHILD_RAN}");
        }

        /// The switch child has no controlling terminal.
        ///
        /// gsw holds the terminal in raw mode while the switch runs. git
        /// starts hooks for a switch (`post-checkout`), and each of them gets
        /// the terminal of the switch child. A hook that opens `/dev/tty`
        /// paints over the frame of gsw and reads the keys that the event
        /// thread of gsw waits for.
        ///
        /// **The recording git is the probe, and the control is armed
        /// first**, as in the test of the abort child: the test process must
        /// hold a terminal, and the git calls of the fixture, which do not
        /// detach, must open it.
        #[test]
        fn the_switch_child_cannot_open_the_controlling_terminal() {
            if std::env::var_os(HOSTILE_MARKER).is_none() {
                if !test_process_can_open_the_terminal() {
                    eprintln!(
                        "skipped: this test process has no controlling terminal, so /dev/tty \
                         is unopenable for every child regardless - the assertion would hold \
                         vacuously",
                    );
                    return;
                }
                a_child_with_a_recording_git_passes(&test_name(
                    module_path!(),
                    "the_switch_child_cannot_open_the_controlling_terminal",
                ));
                return;
            }

            assert!(
                test_process_can_open_the_terminal(),
                "the child must hold the terminal of this test, or the refusal below holds for \
                 every process",
            );
            let dir = repo_on_main_with_topic();
            let fixture: Vec<String> = recorded_git_calls()
                .into_iter()
                .map(|call| call.terminal)
                .collect();
            assert!(
                !fixture.is_empty() && fixture.iter().all(|terminal| terminal == TTY_OPENED),
                "the fixture does not detach its git, so each of its calls must open the \
                 terminal. Otherwise the probe sees no terminal anywhere: {fixture:?}",
            );
            let branch = StartBranch("topic".to_string());

            let outcome = switched(&branch, dir.path());

            assert!(
                outcome.success,
                "the switch must run, or there is no switch child to ask: {:?}",
                outcome.output,
            );
            let terminals: Vec<String> = recorded_switches(&branch)
                .into_iter()
                .map(|call| call.terminal)
                .collect();
            assert_eq!(
                terminals,
                [TTY_REFUSED],
                "the switch child keeps the controlling terminal, so a hook or a helper that \
                 git starts can paint over the frame of gsw and take the keys gsw is reading",
            );

            println!("{CHILD_RAN}");
        }
    }
}
