//! The one way a tool in this repository is allowed to invoke git.
//!
//! Every simulation runs against the developer's *real* repository, so a stray
//! git invocation could rewrite branches they care about. All calls funnel
//! through [`Git`], which pins the configuration that makes simulation
//! non-destructive — most importantly `rebase.updateRefs=false`, which would
//! otherwise move the very branch refs being simulated.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};

/// Who scratch commits are attributed to. Spelled once and used both as
/// configuration and as environment, because those are two ways of saying the
/// same thing and git resolves them in that order - so they must not drift.
const HARNESS_NAME: &str = "gitscratch";
const HARNESS_EMAIL: &str = "gitscratch@localhost";

/// Environment that would aim git at a repository other than the one the runner
/// is rooted in, and is therefore stripped from every invocation.
///
/// This is not a hypothetical set. A tool built on this crate can be invoked
/// from inside a git hook, and git hands its hooks `GIT_INDEX_FILE` - often
/// *relative*, so it silently re-anchors on the runner's own working directory -
/// along with `GIT_DIR` and `GIT_WORK_TREE` for several hooks. Inheriting any of
/// them would point the replay at the developer's real repository and index,
/// which is precisely what a scratch worktree exists to avoid. The rest are here
/// for the same reason: each one redirects some part of where git reads or
/// writes, and none of them can mean anything useful to a throwaway replay.
const REDIRECTING_ENVIRONMENT: [&str; 9] = [
    "GIT_DIR",
    "GIT_COMMON_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_CEILING_DIRECTORIES",
];

/// Who a hook says the commit it is running for belongs to, and when. Git reads
/// all six in preference to `-c` *and* to `git config`, so leaving them in place
/// silently re-attributes anything this crate commits - and a caller that set an
/// identity of its own would find it ignored.
const INHERITED_ATTRIBUTION: [&str; 6] = [
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
];

/// Detach `command` from whatever git environment this process inherited.
///
/// Every git invocation this crate makes goes through here, the runner's and the
/// test fixtures' alike: a fixture that inherits a redirected index cannot even
/// build the repository the runner is supposed to replay, so both need the same
/// immunity and neither should be describing the danger in its own words.
pub(crate) fn shed_inherited_environment(command: &mut Command) {
    for variable in REDIRECTING_ENVIRONMENT
        .into_iter()
        .chain(INHERITED_ATTRIBUTION)
    {
        command.env_remove(variable);
    }
}

/// The outcome of one git invocation.
pub struct GitOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// A git command runner pinned to one working directory.
pub struct Git {
    cwd: PathBuf,
    hooks_path: String,
}

