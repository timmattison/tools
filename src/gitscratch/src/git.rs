//! The one way a tool in this repository is allowed to invoke git.
//!
//! Every simulation runs against the developer's *real* repository, so a stray
//! git invocation could rewrite branches they care about. All calls funnel
//! through [`Git`], which pins the configuration that makes simulation
//! non-destructive — most importantly `rebase.updateRefs=false`, which would
//! otherwise move the very branch refs being simulated.

use std::path::PathBuf;
use std::process::{Command, Output};

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

    /// Spawn git and hand back its output untouched.
    ///
    /// The single place a git process is actually created, so the safety
    /// configuration and the environment overrides cannot be reached around by
    /// anything above. Private because raw output is a footgun in the one way
    /// this crate cares about: everything public either trims it deliberately
    /// ([`Git::try_run`], [`Git::run`]) or deliberately does not
    /// ([`Git::nul_separated`]), and which of those a caller wants is not a
    /// choice worth re-making per call site.
    fn output(&self, args: &[&str]) -> Result<Output> {
        Command::new("git")
            .args(self.safety_config())
            .args(args)
            .current_dir(&self.cwd)
            // A rebase that stops would otherwise try to open an editor and
            // hang forever on a commit message or a todo list.
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .with_context(|| format!("failed to run git {}", args.join(" ")))
    }

    /// Run git, returning the outcome whether or not it succeeded.
    ///
    /// Both streams come back trimmed, which is what a caller reporting them to
    /// a human wants and what every caller of this method does with them. A
    /// caller reading *paths* wants the opposite and must use
    /// [`Git::nul_separated`].
    ///
    /// # Errors
    ///
    /// Returns an error only if git could not be spawned at all.
    pub fn try_run(&self, args: &[&str]) -> Result<GitOutput> {
        let output = self.output(args)?;

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

    /// Run git with `-z` and return stdout split on NUL, byte for byte.
    ///
    /// The only way to read a list of paths out of git, and the only reader
    /// this type offers, because the line-oriented alternative it replaced could
    /// not be made correct. `-z` is the single output mode in which git prints a
    /// path exactly as it is stored: no C-quoting, no octal escaping, and no
    /// ambiguity about where one path ends, since NUL is the one byte a path
    /// cannot contain. That last part is why nothing here trims. A path may
    /// legitimately begin or end with a space - or with U+3000, which Rust's
    /// Unicode-aware `str::trim` eats just as readily - and a separator that
    /// cannot occur inside a path means there is nothing to trim *for*.
    ///
    /// `-z` goes in straight after the subcommand rather than on the end, so an
    /// argument list that finishes with `--` and a pathspec still gets it as a
    /// flag rather than as a path.
    ///
    /// Empty fields are dropped. Git terminates rather than separates, so the
    /// last NUL always leaves one; no path is ever the empty string, so nothing
    /// real is lost with it.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned or exited non-zero.
    pub fn nul_separated(&self, args: &[&str]) -> Result<Vec<String>> {
        let mut with_nul = args.to_vec();
        with_nul.insert(args.len().min(1), "-z");

        let output = self.output(&with_nul)?;

        anyhow::ensure!(
            output.status.success(),
            "git {} failed:\n{}\n{}",
            with_nul.join(" "),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );

        Ok(String::from_utf8_lossy(&output.stdout)
            .split('\0')
            .filter(|field| !field.is_empty())
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

    /// Configuration every git call is pinned to: the settings that keep a
    /// simulation from touching anything real, and the one that keeps git's
    /// answers readable coming back.
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
            // The identity belongs to this crate, not to whichever tool is
            // driving it, so every consumer's scratch commits are attributable
            // to the harness that actually made them.
            "user.name=gitscratch",
            "user.email=gitscratch@localhost",
            // Git's default is to C-quote and octal-escape any path outside
            // ASCII, so `日本語.txt` comes back from `diff --name-only` as
            // `"\346\227\245\346\234\254\350\252\236.txt"`. That breaks a
            // caller twice: it reports a name nobody typed, and the escaped
            // string names no file on disk, so anything that then opens the
            // path quietly falls back to whatever it does for a file it cannot
            // read - in this crate, flooring a conflicted file at one hunk and
            // undercounting the work.
            //
            // This is the belt, not the braces. `quotePath` governs exactly one
            // class - bytes at or above 0x80 - and git's `quote_c_style` quotes
            // a double quote, a backslash and any control character no matter
            // what it is set to. So `back\slash.txt` and `quo"te.txt` come back
            // quoted and escaped with this pinned, and are just as unopenable,
            // and cost just as many uncounted hunks, as a Japanese name would
            // be without it. A path list therefore cannot be read off git's
            // lines under any setting, which is why the only reader this type
            // offers is [`Git::nul_separated`]: `-z` turns quoting off outright
            // and separates on the one byte a path cannot contain.
            //
            // Kept anyway, because it costs one `-c` and it narrows what a
            // future call site can do wrong. Anything reading a path back
            // through `run` rather than `nul_separated` is a bug, but with this
            // pinned it is a bug that survives the common case instead of
            // mangling every non-ASCII name in the repository. Pinning it on
            // the single door every git call goes through is what makes that
            // free.
            "core.quotePath=false",
        ]
        .iter()
        .flat_map(|setting| ["-c".to_string(), (*setting).to_string()])
        .chain([
            "-c".to_string(),
            format!("core.hooksPath={}", self.hooks_path),
        ])
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Git;
    use crate::testing::TestRepo;

    /// `core.quotePath=false` needs its own test now that it protects nothing a
    /// caller can otherwise observe.
    ///
    /// It used to be pinned indirectly, by `tests/conflicts.rs` asserting the
    /// answer a non-ASCII conflicted path produces. That stopped being a test of
    /// this setting the moment [`Git::nul_separated`] became the only path
    /// reader: `-z` output is unquoted whatever `quotePath` says, so removing
    /// the pin would leave every one of those tests green. A guard nothing can
    /// fail is a guard that quietly stops working, so this asserts it against
    /// the surface it still covers — [`Git::run`], the reader a future call site
    /// would reach for by mistake, where an escaped name would be silent.
    ///
    /// `diff --cached --name-only` rather than a conflict, because the escaping
    /// is a property of how git prints a path and needs no conflict to show it.
    #[test]
    fn a_non_ascii_path_read_back_through_run_is_not_octal_escaped() {
        let repo = TestRepo::init();
        repo.write_file("日本語.txt", "staged\n");
        repo.git(&["add", "日本語.txt"]);

        let staged = Git::new(repo.path(), "")
            .run(&["diff", "--cached", "--name-only"])
            .expect("list the staged path");

        assert_eq!(
            staged, "日本語.txt",
            "git must report the path as it is stored, not C-quoted and \
             octal-escaped"
        );
    }

    /// `git var GIT_AUTHOR_IDENT` reports exactly the identity git would stamp
    /// on a commit, so it proves what [`Git::safety_config`] actually pins
    /// without having to build a repository and commit into it. It resolves
    /// outside a repository too, which is why `temp_dir` is only ever a cwd
    /// here — nothing is created in it, so concurrent test runs cannot collide.
    #[test]
    fn commits_under_the_crate_s_own_identity_not_a_consuming_tool_s() {
        let git = Git::new(std::env::temp_dir(), "");

        let ident = git
            .run(&["var", "GIT_AUTHOR_IDENT"])
            .expect("git var GIT_AUTHOR_IDENT");

        assert!(
            ident.starts_with("gitscratch <gitscratch@localhost>"),
            "scratch commits should be authored by the crate, not a consumer: {ident}"
        );
    }
}
