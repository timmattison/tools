//! The one way a tool in this repository is allowed to invoke git.
//!
//! Every simulation runs against the developer's *real* repository, so a stray
//! git invocation could rewrite branches they care about. All calls funnel
//! through [`Git`], which pins the configuration that makes simulation
//! non-destructive — most importantly `rebase.updateRefs=false`, which would
//! otherwise move the very branch refs being simulated.
//!
//! The other half of "against the right repository" is the *environment*, which
//! [`NoInheritedRepository`] strips, because a git invocation obeys it before it
//! obeys the directory it was pointed at.

use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{Context, Result};

/// The name every scratch commit is authored and committed under.
const SCRATCH_USER_NAME: &str = "gitscratch";

/// The email every scratch commit is authored and committed under.
const SCRATCH_USER_EMAIL: &str = "gitscratch@localhost";

/// The identity git hands a child through the environment instead of through
/// configuration. git sets all six for every hook it runs, and sets the author
/// trio again for each commit that rebase, cherry-pick, or am replays. An
/// environment variable outranks every config source, `-c` included, so
/// `Git::output` removes them to keep the pinned identity in force.
const INHERITED_IDENTITY_VARS: [&str; 6] = [
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
];

/// Every environment variable that answers "which repository?" before the
/// working directory gets a say.
///
/// Git exports the first four into every hook it runs, so anything a hook
/// spawns inherits them — and `.husky/pre-commit` in this repository spawns
/// `cargo test`. A tool run this way looks like it is working on the directory
/// it was handed and is in fact working on the hook's repository.
///
/// The list is one constant rather than a `.env_remove` chain per call site
/// because the sites cannot be allowed to drift: a spawn that scrubs three of
/// them is a spawn with a hole in it, and holes of this shape are silent.
///
/// | Variable | Leak |
/// | --- | --- |
/// | `GIT_DIR` | Exported to every hook. Names the repository outright, so `git init` re-initialises it and every read and write goes there. |
/// | `GIT_WORK_TREE` | Travels with `GIT_DIR`. Moves the *files* git compares against, so pathspecs resolve somewhere else entirely. |
/// | `GIT_INDEX_FILE` | Exported to `pre-commit` and friends. The subtler half: discovery still finds the right repository, so a run looks fine while `git add` stages phantom entries into the hook's index. |
/// | `GIT_PREFIX` | Exported to every hook. Names the subdirectory the hook was invoked from, so relative pathspecs resolve against the wrong directory. |
/// | `GIT_COMMON_DIR` | Not exported by hooks, but honoured whenever it is set — and it is what worktree-manipulating scripts export. Scrubbing `GIT_DIR` without it leaves the same door open by its other name: refs and config still come from the repository it points at. |
/// | `GIT_OBJECT_DIRECTORY` | Set by `receive-pack` for `pre-receive`/`update` hooks, which run with the push quarantined. Objects written under it are discarded when the push is rejected, so a replay's commits evaporate for no visible reason. |
/// | `GIT_ALTERNATE_OBJECT_DIRECTORIES` | Set alongside the above by the same quarantine. Read-only contamination rather than a write, but it is what lets a scratch repository resolve objects it does not have — so a test that asserts an object is absent passes for the wrong reason. |
///
/// Two near-misses are deliberately *not* here. `GIT_NAMESPACE` can redirect a
/// ref write, but only within the repository already selected, and nothing
/// short of `git http-backend` sets it. `GIT_CEILING_DIRECTORIES` can only stop
/// discovery, never redirect it — it makes a run fail, which is loud, rather
/// than succeed against the wrong repository, which is not.
pub const REPOSITORY_LOCATION_VARS: [&str; 7] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_PREFIX",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// Spawn a command that takes its repository from its working directory alone.
///
/// Public, and an extension trait rather than a private helper, because the
/// commands that need it are not all git: a consumer's test suite spawning its
/// own binary inherits exactly the same environment, and a second copy of
/// [`REPOSITORY_LOCATION_VARS`] living in a test file is the drift this exists
/// to prevent.
pub trait NoInheritedRepository {
    /// Remove every variable in [`REPOSITORY_LOCATION_VARS`] from the
    /// environment this command will be spawned with.
    ///
    /// A no-op in normal use — nothing sets these outside a hook — which is
    /// precisely why it has to be unconditional. The one run where it matters
    /// is the one nobody is watching.
    fn without_inherited_repository(&mut self) -> &mut Self;
}

