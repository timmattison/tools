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
///
/// Public because the danger is not this crate's alone. Anything in this
/// repository that spawns git - a tool that adds a worktree, a test that builds
/// a throwaway repository - is broken the same way by the same environment, and
/// the list of what to shed is worth keeping in one reusable place rather than
/// copied into each of them to drift.
///
/// That is an offer, not a guarantee. Most of the repository's git spawns still
/// build their own command and inherit whatever this process was handed, and
/// nothing - no lint, no type, no guard - obliges them to call this. Immunity
/// holds where this is called and nowhere else, so anyone who wants it
/// repository-wide has to enforce it first.
///
/// ```no_run
/// let mut command = std::process::Command::new("git");
/// gitscratch::shed_inherited_git_environment(&mut command);
/// ```
pub fn shed_inherited_git_environment(command: &mut Command) {
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

    /// The one git invocation this crate makes, before anyone decides how to
    /// read its output.
    ///
    /// Every guard the crate has lives here — the inherited environment shed,
    /// the safety configuration pinned, the editors pinned off — so a second way
    /// of reading git's answer cannot be a second, weaker way of asking the
    /// question.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        shed_inherited_git_environment(&mut command);
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
        command
    }

    /// Run git, returning the outcome whether or not it succeeded.
    ///
    /// # Errors
    ///
    /// Returns an error only if git could not be spawned at all.
    pub fn try_run(&self, args: &[&str]) -> Result<GitOutput> {
        let output = self
            .command(args)
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
    /// Not for a list of paths — use [`Git::paths`]. Git escapes a path on its
    /// way out of a line-oriented listing and the trimming here finishes the
    /// job, so a name can come back spelled differently from the file it names.
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

    /// Run git and return the paths it listed, as the developer spelled them.
    ///
    /// [`Git::lines`] cannot be used for a path, because git's line-oriented
    /// output is not a faithful rendering of one. A name with a byte outside
    /// printable ASCII comes back C-quoted - `café.txt` as `"caf\303\251.txt"` -
    /// and a name with a leading or trailing space comes back intact only to
    /// lose it to trimming. Neither loss announces itself, and a path read out of
    /// one invocation is usually fed straight back into the next as a pathspec,
    /// which git does not dequote: the mangled spelling matches nothing, and
    /// matching nothing is indistinguishable from there being nothing to match.
    ///
    /// So the paths are asked for NUL-delimited instead, which is git's own
    /// answer to this and turns the escaping off entirely. `-z` goes immediately
    /// after the subcommand rather than at the end, because a command that
    /// carries a pathspec ends in `-- <paths>` and everything after `--` is a
    /// path, not an option.
    ///
    /// # Errors
    ///
    /// Returns an error if `args` is empty, if git could not be spawned, if it
    /// exited non-zero, or if a path it printed is not valid UTF-8. That last one
    /// is deliberately fatal: replacing an undecodable byte would substitute
    /// U+FFFD and hand back a name that matches nothing, which is the very
    /// silence this method exists to remove.
    pub fn paths(&self, args: &[&str]) -> Result<Vec<String>> {
        let (subcommand, rest) = args
            .split_first()
            .context("cannot ask git for paths without a subcommand")?;
        let mut asked = Vec::with_capacity(args.len() + 1);
        asked.push(*subcommand);
        asked.push("-z");
        asked.extend_from_slice(rest);

        let output = self
            .command(&asked)
            .output()
            .with_context(|| format!("failed to run git {}", asked.join(" ")))?;

        anyhow::ensure!(
            output.status.success(),
            "git {} failed:\n{}\n{}",
            asked.join(" "),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );

        // Raw and untrimmed on purpose: trimming stdout as a whole would eat the
        // leading space of the first path, which is one of the two spellings this
        // method exists to preserve. Git terminates every path with a NUL, so the
        // split always ends in an empty remainder.
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| {
                String::from_utf8(path.to_vec()).with_context(|| {
                    format!(
                        "git {} listed a path that is not valid UTF-8: {}",
                        asked.join(" "),
                        String::from_utf8_lossy(path)
                    )
                })
            })
            .collect()
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

    /// Configuration that keeps a simulation from touching anything real, plus
    /// the one main option that belongs beside it.
    ///
    /// Configuration alone is not the whole guard: git resolves the environment
    /// first, so [`Git::command`] also pins the identity as environment and
    /// strips everything that would redirect git elsewhere.
    fn safety_config(&self) -> Vec<String> {
        let mut arguments: Vec<String> = [
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
        .collect();

        // Paths read out of one invocation are fed straight back into the next
        // as pathspecs, and a pathspec is not a path: a leading `:` is pathspec
        // magic, and `*`, `?` and `[` are wildcards. Without this a file
        // genuinely called `star*.txt` matches `starOTHER.txt` too, so a probe
        // asking whether *this* path's content is in the new base quietly
        // answers about some other file's. A main option rather than a `-c`
        // pair, so it belongs here with them, ahead of the subcommand.
        //
        // Over-matching like that is the mild half. It can only add to the set
        // of paths a probe finds missing, and a bigger set only ever buys a
        // refusal nobody needed. The half worth the guard is `:/foo.txt`, a
        // `foo.txt` in a directory named `:`: read as magic its `:/` means from
        // the top of the working tree, so it answers about the root `foo.txt`
        // instead, and if that one is unchanged the diff comes back empty. An
        // empty diff is a commit that adds nothing to the new base, which is a
        // `rebase --skip`, which is the work gone and a cost of zero reported
        // for a branch that was never replayed. Pinned by tests/halts.rs.
        arguments.push("--literal-pathspecs".to_string());

        arguments
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::Git;
    use crate::testing::TestRepo;

    /// Exactly the invocation `stopped_commit_is_already_in_head` makes to find
    /// out which paths a halted commit touched, with the commit left off the
    /// end. Spelled once so the tests below pin the call the replay actually
    /// depends on rather than a plausible-looking neighbour of it.
    const TOUCHED_PATHS: [&str; 5] = ["diff-tree", "--no-commit-id", "--name-only", "-r", "--root"];

    /// A path git cannot spell as UTF-8 has to stop the replay, not be repaired
    /// into one that matches nothing.
    ///
    /// This is the one loss [`Git::paths`] cannot undo. The quoting and the
    /// trimming it exists to defeat are both reversible — ask git for NUL
    /// delimiters and the original bytes come back — but a byte that is not
    /// valid UTF-8 has no `String` to come back *as*. Decoding it lossily, the
    /// way [`Git::try_run`] decodes git's output everywhere else, substitutes
    /// U+FFFD and yields a name no file has. That name goes straight back into
    /// the next invocation as a pathspec, matches nothing, and leaves the
    /// `missing` set empty — which is what "the new base already has this
    /// commit's work" looks like, so the commit is skipped and the work is
    /// gone. Every other test in this suite would still pass, because no other
    /// fixture holds a name git has to refuse.
    ///
    /// So the guard is pinned here, at [`Git::paths`], rather than end-to-end
    /// through a sealed-object-store replay. Such a replay would need the
    /// undecodable name in a working tree, and on macOS no working tree can
    /// hold one: APFS rejects the name with `EILSEQ` before git is involved.
    /// [`TestRepo::commit_file_named_by_bytes`] builds the commit in the object
    /// database instead, which is portable and is also exactly the repository a
    /// developer on this machine gets by cloning one written on a filesystem
    /// that does permit the name.
    ///
    /// The ordinary commit is asserted first, and deliberately: without it a
    /// fixture that failed before reaching the decode — a bad argument list, a
    /// commit id that resolves to nothing — would produce an error too, and the
    /// test would pass for a reason that has nothing to do with the guard.
    #[test]
    fn refuses_a_path_that_is_not_valid_utf_8_rather_than_replacing_the_byte() {
        let repo = TestRepo::init();
        repo.commit_file("ordinary.txt", "ordinary work\n", "ordinary work");
        let git = Git::new(repo.path(), "");

        let mut ordinary = TOUCHED_PATHS.to_vec();
        ordinary.push("HEAD");
        assert_eq!(
            git.paths(&ordinary)
                .expect("list the paths an ordinary commit touched"),
            ["ordinary.txt"],
            "the negative case below only means anything if this invocation reaches the decode \
             at all"
        );

        // `café.txt` as a latin-1 filesystem spells it: one 0xe9 byte where
        // UTF-8 needs two. Invalid, not merely non-ASCII - a non-ASCII name
        // that happens to be valid UTF-8 decodes fine and proves nothing here.
        let undecodable = repo.commit_file_named_by_bytes(
            b"caf\xe9.txt",
            "the branch's work\n",
            "a latin-1 name",
        );
        let mut listed = TOUCHED_PATHS.to_vec();
        listed.push(&undecodable);

        let error = git.paths(&listed).expect_err(
            "a name git cannot spell as UTF-8 must stop the replay; decoding it lossily hands \
             back a U+FFFD name that matches nothing, and a pathspec matching nothing is how a \
             commit gets skipped and its work lost",
        );
        assert!(
            format!("{error:#}").contains("listed a path that is not valid UTF-8"),
            "the refusal has to name what went wrong, since the developer's next move is to look \
             at the path git could not hand over: {error:#}"
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