impl Git {
    /// Run git in `cwd`, with hooks redirected to the empty `hooks_path`.
    ///
    /// Crate-private on purpose. A caller outside this crate could otherwise
    /// build a runner rooted in the developer's real repository, with a
    /// `hooks_path` that redirects nothing — an empty one still resolves hook
    /// lookups, relative to `cwd`. Both guards are established by
    /// [`Scratch::create`](crate::Scratch::create), so it stays the only way in.
    #[must_use]
    pub(crate) fn new(cwd: impl Into<PathBuf>, hooks_path: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            hooks_path: hooks_path.into(),
        }
    }

    /// Run git, returning the outcome whether or not it succeeded.
    ///
    /// # Errors
    ///
    /// Returns an error only if git could not be spawned at all.
    pub fn try_run(&self, args: &[&str]) -> Result<GitOutput> {
        let mut command = Command::new("git");
        shed_inherited_environment(&mut command);
        command
            .args(self.safety_config())
            .args(args)
            .current_dir(&self.cwd)
            // A rebase that stops would otherwise try to open an editor and
            // hang forever on a commit message or a todo list.
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0")
            // Belt to the configuration's braces. Shedding the inherited
            // attribution above already leaves `-c user.name` to decide, but
            // saying it twice means the identity survives either guard being
            // edited away, and git resolves the environment first.
            .env("GIT_AUTHOR_NAME", HARNESS_NAME)
            .env("GIT_AUTHOR_EMAIL", HARNESS_EMAIL)
            .env("GIT_COMMITTER_NAME", HARNESS_NAME)
            .env("GIT_COMMITTER_EMAIL", HARNESS_EMAIL);

        let output = command
            .output()
            .with_context(|| format!("failed to run git {}", args.join(" ")))?;

        Ok(GitOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }

    /// Run git and fail if it does, returning trimmed stdout.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned or exited non-zero.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let output = self.try_run(args)?;

        anyhow::ensure!(
            output.success,
            "git {} failed:\n{}\n{}",
            args.join(" "),
            output.stdout,
            output.stderr
        );

        Ok(output.stdout)
    }

    /// Run git and return stdout split into non-empty lines.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned or exited non-zero.
    pub fn lines(&self, args: &[&str]) -> Result<Vec<String>> {
        Ok(self
            .run(args)?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect())
    }

    /// Resolve a revision to a full commit id.
    ///
    /// # Errors
    ///
    /// Returns an error if the revision does not name a commit.
    pub fn rev_parse(&self, revision: &str) -> Result<String> {
        self.run(&["rev-parse", &format!("{revision}^{{commit}}")])
            .with_context(|| format!("could not resolve '{revision}' to a commit"))
    }

    /// Configuration that keeps a simulation from touching anything real.
    ///
    /// Configuration alone is not the whole guard: git resolves the environment
    /// first, so [`Git::try_run`] also pins the identity as environment and
    /// strips everything that would redirect git elsewhere.
    fn safety_config(&self) -> Vec<String> {
        [
            // Recording resolutions from a simulated conflict would poison the
            // shared rr-cache and silently pre-resolve the developer's real
            // merges later.
            "rerere.enabled=false",
            "rerere.autoupdate=false",
            // Without this, rebasing a detached HEAD still rewrites every branch
            // ref pointing into the replayed range - including the branch being
            // simulated. Proven by tests/safety.rs: with the setting enabled and
            // this override removed, a dry run destroys the developer's branch.
            "rebase.updateRefs=false",
            // The backend is pinned because `--update-refs` is a merge-backend
            // feature that the apply backend ignores outright. Left unpinned,
            // the entry directly above is unfalsifiable on a developer who
            // prefers the apply backend - it could be deleted and nothing on
            // that machine would notice, because the backend already silences
            // what it overrides. The path a halted rebase lives at moves with
            // the backend too, from `rebase-merge` to `rebase-apply`, so a
            // consumer inspecting an interrupted replay would be reading a
            // different repository on a different machine.
            "rebase.backend=merge",
            "rebase.autoStash=false",
            "rebase.autosquash=false",
            // Simulated mains are loose commits nothing references yet; an
            // opportunistic gc mid-run could collect one out from under us.
            "gc.auto=0",
            "commit.gpgsign=false",
            "gpg.format=openpgp",
        ]
        .iter()
        .map(|setting| (*setting).to_string())
        // The identity belongs to this crate, not to whichever tool is driving
        // it, so every consumer's scratch commits are attributable to the
        // harness that actually made them.
        .chain([
            format!("user.name={HARNESS_NAME}"),
            format!("user.email={HARNESS_EMAIL}"),
            format!("core.hooksPath={}", self.hooks_path),
        ])
        .flat_map(|setting| ["-c".to_string(), setting])
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::{Git, HARNESS_EMAIL, HARNESS_NAME};

    /// The identity git hands a child through the environment instead of
    /// through configuration. git sets all six for every hook it runs, and sets
    /// the author trio again for each commit that rebase, cherry-pick, or am
    /// replays. Named here rather than beside the runner because the runner no
    /// longer removes them as a set: it pins the four name and email variables
    /// to the harness by hand and drops the two dates. This list is what a
    /// failure has to report, which is a different job.
    const INHERITED_IDENTITY_VARS: [&str; 6] = [
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_AUTHOR_DATE",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
        "GIT_COMMITTER_DATE",
    ];

    /// The identity variables this process holds, named and valued, for a
    /// failure message. Reports their absence just as plainly: a mismatch with
    /// nothing inherited means something other than the environment outranked
    /// the pin, and that is a different bug worth saying out loud.
    fn inherited_identity() -> String {
        let held: Vec<String> = INHERITED_IDENTITY_VARS
            .iter()
            .filter_map(|name| {
                std::env::var(name)
                    .ok()
                    .map(|value| format!("{name}={value}"))
            })
            .collect();

        if held.is_empty() {
            "nothing (so the pin lost to something other than the environment)".to_string()
        } else {
            held.join(", ")
        }
    }

    /// Assert that git stamps this crate's own identity, and name the inherited
    /// variables when it does not.
    ///
    /// The variables are the whole diagnosis. A bare identity mismatch reads
    /// like a flake, and it surfaces at the worst possible moment - inside a
    /// developer's pre-commit hook, on a commit that has nothing to do with
    /// this crate - so the message has to carry the cause with it.
    ///
    /// `git var GIT_AUTHOR_IDENT` reports exactly the identity git would stamp
    /// on a commit, so it proves what [`Git::safety_config`] and
    /// [`Git::try_run`] actually pin without having to build a repository and
    /// commit into it. It resolves outside a repository too, which is why
    /// `temp_dir` is only ever a cwd here — nothing is created in it, so
    /// concurrent test runs cannot collide.
    fn assert_scratch_identity() {
        let git = Git::new(std::env::temp_dir(), "");

        let ident = git
            .run(&["var", "GIT_AUTHOR_IDENT"])
            .expect("git var GIT_AUTHOR_IDENT");

        let expected = format!("{HARNESS_NAME} <{HARNESS_EMAIL}>");
        assert!(
            ident.starts_with(&expected),
            "scratch commits must be authored by the crate, not by a consumer.\n  \
             expected:    {expected}\n  \
             got:         {ident}\n  \
             inherited:   {}\n\
             An identity variable outranks every config source, `-c` included, so \
             Git::try_run pins the four name and email variables to the harness and \
             drops the two dates before it spawns git. git exports them into every \
             hook it runs, and into every commit that rebase, cherry-pick, or am \
             replays.",
            inherited_identity()
        );
    }

    /// The identity holds in an environment that carries nothing of its own,
    /// which is how the suite runs from a shell.
    #[test]
    fn commits_under_the_crate_s_own_identity_not_a_consuming_tool_s() {
        assert_scratch_identity();
    }

    /// Marks the re-executed child half of
    /// [`the_pinned_identity_survives_a_hook_environment`].
    const CHILD_MARKER: &str = "GITSCRATCH_HOOK_ENVIRONMENT_CHILD";

    /// libtest's exact filter for the one test the child half runs.
    const HOOK_TEST_PATH: &str = "git::tests::the_pinned_identity_survives_a_hook_environment";

    /// The identity variables git exports into every hook it runs, carrying
    /// values that stand in for a developer's own git identity.
    const HOOK_ENVIRONMENT: [(&str, &str); 4] = [
        ("GIT_AUTHOR_NAME", "Consuming Tool"),
        ("GIT_AUTHOR_EMAIL", "consumer@example.invalid"),
        ("GIT_COMMITTER_NAME", "Consuming Tool"),
        ("GIT_COMMITTER_EMAIL", "consumer@example.invalid"),
    ];

    /// A consumer invoked from a git hook inherits `GIT_AUTHOR_NAME` and its
    /// siblings, and those variables outrank the `-c user.name` that
    /// [`Git::safety_config`] pins. This test reproduces that environment, so
    /// the suite catches the leak on its own instead of a developer's own
    /// pre-commit run catching it on an unrelated commit.
    ///
    /// The environment belongs to a re-executed child of this test binary
    /// rather than to this process. `std::env::set_var` would leak into every
    /// other test in the binary, and a child process keeps concurrent runs of
    /// this suite isolated from each other.
    #[test]
    fn the_pinned_identity_survives_a_hook_environment() {
        if std::env::var_os(CHILD_MARKER).is_some() {
            assert_scratch_identity();
            return;
        }

        let mut child = std::process::Command::new(
            std::env::current_exe().expect("path of the running test binary"),
        );
        child
            .args([HOOK_TEST_PATH, "--exact", "--nocapture"])
            .env(CHILD_MARKER, "1");
        for (name, value) in HOOK_ENVIRONMENT {
            child.env(name, value);
        }

        let output = child.output().expect("re-run this test binary");

        assert!(
            output.status.success(),
            "the pinned identity did not survive a hook environment:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Every pin in [`Git::safety_config`] is only as good as the environment it
    /// runs in, because git's environment beats `-c`. That is not a hypothetical:
    /// a consumer invoked from inside a git hook is handed `GIT_AUTHOR_NAME`,
    /// `GIT_AUTHOR_EMAIL` and `GIT_AUTHOR_DATE` naming the developer, plus
    /// `GIT_INDEX_FILE` — *relative*, so it re-anchors on whatever directory the
    /// runner happens to be in — and other hooks add `GIT_DIR` and
    /// `GIT_WORK_TREE`. So an inherited environment can both sign the harness's
    /// commits with the developer's name and aim the whole replay at the
    /// repository they were committing to, which is the one thing this crate
    /// exists to keep it away from.
    ///
    /// Both halves are asserted together, in one test, so this binary's
    /// environment is only ever mutated in one place; the mutation is what the
    /// guard is being asked to survive, and after the guard exists it cannot
    /// reach the sibling test above either.
    #[test]
    fn ignores_an_inherited_git_environment_naming_another_identity_or_repository() {
        // Stands in for the developer's real repository - the place a leaked
        // environment would redirect the replay to.
        let elsewhere = TempDir::new().expect("create the stand-in for a real repository");
        let elsewhere_git_dir = elsewhere.path().join("their-repo.git");
        let elsewhere_index = elsewhere.path().join("their-index");

        std::env::set_var("GIT_AUTHOR_NAME", "A Developer");
        std::env::set_var("GIT_AUTHOR_EMAIL", "developer@example.com");
        std::env::set_var("GIT_COMMITTER_NAME", "A Developer");
        std::env::set_var("GIT_COMMITTER_EMAIL", "developer@example.com");
        std::env::set_var("GIT_DIR", &elsewhere_git_dir);
        std::env::set_var("GIT_WORK_TREE", elsewhere.path());
        std::env::set_var("GIT_INDEX_FILE", &elsewhere_index);

        let here = TempDir::new().expect("create the scratch stand-in");
        let git = Git::new(here.path(), "");
        git.run(&["init", "-q", "-b", "main"])
            .expect("initialise the repository the runner is rooted in");

        for variable in ["GIT_AUTHOR_IDENT", "GIT_COMMITTER_IDENT"] {
            let ident = git
                .run(&["var", variable])
                .expect("read the identity git would stamp");
            assert!(
                ident.starts_with("gitscratch <gitscratch@localhost>"),
                "an inherited environment must not put a developer's name on a scratch \
                 commit, but {variable} is {ident}"
            );
        }

        // Canonicalised on both sides: macOS resolves the temporary directory's
        // /var to /private/var, so the raw paths would never compare equal.
        let expected = std::fs::canonicalize(here.path()).expect("canonicalise the scratch path");
        let git_dir = git
            .run(&["rev-parse", "--absolute-git-dir"])
            .expect("ask git which repository it is operating on");
        assert!(
            std::fs::canonicalize(&git_dir)
                .expect("canonicalise git's answer")
                .starts_with(&expected),
            "the runner must operate on the repository it is rooted in ({}), not the one an \
             inherited GIT_DIR names ({git_dir})",
            expected.display()
        );

        let index = git
            .run(&["rev-parse", "--git-path", "index"])
            .expect("ask git which index it would write");
        assert!(
            !index.contains("their-index"),
            "an inherited GIT_INDEX_FILE must not become the index a replay stages into: {index}"
        );
    }
}
