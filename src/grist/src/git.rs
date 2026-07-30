//! The one way `grist` is allowed to invoke git.
//!
//! Every simulation runs against the developer's *real* repository, so a stray
//! git invocation could rewrite branches they care about. All calls funnel
//! through [`Git`], which pins the configuration that makes simulation
//! non-destructive — most importantly `rebase.updateRefs=false`, which would
//! otherwise move the very branch refs being simulated.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

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
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>, hooks_path: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            hooks_path: hooks_path.into(),
        }
    }

    /// The directory this runner operates in.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Same repository, different working directory.
    #[must_use]
    pub fn at(&self, cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            hooks_path: self.hooks_path.clone(),
        }
    }

    /// Run git, returning the outcome whether or not it succeeded.
    ///
    /// # Errors
    ///
    /// Returns an error only if git could not be spawned at all.
    pub fn try_run(&self, args: &[&str]) -> Result<GitOutput> {
        let output = Command::new("git")
            .args(self.safety_config())
            .args(args)
            .current_dir(&self.cwd)
            // A rebase that stops would otherwise try to open an editor and
            // hang forever on a commit message or a todo list.
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0")
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
            "commit.gpgsign=false",
            "gpg.format=openpgp",
            "user.name=grist",
            "user.email=grist@localhost",
        ]
        .iter()
        .flat_map(|setting| ["-c".to_string(), (*setting).to_string()])
        .chain(["-c".to_string(), format!("core.hooksPath={}", self.hooks_path)])
        .collect()
    }
}
