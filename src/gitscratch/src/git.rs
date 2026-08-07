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

/// The name every scratch commit is authored and committed under.
const SCRATCH_USER_NAME: &str = "gitscratch";

/// The email every scratch commit is authored and committed under.
const SCRATCH_USER_EMAIL: &str = "gitscratch@localhost";

/// The identity git hands a child through the environment instead of through
/// configuration. git sets all six for every hook it runs, and sets the author
/// trio again for each commit that rebase, cherry-pick, or am replays. An
/// environment variable outranks every config source, `-c` included, so
/// [`Git::try_run`] removes them to keep the pinned identity in force.
const INHERITED_IDENTITY_VARS: [&str; 6] = [
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
];

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
        command
            .args(self.safety_config())
            .args(args)
            .current_dir(&self.cwd)
            // A rebase that stops would otherwise try to open an editor and
            // hang forever on a commit message or a todo list.
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0");

        // Config alone does not settle the identity. Whichever tool drives this
        // crate may itself be running under a git that exported the identity
        // into the environment - every hook gets it, and so does every commit
        // replayed by rebase, cherry-pick, or am - and those variables outrank
        // the `user.name` pinned in safety_config. Left in place they put the
        // developer's own name on scratch commits, which is the single thing
        // the pin exists to prevent. Same leak class as the GIT_DIR scrub the
        // repository's pre-commit hook performs before it runs the test suite.
        for variable in INHERITED_IDENTITY_VARS {
            command.env_remove(variable);
        }

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
            "rebase.autoStash=false",
            "rebase.autosquash=false",
            // Simulated mains are loose commits nothing references yet; an
            // opportunistic gc mid-run could collect one out from under us.
            "gc.auto=0",
            "commit.gpgsign=false",
            "gpg.format=openpgp",
        ]
        .into_iter()
        .map(String::from)
        .chain([
            // The identity belongs to this crate, not to whichever tool is
            // driving it, so every consumer's scratch commits are attributable
            // to the harness that actually made them. These two settle it only
            // in company with the environment scrub in `try_run`, which removes
            // the identity variables that would otherwise outrank them.
            format!("user.name={SCRATCH_USER_NAME}"),
            format!("user.email={SCRATCH_USER_EMAIL}"),
            format!("core.hooksPath={}", self.hooks_path),
        ])
        .flat_map(|setting| ["-c".to_string(), setting])
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Git, INHERITED_IDENTITY_VARS, SCRATCH_USER_EMAIL, SCRATCH_USER_NAME};

    /// The identity variables this process holds, named and valued, for a
    /// failure message. Reports their absence just as plainly: a mismatch with
    /// nothing inherited means something other than the environment outranked
    /// the pin, and that is a different bug worth saying out loud.
    fn inherited_identity() -> String {
        let held: Vec<String> = INHERITED_IDENTITY_VARS
            .iter()
            .filter_map(|name| std::env::var(name).ok().map(|value| format!("{name}={value}")))
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

        let expected = format!("{SCRATCH_USER_NAME} <{SCRATCH_USER_EMAIL}>");
        assert!(
            ident.starts_with(&expected),
            "scratch commits must be authored by the crate, not by a consumer.\n  \
             expected:    {expected}\n  \
             got:         {ident}\n  \
             inherited:   {}\n\
             An identity variable outranks every config source, `-c` included, so \
             Git::try_run removes the six of them before it spawns git. git exports \
             them into every hook it runs, and into every commit that rebase, \
             cherry-pick, or am replays.",
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
}