impl NoInheritedRepository for Command {
    fn without_inherited_repository(&mut self) -> &mut Self {
        for name in REPOSITORY_LOCATION_VARS {
            self.env_remove(name);
        }

        self
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

    /// Spawn git and hand back its output untouched.
    ///
    /// The single place a git process is actually created, so the safety
    /// configuration, the scrub and the environment overrides cannot be reached
    /// around by anything above. Private because raw output is a footgun in the
    /// one way this crate cares about: everything public either trims it deliberately
    /// ([`Git::try_run`], [`Git::run`]) or deliberately does not
    /// ([`Git::nul_separated`]), and which of those a caller wants is not a
    /// choice worth re-making per call site.
    fn output(&self, args: &[&str]) -> Result<Output> {
        let mut command = Command::new("git");
        command
            .args(self.safety_config())
            .args(args)
            .current_dir(&self.cwd)
            // `cwd` is only where the repository is if nothing in the inherited
            // environment says otherwise. Run from inside a git hook - a
            // pre-push gate, `git bisect run`, `rebase --exec` - something does.
            .without_inherited_repository()
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

        command
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
    use super::{
        Git, NoInheritedRepository, INHERITED_IDENTITY_VARS, SCRATCH_USER_EMAIL, SCRATCH_USER_NAME,
    };
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

    /// A file name git can carry that is not valid UTF-8: `bad-`, two bytes no
    /// UTF-8 sequence may begin with, and `.txt`.
    ///
    /// `0xff` and `0xfe` are the two bytes the encoding never uses at all, so
    /// nothing here depends on a lossy conversion's taste in replacement
    /// boundaries: whatever it does with them, it cannot hand these two back.
    const BAD_NAME: &[u8] = b"bad-\xff\xfe.txt";

    /// A path is a string of bytes on unix, not a string of characters, and git
    /// reports one as it is stored. So the reader has to hand those bytes back
    /// untouched, and a lossy conversion is exactly the defect
    /// `core.quotePath=false` and `-z` were pinned to remove: the name comes
    /// back with U+FFFD where the bytes were, which prints a name nobody typed
    /// and opens no file on disk - and in this crate a conflicted file that
    /// cannot be opened is floored at one hunk, so a file contested in two
    /// regions reports one and the total still looks plausible.
    ///
    /// The name never touches the filesystem. APFS refuses one outright
    /// (`Errno 92`, illegal byte sequence), so a fixture that wrote the file
    /// could not run on the machine this crate is developed on; git will carry
    /// the name in the *index* on any platform, which is all the reader needs to
    /// be asked about.
    ///
    /// `update-index --index-info` reads its records from stdin and
    /// [`TestRepo::git`] passes none, so that one spawn is made directly here -
    /// scrubbed through [`NoInheritedRepository`] like every other spawn in this
    /// crate, because an inherited `GIT_INDEX_FILE` would otherwise stage this
    /// name into whichever repository the hook that started the run belongs to.
    #[test]
    fn a_path_git_reports_comes_back_byte_for_byte_even_when_it_is_not_utf8() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let repo = TestRepo::init();
        repo.commit_file("seed.txt", "seed\n", "base");
        // Any blob will do - the index entry is about the name, not the content.
        let blob = repo.git(&["hash-object", "-w", "seed.txt"]);

        let mut record = format!("100644 {blob}\t").into_bytes();
        record.extend_from_slice(BAD_NAME);
        record.push(0);

        let mut child = Command::new("git")
            .args(["update-index", "-z", "--index-info"])
            .current_dir(repo.path())
            .without_inherited_repository()
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn git update-index");
        child
            .stdin
            .take()
            .expect("the piped stdin of the child just spawned")
            .write_all(&record)
            .expect("hand git the index record");
        assert!(
            child.wait().expect("wait for git update-index").success(),
            "git could not be made to hold a non-UTF-8 name in its index"
        );

        let staged = Git::new(repo.path(), "")
            .nul_separated(&["diff", "--cached", "--name-only"])
            .expect("list the staged path");

        assert_eq!(
            staged.len(),
            1,
            "exactly one path is staged beyond HEAD, got {staged:?}"
        );
        assert_eq!(
            staged[0].as_bytes(),
            BAD_NAME,
            "git reports a path as it is stored, so the reader must carry those \
             bytes back untouched rather than replacing them"
        );
    }

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
