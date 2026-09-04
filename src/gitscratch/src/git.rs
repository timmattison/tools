//! The one way a tool in this repository is allowed to invoke git.
//!
//! Every simulation runs against the developer's *real* repository, so a stray
//! git invocation could rewrite branches they care about. All calls funnel
//! through [`Git`], which pins the configuration that makes simulation
//! non-destructive — most importantly `rebase.updateRefs=false`, which would
//! otherwise move the very branch refs being simulated.
//!
//! The other half of "against the right repository" is the *environment*, which
//! [`NoInheritedGitEnvironment`] strips, because a git invocation obeys it
//! before it obeys the directory it was pointed at. The same sweep takes the
//! rest of what a hook hands its children — who is committing, and when —
//! because an environment variable outranks every config source, so the
//! identity pinned here only holds once they are gone.

use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{Context, Result};

/// Who scratch commits are attributed to. Spelled once and used both as
/// configuration and as environment, because those are two ways of saying the
/// same thing and git resolves them in that order - so they must not drift.
const HARNESS_NAME: &str = "gitscratch";
const HARNESS_EMAIL: &str = "gitscratch@localhost";

/// The prefix that makes a variable git's, and therefore one
/// [`NoInheritedGitEnvironment`] removes.
const GIT_ENVIRONMENT_PREFIX: &str = "GIT_";

/// Spawn a command that takes its repository, and its identity, from nothing it
/// inherited.
///
/// **The rule is the `GIT_` prefix, and never a list of names.** A list strips
/// nothing new the day git adds a variable, and from then on it returns the same
/// clean-looking answer as a list that works. This scrub was a fifteen-name list
/// once, and `GIT_CONFIG_PARAMETERS` walked straight through it — a variable git
/// exports to every hook, which injects arbitrary configuration (`user.email`,
/// `core.bare`, `core.hooksPath`) into every git this crate spawns. It is not a
/// location variable, so no amount of adding location names would have caught
/// it. Enumerating [`std::env::vars_os`] sweeps whatever git invents next
/// without anyone editing this file. Pinned by `tests/inherited-environment.rs`.
///
/// Three families the old lists named are worth keeping in mind, because the
/// prefix now covers all of them. The location variables — `GIT_DIR`,
/// `GIT_WORK_TREE`, `GIT_INDEX_FILE` and their kin — aim git at a repository
/// other than the one the runner is rooted in, and git hands several of them to
/// its hooks, `GIT_INDEX_FILE` often *relative* so it silently re-anchors on
/// whatever directory each command runs in. The attribution variables —
/// `GIT_AUTHOR_NAME` and its five siblings — are read in preference to `-c`
/// *and* to `git config`, so leaving them in place re-attributes anything this
/// crate commits, and the two `DATE` variables are the quieter half: left in
/// place, every commit a run makes carries one identical timestamp. The
/// configuration variables — `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`,
/// `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM` — hand the caller a way to set any
/// key at all.
///
/// Removing `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` costs nothing, because
/// removing a variable is not the same as pinning it to `/dev/null`: with them
/// gone git falls back to the host's `~/.gitconfig` and `/etc/gitconfig` exactly
/// as it does in a normal shell.
///
/// One trait rather than one per family, because the prefix cannot tell the
/// families apart, and any split back into halves is a list again. A call site
/// that asked for the location half alone would be asking for the shape this
/// crate stopped trusting.
///
/// Public, and an extension trait rather than a private helper, because the
/// commands that need it are not all git: a consumer's test suite spawning its
/// own binary inherits exactly the same environment, and a second copy of this
/// rule living in a test file is the drift this exists to prevent.
pub trait NoInheritedGitEnvironment {
    /// Remove every `GIT_`-prefixed variable from the environment this command
    /// will be spawned with.
    ///
    /// A no-op in normal use — nothing sets these outside a hook or a replay —
    /// which is precisely why it has to be unconditional. The one run where it
    /// matters is the one nobody is watching.
    ///
    /// A caller that wants one of these variables set does so *after* calling
    /// this, and wins — which is how [`Git::command`] pins `GIT_EDITOR` and the
    /// harness identity on top of a swept command.
    ///
    /// Keys are compared through [`std::ffi::OsStr::to_string_lossy`]: lossy
    /// conversion replaces invalid bytes with U+FFFD, so it can never
    /// manufacture a `GIT_` prefix out of bytes that did not spell one.
    fn without_inherited_git_environment(&mut self) -> &mut Self;
}

impl NoInheritedGitEnvironment for Command {
    fn without_inherited_git_environment(&mut self) -> &mut Self {
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with(GIT_ENVIRONMENT_PREFIX) {
                self.env_remove(&key);
            }
        }

        self
    }
}

/// Detach `command` from every part of the git environment this process
/// inherited: the repository it names and the identity it carries, in one call.
///
/// The free-function spelling of [`NoInheritedGitEnvironment`], for a call site
/// that holds a `&mut Command` rather than a builder chain.
///
/// Public because the danger is not this crate's alone. Anything in this
/// repository that spawns git - a tool that adds a worktree, a test that builds
/// a throwaway repository - is broken the same way by the same environment, and
/// the rule for what to shed is worth keeping in one reusable place rather than
/// copied into each of them to drift.
///
/// That is an offer, not a guarantee. Nothing - no lint, no type, no guard -
/// obliges a git spawn in this repository to call this, so immunity holds where
/// it is called and nowhere else.
///
/// ```no_run
/// let mut command = std::process::Command::new("git");
/// gitscratch::shed_inherited_git_environment(&mut command);
/// ```
pub fn shed_inherited_git_environment(command: &mut Command) {
    command.without_inherited_git_environment();
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
    ///
    /// **The subcommand is a parameter of its own, and that is a guard rather
    /// than a convenience.** Everything ahead of the subcommand is git's own
    /// option position, and a caller who reaches it undoes the rest of this
    /// method: git's rule for two `-c` pairs naming one key is that the last
    /// pair wins, so a smuggled pair re-pins any setting [`Git::safety_config`]
    /// fixed — `rebase.updateRefs=false` included, which is the pin
    /// `tests/safety.rs` proves stands between a replay and the developer's own
    /// branch refs. A smuggled `-C` is the same hole aimed at a different
    /// guarantee: it outranks the working directory set below, so a runner
    /// documented as pinned to one repository answers about any repository on
    /// the machine. Taking the subcommand separately puts every caller argument
    /// after it, where git reads it as an argument of the subcommand and not as
    /// one of its own. Pinned by
    /// `an_argument_cannot_re_pin_a_setting_the_safety_config_fixed` and
    /// `an_argument_cannot_aim_the_runner_at_another_repository`.
    fn command(&self, subcommand: &str, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .args(self.safety_config())
            .arg(subcommand)
            .args(args)
            .current_dir(&self.cwd)
            // `cwd` is only where the repository is if nothing in the inherited
            // environment says otherwise. Run from inside a git hook - a
            // pre-push gate, `git bisect run`, `rebase --exec` - something does.
            // Config alone does not settle the identity either: a hook exports
            // the identity into the environment, and every commit that rebase,
            // cherry-pick, or am replays exports it again - and those variables
            // outrank the `user.name` pinned in safety_config. Left in place
            // they put the developer's own name on scratch commits, which is
            // the single thing the pin exists to prevent. Both halves leave
            // with the one sweep, because the rule is the `GIT_` prefix.
            .without_inherited_git_environment()
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

    /// Spawn git and hand back its output untouched.
    ///
    /// Private because raw output is a footgun in the one way this crate cares
    /// about: everything public either trims it deliberately ([`Git::try_run`],
    /// [`Git::run`]) or deliberately does not ([`Git::nul_separated`],
    /// [`Git::nul_separated_paths`], [`Git::path`]), and which of those a caller
    /// wants is not a choice worth re-making per call site.
    fn output(&self, subcommand: &str, args: &[&str]) -> Result<Output> {
        self.command(subcommand, args)
            .output()
            .with_context(|| format!("failed to run git {}", invocation(subcommand, args)))
    }

    /// Run git, returning the outcome whether or not it succeeded.
    ///
    /// Both streams come back trimmed, and lossily decoded, which is what a
    /// caller reporting them to a human wants and what every caller of this
    /// method does with them. A caller reading a *path* wants the opposite on
    /// both counts, and there are two readers for that: a list of paths comes
    /// back through [`Git::nul_separated_paths`], and one path through
    /// [`Git::path`].
    ///
    /// # Errors
    ///
    /// Returns an error only if git could not be spawned at all.
    pub fn try_run(&self, subcommand: &str, args: &[&str]) -> Result<GitOutput> {
        let output = self.output(subcommand, args)?;

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
    pub fn run(&self, subcommand: &str, args: &[&str]) -> Result<String> {
        let output = self.try_run(subcommand, args)?;

        anyhow::ensure!(
            output.success,
            "git {} failed:\n{}\n{}",
            invocation(subcommand, args),
            output.stdout,
            output.stderr
        );

        Ok(output.stdout)
    }

    /// Run git with `-z` and return stdout split on NUL, byte for byte.
    ///
    /// The one reader that keeps git's output intact, and the base of the only
    /// way to read a list of paths out of git, because the line-oriented
    /// alternative it replaced could not be made correct. `-z` is the single
    /// output mode in which git prints a path exactly as it is stored: no
    /// C-quoting, no octal escaping, and no ambiguity about where one path ends,
    /// since NUL is the one byte a path cannot contain. That last part is why
    /// nothing here trims. A path may legitimately begin or end with a space -
    /// or with U+3000, which Rust's Unicode-aware `str::trim` eats just as
    /// readily - and a separator that cannot occur inside a path means there is
    /// nothing to trim *for*.
    ///
    /// **Bytes, not text.** A field comes back as the bytes git wrote, because
    /// on unix that is what a path *is* - an arbitrary byte string with no
    /// encoding promised - and a lossy conversion to `String` destroys exactly
    /// the names this reader exists to preserve: every byte outside UTF-8
    /// becomes U+FFFD, which prints a name nobody typed and opens no file on
    /// disk. That is the same two-part failure C-quoting causes, arriving by a
    /// different door, and the second half is the quiet one - in this crate a
    /// conflicted file that cannot be opened is floored at one hunk, so a file
    /// contested in two regions reports one and the total still looks plausible.
    ///
    /// A caller reading a list of *paths* wants [`Git::nul_separated_paths`],
    /// which is this plus that one conversion. This one is for output whose
    /// fields are not paths: a `status --porcelain -z` record is `XY <path>`,
    /// so it is read as bytes and only its tail is ever a path.
    ///
    /// `-z` is the first argument after the subcommand, ahead of everything the
    /// caller passed, so an argument list that finishes with `--` and a
    /// pathspec still gets it as a flag rather than as a path. That position is
    /// structural: [`Git::command`] takes the subcommand separately and puts it
    /// first, so `-z` at the front of this slice is `-z` right after the
    /// subcommand.
    ///
    /// Empty fields are dropped. Git terminates rather than separates, so the
    /// last NUL always leaves one; no path is ever the empty string, so nothing
    /// real is lost with it.
    ///
    /// Not for a list of paths — use [`Git::paths`]. Git escapes a path on its
    /// way out of a line-oriented listing and the trimming here finishes the
    /// job, so a name can come back spelled differently from the file it names.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned or exited non-zero.
    pub fn nul_separated(&self, subcommand: &str, args: &[&str]) -> Result<Vec<Vec<u8>>> {
        let asked = with_nul_delimiters(args);

        let output = self.output(subcommand, &asked)?;

        anyhow::ensure!(
            output.status.success(),
            "git {} failed:\n{}\n{}",
            invocation(subcommand, &asked),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );

        Ok(output
            .stdout
            .split(|byte| *byte == b'\0')
            .filter(|field| !field.is_empty())
            .map(<[u8]>::to_vec)
            .collect())
    }

    /// The same NUL-separated fields, read as the paths they name.
    ///
    /// **The reader a call site reading a path list must use.** Everything
    /// [`Git::nul_separated`] says about `-z`, about trimming and about bytes
    /// applies here unchanged; this adds the one conversion that turns those
    /// bytes into a path, and it is a conversion rather than a parse - on unix
    /// the bytes *are* the path, so nothing is interpreted, validated or
    /// replaced on the way.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned or exited non-zero.
    pub fn nul_separated_paths(&self, subcommand: &str, args: &[&str]) -> Result<Vec<PathBuf>> {
        Ok(self
            .nul_separated(subcommand, args)?
            .into_iter()
            .map(path_from_git)
            .collect())
    }

    /// Run git and return the paths it listed, as the developer spelled them.
    ///
    /// [`Git::run`] cannot be used for a path, because git's line-oriented
    /// output is not a faithful rendering of one. A name with a byte outside
    /// printable ASCII comes back C-quoted - `café.txt` as `"caf\303\251.txt"` -
    /// and a name with a leading or trailing space comes back intact only to
    /// lose it to trimming. Neither loss announces itself. A caller then reports
    /// a name nobody typed, and a name nobody typed opens no file on disk - so
    /// anything that goes on to read the path falls back to whatever it does for
    /// a file it cannot read. In this crate that is one hunk for a conflicted
    /// file, which undercounts the work and still looks plausible. A caller that
    /// hands the spelling back to git as a pathspec pays twice, because git
    /// dequotes nothing on the way in.
    ///
    /// So the paths are asked for NUL-delimited instead, which is git's own
    /// answer to this and turns the escaping off entirely. `-z` is the first
    /// argument after the subcommand, ahead of everything the caller passed,
    /// because a command that carries a pathspec ends in `-- <paths>` and
    /// everything after `--` is a path, not an option.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned, if it exited non-zero, or
    /// if a path it printed is not valid UTF-8. That last one is deliberately
    /// fatal: replacing an undecodable byte substitutes U+FFFD and hands back a
    /// name no file has, which is the very silence this method exists to
    /// remove.
    pub fn paths(&self, subcommand: &str, args: &[&str]) -> Result<Vec<String>> {
        let asked = with_nul_delimiters(args);

        let output = self.output(subcommand, &asked)?;

        anyhow::ensure!(
            output.status.success(),
            "git {} failed:\n{}\n{}",
            invocation(subcommand, &asked),
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
                        invocation(subcommand, &asked),
                        String::from_utf8_lossy(path)
                    )
                })
            })
            .collect()
    }

    /// Run git and return the one path it printed, as the bytes git wrote.
    ///
    /// **The reader a call site reading a single path must use.** [`Git::run`]
    /// is wrong for a path in two ways, and neither loss announces itself. It
    /// trims, and `str::trim` is Unicode-aware, so it eats a trailing space and
    /// a trailing U+3000 alike - and a repository directory named with one of
    /// those spells that character as the last character of its own path. It
    /// also decodes lossily, so every byte outside UTF-8 becomes U+FFFD, and on
    /// unix a path is an arbitrary byte string with no encoding promised. Both
    /// losses hand back a name that opens no file, and a caller reads that as
    /// an absence: `exists()` is false, so the thing asked about reports as not
    /// there rather than as unreadable.
    ///
    /// **One newline, never a trim.** Git terminates one answer with a single
    /// `\n`, and every other byte of that answer belongs to the path. So this
    /// strips exactly that one byte and hands the rest to the same conversion
    /// [`Git::nul_separated_paths`] uses, which takes bytes as the path they
    /// spell rather than decoding them. Trimming instead is the defect above,
    /// arriving by a different door.
    ///
    /// **`-z` is not available here, which is why this reader exists.**
    /// [`Git::nul_separated_paths`] reads a path list by asking git for NUL
    /// delimiters, and `rev-parse` has no such flag: it prints `-z` back as an
    /// unknown option and exits 0, so the reader hands back `-z` and the path
    /// as two fields. A single answer needs no separator anyway - the end of
    /// stdout ends the path - so the two questions take two readers.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned or exited non-zero.
    pub fn path(&self, subcommand: &str, args: &[&str]) -> Result<PathBuf> {
        let output = self.output(subcommand, args)?;

        anyhow::ensure!(
            output.status.success(),
            "git {} failed:\n{}\n{}",
            invocation(subcommand, args),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );

        let mut printed = output.stdout;
        if printed.last() == Some(&b'\n') {
            printed.pop();
        }

        Ok(path_from_git(printed))
    }

    /// Resolve a revision to a full commit id.
    ///
    /// **Both flags are the question, not decoration.** A bare
    /// `git rev-parse <revision>` reads a dash-leading argument as an option it
    /// does not know, prints the argument back, and exits 0 - rev-parse passes
    /// an option it cannot place through to rev-list rather than refusing it.
    /// This method is the whole pre-flight, so that exit code becomes "the
    /// revision names a commit", and a name that names nothing starts a full
    /// replay. `grind -- --root` answered `clean` for a branch that does not
    /// exist, which is the one answer this crate exists never to give.
    ///
    /// `--verify` makes git refuse a revision it cannot resolve. It reports
    /// exactly one object id or it fails, so an argument that resolves to
    /// nothing exits 128 instead of exiting 0 with the argument echoed back.
    ///
    /// `--end-of-options` ends git's own option position, so the revision
    /// arrives as a revision whatever git learns to recognise next. `--verify`
    /// alone catches every dash-leading revision git knows today, because an
    /// option prints no object id and `--verify` demands one. That is a fact
    /// about today's option list rather than a rule, and this crate takes the
    /// rule: the same reasoning that makes [`NoInheritedGitEnvironment`] match
    /// a prefix instead of a list of names.
    ///
    /// # Errors
    ///
    /// Returns an error if the revision does not name a commit. The message
    /// names the revision, because that name is what the caller typed and has
    /// to correct.
    pub fn rev_parse(&self, revision: &str) -> Result<String> {
        self.run(
            "rev-parse",
            &[
                "--verify",
                "--end-of-options",
                &format!("{revision}^{{commit}}"),
            ],
        )
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
            // A rebase that keeps merges puts a merge commit on its todo list,
            // and a merge commit at a halt is a commit the replay cannot
            // measure: `diff-tree` prints no path at all for one unless it is
            // asked for `-c`, `--cc` or `-m`, and the empty-commit probe asks
            // for none of them. That probe would read the halt as a commit that
            // changes nothing, and `rebase --skip` would drop a whole side of
            // history. Executed rather than reasoned: git 2.55 was watched to
            // re-create the merge commit under `rebase.rebaseMerges=true`, so
            // the developer's own configuration is what opens this route.
            // `stopped_commit_is_already_in_head` refuses a multi-parent
            // stopped commit as well, and that refusal holds whatever a later
            // setting does; this pin closes the one route into it that exists
            // today.
            "rebase.rebaseMerges=false",
            // Simulated mains are loose commits nothing references yet; an
            // opportunistic gc mid-run could collect one out from under us.
            // This entry covers the gc task and nothing else - the switch on
            // the rest of automatic maintenance is the entry below it.
            "gc.auto=0",
            // Git's `run_auto_maintenance` starts the maintenance tasks unless
            // this key is explicitly false, and the default is to run them.
            // Every resolved conflict runs `rebase --continue`, which commits,
            // and a commit reaches that call. On a developer who has run
            // `git maintenance start` the incremental strategy turns the
            // prefetch task on, and prefetch carries no auto-condition of its
            // own, so `--auto` does not hold it back. Prefetch fetches from
            // every remote and writes `refs/prefetch/*` into the real
            // repository, because a linked scratch worktree shares the common
            // dir. A dry run that reaches the network and writes refs is the
            // class `gc.auto=0` was added for. The chain from
            // `run_auto_maintenance` to prefetch is read from git's source
            // rather than executed.
            "maintenance.auto=false",
            // The filesystem monitor names a program git runs itself rather
            // than one it resolves through the hooks directory, so the
            // redirected `core.hooksPath` does not take it away. The classic
            // watchman integration is spelled
            // `core.fsmonitor=.git/hooks/fsmonitor-watchman`, a path that
            // survives the redirect verbatim, and every index refresh a replay
            // performs would run it - in the developer's repository and in the
            // scratch worktree both. `core.fsmonitor=true` costs more than
            // that: git starts a daemon that watches a temporary directory the
            // replay is about to delete. A freshly created scratch worktree
            // gains nothing from a monitor, so the pin costs the replay
            // nothing. Read from git's settings resolution rather than
            // executed.
            "core.fsmonitor=false",
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
            // lines under any setting, which is why the reader this type offers
            // for one is [`Git::nul_separated_paths`]: `-z` turns quoting off
            // outright and separates on the one byte a path cannot contain, and
            // the bytes between two separators become the path without being
            // decoded on the way - a path on unix is bytes, and a name that is
            // not valid UTF-8 is destroyed by a lossy conversion exactly as
            // thoroughly as by an octal escape.
            //
            // Kept anyway, because it costs one `-c` and it narrows what a
            // future call site can do wrong. There are two readers for a path
            // and `run` is neither of them: a list of paths goes through
            // `nul_separated_paths`, one path goes through `path`, and reading
            // either back through `run` is a bug. With this pinned it is a bug
            // that survives the common case instead of mangling every
            // non-ASCII name in the repository. Pinning it on the single door
            // every git call goes through is what makes that free.
            "core.quotePath=false",
        ]
        .iter()
        .map(|setting| (*setting).to_string())
        // The identity belongs to this crate, not to whichever tool is driving
        // it, so every consumer's scratch commits are attributable to the
        // harness that actually made them.
        .chain([
            // These two settle the identity only in company with
            // `NoInheritedGitEnvironment`, which takes back off the identity
            // variables that would otherwise outrank them.
            format!("user.name={HARNESS_NAME}"),
            format!("user.email={HARNESS_EMAIL}"),
            format!("core.hooksPath={}", self.hooks_path),
        ])
        .flat_map(|setting| ["-c".to_string(), setting])
        .collect();

        // A path read out of one invocation and handed back to the next is not
        // a path any more, it is a pathspec: a leading `:` is pathspec magic,
        // and `*`, `?` and `[` are wildcards. Without this a file genuinely
        // called `star*.txt` matches `starOTHER.txt` too, so a question asked
        // about *this* path quietly gets answered about some other file's. A
        // main option rather than a `-c` pair, so it belongs here with them,
        // ahead of the subcommand.
        //
        // Over-matching like that is the mild half. The half worth the guard is
        // `:/foo.txt`, a `foo.txt` in a directory named `:`: read as magic its
        // `:/` means from the top of the working tree, so git answers about the
        // root `foo.txt` instead. An answer about the wrong file is an answer
        // nothing downstream can tell from the right one.
        //
        // **No call site in this crate hands a path back to git today.** The
        // empty-commit probe was the one that did, and it now intersects the two
        // path lists in Rust instead - which is bounded by no argv and reads a
        // name as a name. So this pin currently protects a call site nobody has
        // written yet, and nothing in the suite can be made to fail by removing
        // it; `MUTATIONS.md` records that removal being run and watched to change
        // nothing. It stays because it costs one argument at the single door
        // every git call in this crate goes through, and because the next call
        // site that reads a path list back is one edit away.
        arguments.push("--literal-pathspecs".to_string());

        arguments
    }
}

/// How one invocation is spelled in a message: the subcommand, then the
/// arguments after it, exactly in the order [`Git::command`] hands them to git.
///
/// The `-c` pairs are left out. They are the same on every invocation, they are
/// long, and a caller reading a failure wants the command they asked for.
fn invocation(subcommand: &str, args: &[&str]) -> String {
    std::iter::once(subcommand)
        .chain(args.iter().copied())
        .collect::<Vec<&str>>()
        .join(" ")
}

/// `args` with `-z` in front of it, which is the position right after the
/// subcommand once [`Git::command`] builds the argv.
///
/// Front rather than back, because a command that carries a pathspec ends in
/// `-- <paths>` and everything after `--` is a path. A `-z` on the end would be
/// asked for as a file name.
fn with_nul_delimiters<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut asked = Vec::with_capacity(args.len() + 1);
    asked.push("-z");
    asked.extend_from_slice(args);
    asked
}

/// Take the bytes git wrote for one path as that path.
///
/// Free rather than a `From` impl, and defined on both platforms with the one
/// call site in [`Git::nul_separated_paths`], because the alternative shape - a
/// function on unix and an inline conversion elsewhere - is how the two halves
/// drift apart unnoticed: nothing on this platform can warn that the other one
/// is wrong, since it is never compiled here.
///
/// On unix a path is an arbitrary byte string, so this is a move rather than a
/// conversion: no encoding is assumed and no byte is replaced.
#[cfg(unix)]
fn path_from_git(field: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;

    PathBuf::from(std::ffi::OsString::from_vec(field))
}

/// Take the bytes git wrote for one path as that path.
///
/// Windows filenames are Unicode - the kernel stores them as UTF-16, and git
/// writes them out as UTF-8 - so the lossy conversion loses nothing real here.
/// A byte sequence this replaces is one no Windows filesystem could have been
/// holding a name for in the first place.
#[cfg(not(unix))]
fn path_from_git(field: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&field).into_owned())
}

#[cfg(test)]
mod tests {
    use pulldown_cmark::{Event, Parser, Tag};
    use tempfile::TempDir;

    use super::{
        Git, NoInheritedGitEnvironment, PathBuf, GIT_ENVIRONMENT_PREFIX, HARNESS_EMAIL,
        HARNESS_NAME,
    };
    use crate::testing::TestRepo;

    /// The setting the two tests below try to undo. It stands for every pin
    /// [`Git::safety_config`] makes, because git resolves them all by one rule,
    /// and it is the pin that costs the most: `tests/safety.rs` proves that a
    /// replay rewrites the developer's own branch refs when this setting is on.
    const PINNED_SETTING: &str = "rebase.updateRefs";

    /// The value a smuggled `-c` pair puts in place of the pinned one. It is
    /// not a boolean, so no reader can take it for another spelling of `false`.
    const SMUGGLED_VALUE: &str = "OVERRIDDEN";

    /// An argument the caller hands the runner must not reach the position
    /// ahead of the subcommand, because that position belongs to git.
    ///
    /// Git reads the arguments ahead of the subcommand as its own options, and
    /// its rule for two `-c` pairs that name one key is that the last pair
    /// wins. An argument list that lands there therefore undoes every pin
    /// [`Git::safety_config`] makes, one `-c` pair at a time.
    ///
    /// The runner keeps the caller out of that position by taking the
    /// subcommand as a parameter of its own. An argument can only land after
    /// the subcommand, where git reads it as an argument of the subcommand
    /// rather than as one of its own. So the read below either fails, because
    /// `config` refuses a `-c` it does not know, or hands back the pinned
    /// value. Both of those are the guard holding.
    ///
    /// Two controls stand ahead of the assertion, because an assertion that
    /// something did not happen passes just as readily when it was never
    /// possible. The first proves the pin is real and readable. The second
    /// proves git still lets the last `-c` pair win, which is the hazard.
    #[test]
    fn an_argument_cannot_re_pin_a_setting_the_safety_config_fixed() {
        let repo = TestRepo::init();
        let git = Git::new(repo.path(), "");
        let smuggled = format!("{PINNED_SETTING}={SMUGGLED_VALUE}");

        assert_eq!(
            git.run("config", &["--get", PINNED_SETTING])
                .expect("read back the setting the safety configuration pins"),
            "false",
            "the safety configuration has to pin `{PINNED_SETTING}=false`, or there is nothing \
             here for an argument to undo and the assertion below is measured against nothing"
        );

        assert_eq!(
            repo.git(&[
                "-c",
                &format!("{PINNED_SETTING}=false"),
                "-c",
                &smuggled,
                "config",
                "--get",
                PINNED_SETTING,
            ]),
            SMUGGLED_VALUE,
            "git no longer lets the last `-c` pair win, so this test could only pass vacuously"
        );

        let read_back = git.run("config", &["-c", &smuggled, "--get", PINNED_SETTING]);

        if let Ok(setting) = read_back {
            assert_eq!(
                setting, "false",
                "an argument reached the position ahead of the subcommand, where git reads it as \
                 one of its own options, and the last `-c` pair wins. Every guard the safety \
                 configuration pins is off for that invocation, `{PINNED_SETTING}=false` \
                 included, and that one costs the developer the branch under replay."
            );
        }
    }

    /// The same position also carries `-C`, which moves git to another
    /// directory, so the guard has to hold for where the runner works as well
    /// as for what it pins.
    ///
    /// `-C` outranks the working directory [`Git::command`] sets, so an
    /// argument that reaches the position ahead of the subcommand aims a runner
    /// documented as pinned to one working directory at any repository on the
    /// machine. The developer's own repository is the one that matters.
    ///
    /// Both fixtures answer through git itself, so the two paths are spelled
    /// the way git spells them and nothing here has to canonicalise a path to
    /// compare it. The difference between the two answers is the armed control:
    /// it proves `-C` really does move git, and therefore that the assertion
    /// below has a hazard to defend against.
    #[test]
    fn an_argument_cannot_aim_the_runner_at_another_repository() {
        let here = TestRepo::init();
        let elsewhere = TestRepo::init();
        let git = Git::new(here.path(), "");
        let their_path = elsewhere.path().to_str().expect("utf-8 fixture path");

        let ours = git
            .path("rev-parse", &["--absolute-git-dir"])
            .expect("ask git which repository the runner is rooted in");
        let theirs =
            PathBuf::from(here.git(&["-C", their_path, "rev-parse", "--absolute-git-dir"]));

        assert_ne!(
            ours, theirs,
            "`-C` no longer moves git to another directory, so this test could only pass vacuously"
        );

        let read_back = git.path("rev-parse", &["-C", their_path, "--absolute-git-dir"]);

        if let Ok(answered) = read_back {
            assert_eq!(
                answered, ours,
                "an argument reached the position ahead of the subcommand, where `-C` moves git \
                 to another directory. A runner pinned to one working directory answered about a \
                 repository nobody pointed it at, and the developer's own repository is reachable \
                 the same way."
            );
        }
    }

    /// Read one setting back twice: once through plain git, which has to answer
    /// with the repository's own value, and once through the runner, which has
    /// to answer with the pinned one.
    ///
    /// The first read is the armed control. A setting the fixture never took is
    /// a setting the runner cannot be shown to override, so an assertion that
    /// the pinned value came back would pass on a repository where nothing was
    /// ever at stake. The fixture arms the opposite value, and plain git reports
    /// it, before the runner is asked anything.
    ///
    /// `setting` is spelled the way [`Git::safety_config`] spells it, `key=value`
    /// and nothing else, so the test names the pin rather than a paraphrase of
    /// it.
    fn assert_the_runner_pins(setting: &str, over: &str) {
        let (key, pinned) = setting
            .split_once('=')
            .expect("a pinned setting is spelled `key=value`");

        let repo = TestRepo::init();
        repo.git(&["config", key, over]);

        assert_eq!(
            repo.git(&["config", "--get", key]),
            over,
            "the fixture does not hold `{key}={over}`, so there is nothing here for the runner to \
             override and the assertion below is measured against nothing"
        );

        assert_eq!(
            Git::new(repo.path(), "")
                .run("config", &["--get", key])
                .unwrap_or_else(|error| panic!("read `{key}` back through the runner: {error:#}")),
            pinned,
            "`{setting}` is not pinned, so git reads `{key}` out of the developer's own \
             configuration and acts on it for the length of a replay"
        );
    }

    /// Automatic maintenance is a second switch beside `gc.auto=0`, and the half
    /// it leaves open reaches the network.
    ///
    /// `gc.auto=0` stops the gc task alone. `maintenance.auto` governs the whole
    /// set, and git's `run_auto_maintenance` returns early only when that key is
    /// explicitly false - the default is to run. Every resolved conflict runs
    /// `rebase --continue`, which commits, and a commit reaches that call. On a
    /// developer who has run `git maintenance start` the incremental strategy
    /// turns the prefetch task on, and prefetch carries no auto-condition of its
    /// own, so `--auto` does not hold it back. Prefetch fetches from every
    /// remote and writes `refs/prefetch/*` into the real repository, because a
    /// linked scratch worktree shares the common dir. A dry run that reaches the
    /// network and writes refs is the class `gc.auto=0` was added for.
    ///
    /// The chain from `run_auto_maintenance` to prefetch is read from git's
    /// source rather than executed. What this test executes is the pin: the
    /// fixture turns the key on, and the runner has to report it off.
    #[test]
    fn pins_automatic_maintenance_off_even_when_the_repository_turns_it_on() {
        assert_the_runner_pins("maintenance.auto=false", "true");
    }

    /// The filesystem monitor names a program git runs itself, so the redirected
    /// `core.hooksPath` does not disable it.
    ///
    /// The classic watchman integration is spelled
    /// `core.fsmonitor=.git/hooks/fsmonitor-watchman`. Git executes that path
    /// directly rather than resolving it through the hooks directory, so the
    /// redirect leaves it standing and every index refresh a replay performs
    /// runs it - in the real repository and in the scratch worktree both.
    /// `core.fsmonitor=true` costs more than that: git starts a daemon that
    /// watches a temporary directory the replay is about to delete.
    ///
    /// `tests/safety.rs` states the guarantee as "no replay fires anything", and
    /// the hooks it plants cannot reach this route at all, so the pin is
    /// asserted here instead. A freshly created scratch worktree gains nothing
    /// from a monitor, so the pin costs the replay nothing.
    ///
    /// Git's resolution of the setting is read from its source rather than
    /// executed. What this test executes is the pin.
    #[test]
    fn pins_the_filesystem_monitor_off_even_when_the_repository_names_one() {
        assert_the_runner_pins("core.fsmonitor=false", ".git/hooks/fsmonitor-watchman");
    }

    /// A merge-preserving rebase puts a merge commit on the replay's todo list,
    /// and a merge commit at a halt is a commit the replay cannot measure.
    ///
    /// `diff-tree` prints no path at all for a merge commit unless it is asked
    /// for `-c`, `--cc` or `-m`. The probe that decides whether a halted commit
    /// adds anything to the new base asks for none of them, so an unguarded
    /// probe reads a merge as a commit that changes nothing, and `rebase --skip`
    /// drops a whole side of history. Git 2.55 was watched to re-create the
    /// merge commit under `rebase.rebaseMerges=true`, so the developer's own
    /// configuration is what opens this route.
    ///
    /// The probe refuses a stopped commit with more than one parent as well, and
    /// that refusal holds whatever a later setting does. This pin closes the one
    /// route into it that exists today. Both halves are wanted: the pin keeps
    /// the replay away from a state it cannot measure, and the refusal makes the
    /// classification correct if it ever arrives there anyway.
    #[test]
    fn pins_merge_preserving_rebase_off_even_when_the_repository_turns_it_on() {
        assert_the_runner_pins("rebase.rebaseMerges=false", "true");
    }

    /// `core.quotePath=false` needs its own test now that it protects nothing a
    /// caller can otherwise observe.
    ///
    /// It used to be pinned indirectly, by `tests/conflicts.rs` asserting the
    /// answer a non-ASCII conflicted path produces. That stopped being a test of
    /// this setting the moment a `-z` reader became the way a path list comes
    /// back: `-z` output is unquoted whatever `quotePath` says, so removing
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
            .run("diff", &["--cached", "--name-only"])
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
    /// scrubbed through [`NoInheritedGitEnvironment`] like every other spawn in this
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
            .without_inherited_git_environment()
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
            .nul_separated("diff", &["--cached", "--name-only"])
            .expect("list the staged path");

        assert_eq!(
            staged.len(),
            1,
            "exactly one path is staged beyond HEAD, got {staged:?}"
        );
        assert_eq!(
            staged[0], BAD_NAME,
            "git reports a path as it is stored, so the reader must carry those \
             bytes back untouched rather than replacing them"
        );
    }

    /// The stem every fixture repository of the test below is named with. The
    /// whitespace goes on the end of it.
    const REPOSITORY_STEM: &str = "repository";

    /// The two spellings of trailing whitespace that `str::trim` eats. A space
    /// is the one a developer types by accident. U+3000 is the one nobody
    /// expects a trimmer to touch, and Rust's trimmer is Unicode-aware, so it
    /// takes that one just as readily.
    const TRAILING_WHITESPACE: [(&str, char); 2] = [
        ("a space", ' '),
        ("U+3000, the ideographic space", '\u{3000}'),
    ];

    /// A path git printed has to come back with its last character still on it,
    /// and a repository whose own directory name ends in whitespace is where
    /// that character is at risk.
    ///
    /// [`Git::run`] trims, and `str::trim` is Unicode-aware, so it eats a
    /// trailing space and a trailing U+3000 alike. A path read back through it
    /// therefore names a directory that does not exist, and every question
    /// asked of that path is answered about nothing: `exists()` is false, an
    /// open fails, and the caller reads the loss as an absence. Nothing says a
    /// character went missing.
    ///
    /// `rev-parse --show-toplevel` is asked rather than the `--git-path` the
    /// replay asks for, and the difference is worth stating. `--git-path` glues
    /// a state directory name onto the end of its answer, so the repository's
    /// own last character lands in the middle of the path and the trimmer
    /// cannot reach it. What reaches it there is the other half of the same
    /// defect - the lossy decode, which replaces every byte outside UTF-8 with
    /// U+FFFD wherever it sits. APFS refuses a name with such a byte outright,
    /// so no fixture on this machine can hold one, and `--show-toplevel` is the
    /// answer whose last character is the repository's own. The reader is one
    /// reader for both halves, so pinning the half that can be built here pins
    /// the reader.
    ///
    /// The armed control runs first. It reads the same answer back through
    /// [`Git::run`] and requires exactly the trailing character to be missing,
    /// so the assertion below stands against a live loss rather than against a
    /// trimmer that already leaves the path alone.
    #[test]
    fn a_path_that_ends_in_whitespace_comes_back_with_that_whitespace_intact() {
        for (spelling, trailing) in TRAILING_WHITESPACE {
            let parent = TempDir::new().expect("create the directory the fixture sits in");
            let root = parent.path().join(format!("{REPOSITORY_STEM}{trailing}"));
            std::fs::create_dir(&root).unwrap_or_else(|error| {
                panic!("create a repository directory whose name ends in {spelling}: {error}")
            });

            let git = Git::new(&root, "");
            git.run("init", &["-q", "-b", "main"])
                .expect("initialise the fixture repository");

            // Git resolves symlinks on its way to an answer, and macOS puts a
            // temporary directory behind one - `/var` for `/private/var` - so
            // the two paths agree only after the same resolution.
            let expected = std::fs::canonicalize(&root).expect("canonicalise the fixture path");

            let through_run = git
                .run("rev-parse", &["--show-toplevel"])
                .expect("read the repository root back through the trimming reader");
            assert_eq!(
                format!("{through_run}{trailing}"),
                expected.to_string_lossy(),
                "`run` no longer eats {spelling} off the end of git's answer, so the assertion \
                 below could only pass vacuously"
            );

            let read = git
                .path("rev-parse", &["--show-toplevel"])
                .expect("read the repository root back as the bytes git printed");

            assert_eq!(
                read, expected,
                "a reader for one path has to hand back the bytes git printed. A repository \
                 directory named with {spelling} on the end spells that character as the last \
                 character of its own path, and a trimmed answer names a directory nothing \
                 holds."
            );
        }
    }

    /// The subcommand `stopped_commit_is_already_in_head` asks which paths a
    /// halted commit touched. Spelled here, with the arguments below, so the
    /// tests pin the call the replay actually depends on rather than a
    /// plausible-looking neighbour of it.
    const TOUCHED_PATHS_SUBCOMMAND: &str = "diff-tree";

    /// The arguments that go after it, with the commit left off the end.
    /// `--ignore-submodules=none` is one of them because the probe asks both of
    /// its invocations for it, so that a submodule pointer reads the same way to
    /// the plumbing command and to the porcelain one.
    const TOUCHED_PATHS: [&str; 5] = [
        "--no-commit-id",
        "--name-only",
        "-r",
        "--root",
        "--ignore-submodules=none",
    ];

    /// A path git cannot spell as UTF-8 has to stop the replay, not be repaired
    /// into one that matches nothing.
    ///
    /// This is the one loss [`Git::paths`] cannot undo. The quoting and the
    /// trimming it exists to defeat are both reversible — ask git for NUL
    /// delimiters and the original bytes come back — but a byte that is not
    /// valid UTF-8 has no `String` to come back *as*. Decoding it lossily, the
    /// way [`Git::try_run`] decodes git's output everywhere else, substitutes
    /// U+FFFD and yields a name no file has. The replay then shows a developer a
    /// path they cannot find in their own repository, and anything that opens
    /// the path fails — which in this crate floors a conflicted file at one hunk
    /// and undercounts the work while the total still looks plausible. Every
    /// other test in this suite stays green, because no other fixture holds a
    /// name git has to refuse.
    ///
    /// The classification is safe from this one, and it is worth saying which
    /// half is which. `stopped_commit_is_already_in_head` reads both of its path
    /// lists through this method, so a lossy decode mangles the two lists the
    /// same way and their intersection is unchanged. What a repaired name still
    /// costs is the report.
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
            git.paths(TOUCHED_PATHS_SUBCOMMAND, &ordinary)
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

        let error = git.paths(TOUCHED_PATHS_SUBCOMMAND, &listed).expect_err(
            "a name git cannot spell as UTF-8 must stop the replay; decoding it lossily hands \
             back a U+FFFD name that names no file, which is a path the developer cannot find \
             and a file nothing can open",
        );
        assert!(
            format!("{error:#}").contains("listed a path that is not valid UTF-8"),
            "the refusal has to name what went wrong, since the developer's next move is to look \
             at the path git could not hand over: {error:#}"
        );
    }

    /// A revision that starts with a dash and names no commit in any fixture.
    ///
    /// `--root` is the one that costs the most, because git accepts it as an
    /// option of `rebase`: a replay handed this name rebases the whole history
    /// onto nothing, hits no conflict, and answers "clean" for a branch that
    /// does not exist.
    const DASH_LEADING_REVISION: &str = "--root";

    /// A revision that starts with a dash is a revision, and the runner has to
    /// hand it to git as one.
    ///
    /// Plain `git rev-parse <revision>` reads a dash-leading argument as an
    /// option it does not know, prints the argument back, and exits 0. The
    /// pre-flight reads that exit code as "this revision names a commit", so
    /// the one check between a name that names nothing and a full replay lets
    /// the run through. `grind -- --root` then printed a clean verdict for a
    /// branch that does not exist, which is the cheap answer this crate exists
    /// never to give.
    ///
    /// The armed control runs first. It proves that plain git still prints the
    /// argument back at exit 0, so the refusal below stands against a live
    /// hazard rather than against a git that already refuses.
    #[test]
    fn refuses_a_revision_that_starts_with_a_dash_rather_than_echoing_it_back() {
        let repo = TestRepo::init();
        repo.commit_file("seed.txt", "seed\n", "seed");
        let asked = format!("{DASH_LEADING_REVISION}^{{commit}}");

        assert_eq!(
            repo.git(&["rev-parse", &asked]),
            asked,
            "git no longer prints a dash-leading argument back at exit 0, so this test could \
             only pass vacuously"
        );

        let error = Git::new(repo.path(), "")
            .rev_parse(DASH_LEADING_REVISION)
            .expect_err(
                "a revision that names no commit has to be refused. Accepting it lets a replay \
                 start on a name that names nothing, and `--root` is a `rebase` option, so the \
                 replay finishes and reports a clean verdict for a branch nobody has",
            );

        assert!(
            format!("{error:#}").contains(DASH_LEADING_REVISION),
            "the refusal has to name the revision that did not resolve, because that name is \
             what the developer typed and has to correct: {error:#}"
        );
    }

    /// The refusal above must refuse only what it cannot resolve.
    ///
    /// A guard that answers "no" to every revision passes the test above and
    /// breaks every caller, and the two failures look nothing alike from the
    /// outside: one is a run that stops, the other is a tool nobody can use.
    /// So the reader is asked for a revision that does name a commit, and its
    /// answer is compared with git's own.
    #[test]
    fn resolves_a_revision_that_names_a_commit_to_its_full_id() {
        let repo = TestRepo::init();
        repo.commit_file("seed.txt", "seed\n", "seed");

        assert_eq!(
            Git::new(repo.path(), "")
                .rev_parse("HEAD")
                .expect("resolve a revision that names a commit"),
            repo.rev_parse("HEAD"),
            "the reader has to agree with git about where HEAD points"
        );
    }

    /// The heading the guard inventory lives under. Named once because the
    /// tests below have to agree on which section of the README is the
    /// inventory before they can agree on what it says.
    const INVENTORY_HEADING: &str = "## What it guarantees";

    /// The heading the README's account of this suite lives under. It is cut
    /// with the same helper as the guard inventory because it is the same kind
    /// of claim: a section written as an exhaustive list, which is worth
    /// exactly as much as the list is complete.
    const TESTING_HEADING: &str = "## Testing";

    /// The one section a heading opens, cut out of the document around it.
    ///
    /// The checks below are claims about one section, so they must read one
    /// section, or "named in the inventory" degrades into "mentioned somewhere
    /// in the README". A scope that cannot be trusted — a renamed heading, an
    /// emptied section, one nothing closes — panics rather than being returned.
    ///
    /// The bounds come from a CommonMark parse, because "is this line a
    /// heading?" is a question about syntax and every lexical answer to it has
    /// been wrong here: `\n## ` missed a heading demoted by one character, then
    /// `starts_with('#')` cut this README's `## Testing` nineteen lines early
    /// at a wrapped `#329`, and a fenced `# comment` and a setext heading are
    /// two more spellings a `#` matcher reads wrong. A parse settles all four
    /// at once, and the tests below hold one fixture for each of them.
    fn section_under<'a>(document: &'a str, heading: &str) -> &'a str {
        // The line a source offset opens, `\n` included, so summing it with the
        // offset lands on the character after the heading.
        let line_at = |offset: usize| -> &'a str {
            document
                .get(offset..)
                .expect("a parser reports offsets at character boundaries")
                .split_inclusive('\n')
                .next()
                .unwrap_or_default()
        };

        let headings: Vec<usize> = Parser::new(document)
            .into_offset_iter()
            .filter_map(|(event, span)| match event {
                Event::Start(Tag::Heading { .. }) => Some(span.start),
                _ => None,
            })
            .collect();

        let opens = headings
            .iter()
            .position(|&offset| line_at(offset).trim_end() == heading)
            .unwrap_or_else(|| {
                panic!(
                    "the README has no `{heading}` section, so there is nothing left for this \
                     check to read; a renamed or deleted heading has to fail here rather than \
                     let this test pass by finding nothing"
                )
            });
        let opened_at = headings[opens];
        let closed_at = *headings.get(opens + 1).unwrap_or_else(|| {
            panic!(
                "the `{heading}` section runs to the end of the document with no heading after \
                 it, so nothing bounds it and its scope would silently become the whole rest of \
                 the file; a check that is supposed to be asking about the `{heading}` section \
                 cannot be handed everything below it"
            )
        });

        // The heading's own line is left out, so a section of nothing but a
        // heading still reads as empty below.
        let section = document
            .get(opened_at + line_at(opened_at).len()..closed_at)
            .expect("both ends are character boundaries the parser reported");
        assert!(
            !section.trim().is_empty(),
            "the `{heading}` section is empty, which would make every check below succeed \
             against nothing"
        );

        section
    }

    /// The README's `What it guarantees` table is written as an exhaustive
    /// inventory, so a guard missing from it is not a documentation gap but a
    /// false statement about what the harness pins.
    ///
    /// Someone deciding whether to point this crate at their own repository
    /// reads that table and takes it for the whole list. That is the table's
    /// value and also its liability: every row is a promise, and the promise the
    /// reader most needs is the one nobody wrote down. `--literal-pathspecs` is
    /// how this test came to exist. It went into [`Git::safety_config`] as the
    /// guard between a path git printed and the pathspec that path becomes on
    /// the way back in, it was load-bearing enough at the time that removing it
    /// made a `tests/halts.rs` case misclassify a commit as adding nothing to
    /// the new base — the silent skip that throws work away — and it reached the
    /// table in neither the row list nor the prose beneath it. Nothing failed.
    /// The inventory just quietly became a subset. The empty-commit probe stopped
    /// handing paths back to git afterwards, so that mutation reddens nothing
    /// today, which is the other half of the reason this check earns its place: a
    /// guard whose own test has gone quiet still has to be a row.
    ///
    /// So the completeness of the table is checked here rather than maintained
    /// by care. What is asserted is only that: completeness, not correctness.
    /// Whether a row *explains* its guard well is a human's judgement and stays
    /// one — but a guard the config pins and the table never names cannot ship,
    /// and the failure below says which guard went missing.
    ///
    /// The check is scoped to that one section — cut at the next heading of any
    /// level, and refused outright when no heading follows — because a mention
    /// anywhere else in the README is exactly what must not satisfy it: the
    /// finding is that the *inventory* is short, and a stray sentence three
    /// sections away does not lengthen it. Everything that could make the check
    /// vacuous — a renamed heading, an emptied section, one nothing closes, a
    /// config that pins nothing — panics instead of passing, since a guard that
    /// reports clean because it found nothing to look at is worse than no guard
    /// at all.
    #[test]
    fn every_guard_the_safety_config_pins_is_named_in_the_readme_inventory() {
        /// Embedded under `#[cfg(test)]` only, so the README rides in the test
        /// binary and never in anything a consumer ships.
        const README: &str = include_str!("../README.md");

        let inventory = section_under(README, INVENTORY_HEADING);

        // The hooks path is deliberately empty: this runner exists only to be
        // asked what it would pin, and an empty value is what makes the
        // computed-value rule below observable.
        let settings = Git::new(std::env::temp_dir(), "").safety_config();
        assert!(
            !settings.is_empty(),
            "safety_config pins nothing at all, so there is no guard for the inventory to be \
             missing and this test is asserting nothing"
        );

        for argument in settings.iter().filter(|argument| argument.as_str() != "-c") {
            let named = match argument.split_once('=') {
                // `core.hooksPath`'s value is a per-run temporary directory, so
                // no document could quote it and the key alone is the whole
                // promise. Anything else with a computed value lands here too
                // and fails on the key rather than passing on a coincidence,
                // which is the safe direction: a new guard the README has never
                // heard of stops the build instead of slipping past it.
                Some((key, "")) => key,
                // A settled value is part of the guarantee - `gpg.format=ssh`
                // is a different promise from `gpg.format=openpgp` - so the
                // whole `key=value` has to be the thing the table says.
                Some(_) => argument.as_str(),
                // Not a `-c` setting at all, so there is no key to fall back
                // to: the option is its own name. `--literal-pathspecs` today.
                None => argument.as_str(),
            };

            assert!(
                inventory.contains(named),
                "`{named}` is pinned by safety_config and named nowhere in the README's \
                 `{INVENTORY_HEADING}` inventory. That table is what someone reads to decide \
                 whether this harness is safe to point at their real repository, and they read \
                 it as the complete list; a guard missing from it is a guarantee they cannot \
                 know they have, and its absence from the list reads as its absence from the \
                 harness. Add a row for it, or remove it from safety_config."
            );
        }
    }

    /// The scope the inventory check runs in has to end at the next heading of
    /// *any* level, because a heading is not required to stay the level it was
    /// written at.
    ///
    /// Demote `## Testing` to `### Testing` — one character — and a cut that
    /// ends only at the level it was told about silently grows the section to
    /// swallow everything under it. That is not an abstract widening. The prose
    /// below this table names `--literal-pathspecs` and `core.hooksPath`, the
    /// exact two guards the check matches by bare name, so a swallowed Testing
    /// section makes both of them satisfiable without a row ever existing. The
    /// check would go on reporting clean while checking nothing, which is the
    /// failure that costs the most to notice: an over-wide scope never says a
    /// word.
    ///
    /// [`section_under`] gets this for free, since every level is a heading
    /// to a CommonMark parser, and what is pinned here is that it is *asked*
    /// about every level rather than filtered back down to one. So each level
    /// gets its own fixture rather than the one that prompted this, and each
    /// fixture's stray sentence is the sentence that would do the damage.
    ///
    /// The last fixture is a setext heading — `Testing` over a rule of dashes —
    /// and it is the spelling that tells a parse apart from a matcher. A
    /// corrected `#` matcher reads every level above correctly and still reads
    /// this one wrong, because a setext heading opens with no `#` at all. Put
    /// `.filter(|&offset| document[offset..].starts_with('#'))` on the heading
    /// collection and the levels above stay green while this fixture fails, so
    /// this fixture is what keeps the parse from becoming a lexical cut again.
    #[test]
    fn the_inventory_section_stops_at_the_next_heading_of_any_level() {
        const STRAY_GUARD: &str = "--literal-pathspecs";

        for (spelling, prefix, suffix) in [
            ("`# Testing`", "# ", ""),
            ("`### Testing`", "### ", ""),
            ("`#### Testing`", "#### ", ""),
            ("setext `Testing` over a rule of dashes", "", "\n-------"),
        ] {
            let document = format!(
                "# gitscratch\n\n{INVENTORY_HEADING}\n\n\
                 | Guard | Why |\n| --- | --- |\n\
                 | `gc.auto=0` | A gc could collect a loose simulated commit. |\n\n\
                 {prefix}Testing{suffix}\n\n\
                 The suite pins the {STRAY_GUARD} guard by mutation.\n\n\
                 ## Used by\n\ngrist.\n"
            );

            let inventory = section_under(&document, INVENTORY_HEADING);

            assert!(
                !inventory.contains(STRAY_GUARD),
                "{spelling} ends the `{INVENTORY_HEADING}` section as surely as a `## ` heading \
                 does, so the prose under it is outside the inventory; swallowing it lets a \
                 sentence stand in for the row `{STRAY_GUARD}` needs: {inventory}"
            );
            assert!(
                inventory.contains("gc.auto=0"),
                "the cut at {spelling} has to keep the table it is scoping to, or the check below \
                 would pass against nothing for the opposite reason: {inventory}"
            );
        }
    }

    /// An inventory section with no heading after it is refused, not read to
    /// the end of the file.
    ///
    /// Falling back to "everything below the heading" is the same widening as
    /// missing the cut, arrived at from the other side, and it is the one the
    /// old fallback took silently. A section that is last in the document has
    /// no boundary this check can trust, and a scope nobody bounded is a scope
    /// that will match the first sentence that happens to say the right words.
    /// Refusing says so; returning the rest of the file says nothing and passes.
    #[test]
    #[should_panic(expected = "runs to the end of the document with no heading after it")]
    fn an_inventory_section_that_nothing_closes_is_refused_rather_than_run_to_the_end() {
        let document = format!(
            "# gitscratch\n\n{INVENTORY_HEADING}\n\n\
             | Guard | Why |\n| --- | --- |\n\
             | `gc.auto=0` | A gc could collect a loose simulated commit. |\n\n\
             The suite pins the --literal-pathspecs guard by mutation.\n"
        );

        section_under(&document, INVENTORY_HEADING);
    }

    /// A `#` is not a heading, and the cut has to know the difference.
    ///
    /// This README wraps at eighty columns, so `Issue #329` broke across a line
    /// and left `#329` as the first word of one — a `#` run with no space after
    /// it, which opens no heading in any markdown dialect. A cut that stops at
    /// any line starting with `#` stopped there, nineteen lines early, and both
    /// halves of that are bad. The mild half is the narrowing: a test named in
    /// the tail of the section reads as named nowhere. The severe half is that
    /// the bogus boundary *satisfies* the refusal above — a section nothing
    /// closes still finds a `#` to end at, so the one guard against an unbounded
    /// scope reports clean while the scope is unbounded, which is the exact
    /// false green that refusal was written for.
    ///
    /// Fenced code is the same mistake with a different spelling: `# a comment`
    /// inside a ```` ```sh ```` block is shell, not a section boundary. Each
    /// spelling gets its own fixture, because enumerating spellings is what this
    /// is meant to stop. The spelling that runs the other way — a heading that
    /// carries no `#` — is a fixture in
    /// [`the_inventory_section_stops_at_the_next_heading_of_any_level`].
    #[test]
    fn a_hash_that_is_not_a_heading_does_not_end_a_section() {
        // Placed after the stray `#` in every fixture, so a section cut there
        // loses it and a section cut at a real heading keeps it.
        const KEPT: &str = "gc.auto=0";

        let wrapped = format!(
            "# gitscratch\n\n{INVENTORY_HEADING}\n\n\
             Issue\n#329 tracks growing the suite to eight guarantees.\n\n\
             | Guard | Why |\n| --- | --- |\n\
             | `{KEPT}` | A gc could collect a loose simulated commit. |\n\n\
             ## Used by\n\ngrist.\n"
        );
        let fenced = format!(
            "# gitscratch\n\n{INVENTORY_HEADING}\n\n\
             ```sh\n# not a heading, a shell comment\ngit gc --auto\n```\n\n\
             | Guard | Why |\n| --- | --- |\n\
             | `{KEPT}` | A gc could collect a loose simulated commit. |\n\n\
             ## Used by\n\ngrist.\n"
        );

        for (spelling, document) in [
            ("a wrapped `#329`", &wrapped),
            ("a `#` comment inside a fenced block", &fenced),
        ] {
            let inventory = section_under(document, INVENTORY_HEADING);

            assert!(
                inventory.contains(KEPT),
                "{spelling} opens no heading, so it cannot end the `{INVENTORY_HEADING}` \
                 section; cutting there hands back a scope missing everything the section says \
                 below it, and a row that is present reads as a row that is absent: {inventory}"
            );
        }

        // The same non-heading `#`, with nothing after the section at all. The
        // refusal above has to fire; a cut at `#329` used to stand in for the
        // heading it needs and let an unbounded scope pass for a bounded one.
        let unclosed = format!(
            "# gitscratch\n\n{INVENTORY_HEADING}\n\n\
             Issue\n#329 tracks growing the suite to eight guarantees.\n\n\
             | Guard | Why |\n| --- | --- |\n\
             | `{KEPT}` | A gc could collect a loose simulated commit. |\n"
        );

        let refusal = std::panic::catch_unwind(|| section_under(&unclosed, INVENTORY_HEADING))
            .expect_err(
                "nothing closes this section, so it has to be refused; a `#` that opens no \
                 heading must not be allowed to pass for the boundary that is missing, or the \
                 one check standing between this scope and the rest of the file is satisfied by \
                 a line of prose",
            );
        let message = refusal
            .downcast_ref::<String>()
            .map_or("", |panicked| panicked.as_str());

        assert!(
            message.contains("runs to the end of the document with no heading after it"),
            "the refusal has to be the unbounded-scope one, since that is the guarantee at \
             stake; failing for some other reason would leave it untested: {message}"
        );
    }

    /// Every test this file defines, named the way the compiler sees it.
    ///
    /// Read off the source text because nothing in Rust hands a test its own
    /// suite: `cargo test` knows the list, and no code running inside the test
    /// binary can ask for it. The file is available as a string instead, and a
    /// string is enough — the attribute is spelled the same way every time and
    /// the name follows it.
    ///
    /// The match is whole-line rather than a search for the attribute anywhere,
    /// and that is not fussiness. This scanner's own source carries the
    /// attribute as a string *literal*, so a `contains` would count the constant
    /// below as a test and report a suite one larger than the file has —
    /// self-reference turning a guard into a liar about the very file it is
    /// reading. Only a line whose trimmed content *is* the attribute counts, and
    /// the name is taken from the next line that opens a function, so the
    /// `#[should_panic]` sitting between the two on one of these tests does not
    /// break the pairing. Anything that would make the scan quietly short — an
    /// attribute with no function under it, a signature no name can be read out
    /// of — panics instead of being skipped, because a scanner that finds
    /// nothing looks exactly like a suite that is fully documented.
    fn unit_test_names(source: &str) -> Vec<&str> {
        const TEST_ATTRIBUTE: &str = "#[test]";

        let mut lines = source.lines();
        let mut names = Vec::new();

        while let Some(line) = lines.next() {
            if line.trim() != TEST_ATTRIBUTE {
                continue;
            }

            let signature = lines
                .by_ref()
                .find(|candidate| candidate.trim_start().starts_with("fn "))
                .unwrap_or_else(|| {
                    panic!(
                        "a `{TEST_ATTRIBUTE}` in this file has no function under it, so there is \
                         no name to check the README against and the scan would hand back a list \
                         shorter than the suite"
                    )
                });

            let named = signature
                .trim_start()
                .strip_prefix("fn ")
                .and_then(|rest| rest.split_once('('))
                .map(|(name, _)| name.trim())
                .unwrap_or_else(|| {
                    panic!(
                        "`{signature}` follows a `{TEST_ATTRIBUTE}` but is not a signature this \
                         scan can read a test name out of"
                    )
                });

            names.push(named);
        }

        names
    }

    /// The README's `## Testing` section is written as an exhaustive inventory
    /// of what this file pins, so a test it never names is not a documentation
    /// gap but a false statement about what is covered.
    ///
    /// This is the second inventory in this README to drift out from under its
    /// own prose. The **What it guarantees** table went short first, which is
    /// why the test above it exists; then this section went on describing four
    /// unit tests here through two commits that added two more, and its ordinal
    /// framing — "the fourth is about this document" — quietly stopped counting
    /// anything real. Both drifts were found by a reader, which is the expensive
    /// way: a list that is merely *maintained* is correct only until the next
    /// person forgets, and nothing about forgetting announces itself.
    ///
    /// So the same treatment. Every `fn` under a test attribute in this file
    /// must be named verbatim in that section. What is asserted is completeness,
    /// not quality: whether a sentence *describes* its test well stays a human's
    /// judgement, but a test this file runs and that section never mentions
    /// cannot ship.
    ///
    /// Three of the tests this pins are the ones the section's own thesis rests
    /// on. It claims the guard inventory is "checked, not merely maintained",
    /// and that claim is only safe to believe because the scope the check runs
    /// in is itself pinned — so those three, of all of them, are the ones that
    /// must not go unnamed.
    ///
    /// Every way this could pass while checking nothing is closed off: the
    /// section is cut by the same helper that refuses a renamed heading, an
    /// emptied section and one nothing closes, and cut from a parse, so the
    /// `#329` this very section wraps onto a line of its own is prose rather
    /// than a boundary; a scan that found no tests at all fails; and a scan that
    /// found tests but not a test known to be in this file fails too, since a
    /// matcher quietly reduced to finding some other shape would otherwise sail
    /// through against a short list of its own making.
    #[test]
    fn every_unit_test_in_this_file_is_named_in_the_readme_testing_section() {
        /// Embedded under `#[cfg(test)]` only, so the README rides in the test
        /// binary and never in anything a consumer ships.
        const README: &str = include_str!("../README.md");
        /// This file, read back as text. A file including itself as a *string*
        /// is not recursion — `include_str!` never asks the compiler to expand
        /// anything, it just embeds the bytes on disk.
        const SOURCE: &str = include_str!("git.rs");
        /// A test that is certainly in this file, so a scan that silently found
        /// the wrong shape fails instead of passing against its own empty list.
        const KNOWN_TEST: &str =
            "every_guard_the_safety_config_pins_is_named_in_the_readme_inventory";

        let section = section_under(README, TESTING_HEADING);
        let names = unit_test_names(SOURCE);

        assert!(
            !names.is_empty(),
            "no tests were found in `src/git.rs` at all, which cannot be true of the file this \
             very test is defined in; the scan is broken, and a broken scan reports a perfectly \
             documented suite"
        );
        assert!(
            names.contains(&KNOWN_TEST),
            "`{KNOWN_TEST}` is defined in this file and the scan did not find it, so whatever \
             the scan is matching is not tests; the names it did find are {names:?}"
        );

        for name in &names {
            assert!(
                section.contains(name),
                "`{name}` is a unit test in `src/git.rs` and is named nowhere in the README's \
                 `{TESTING_HEADING}` section. That section is written as the complete account of \
                 what this suite covers, and a reader deciding whether this harness is safe to \
                 point at their real repository takes it for one; a test missing from it is \
                 coverage they cannot know they have. Describe it there, or delete the test."
            );
        }
    }

    /// The `GIT_*` variables this process holds, named and valued, for a failure
    /// message. Reports their absence just as plainly: a mismatch with nothing
    /// inherited means something other than the environment outranked the pin,
    /// and that is a different bug worth saying out loud.
    ///
    /// Applies [`shed_inherited_git_environment`]'s own rule — the `GIT_`
    /// prefix, read off [`std::env::vars_os`] — rather than a list of its own,
    /// so the diagnosis names the variables the runner actually acts on. A list
    /// here would go stale the first time the rule widened, and it would go
    /// stale silently: the message would simply stop mentioning the variable
    /// that caused the failure it is trying to explain. That is how the six
    /// attribution names used to be spelled here, and it is exactly the shape
    /// this crate stopped trusting.
    fn inherited_git_environment() -> String {
        let mut held: Vec<String> = std::env::vars_os()
            .filter(|(key, _)| key.to_string_lossy().starts_with(GIT_ENVIRONMENT_PREFIX))
            .map(|(key, value)| format!("{}={}", key.to_string_lossy(), value.to_string_lossy()))
            .collect();
        held.sort();

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
            .run("var", &["GIT_AUTHOR_IDENT"])
            .expect("git var GIT_AUTHOR_IDENT");

        let expected = format!("{HARNESS_NAME} <{HARNESS_EMAIL}>");
        assert!(
            ident.starts_with(&expected),
            "scratch commits must be authored by the crate, not by a consumer.\n  \
             expected:    {expected}\n  \
             got:         {ident}\n  \
             inherited:   {}\n\
             An identity variable outranks every config source, `-c` included, so \
             NoInheritedGitEnvironment removes every `GIT_` variable and \
             Git::command pins the four name and email variables back to the \
             harness before git is spawned. git exports them into every hook it \
             runs, and into every commit that rebase, cherry-pick, or am \
             replays.",
            inherited_git_environment()
        );
    }

    /// The identity holds in an environment that carries nothing of its own,
    /// which is how the suite runs from a shell.
    #[test]
    fn commits_under_the_crate_s_own_identity_not_a_consuming_tool_s() {
        assert_scratch_identity();
    }

    /// Printed by a child half that ran its assertions and returned, and
    /// required by [`run_child_half`] before it calls the run a pass.
    ///
    /// A zero exit is not evidence that anything ran. libtest exits 0 when a
    /// filter matches no test, so a filter gone stale hands the parent exactly
    /// the exit status a passing child hands it. Only a child that reached the
    /// end of its body prints this line, so the parent reads proof of work
    /// instead of absence of failure. `--nocapture` is already on the child's
    /// command line, so the line arrives in the child's stdout with no more
    /// plumbing.
    ///
    /// The value is a token no libtest output holds, because the parent looks
    /// for it in the whole of that output. A sentinel that a test name or a
    /// progress line could spell would report the work of libtest as the work
    /// of the child.
    const CHILD_RAN: &str = "GITSCRATCH_CHILD_HALF_RAN";

    /// Re-execute this test binary on one test, under an environment
    /// `configure` sets, and report what the child wrote when the run failed.
    ///
    /// Two tests here need an environment of their own, and an environment is
    /// process-wide: `std::env::set_var` in either one reaches every sibling
    /// test in the binary and every concurrent run of the suite. So each of
    /// them re-executes this binary with `marker` set, and its own child branch
    /// recognises the marker and does the asserting.
    ///
    /// The two parents differ in nothing but the environment they set, so one
    /// helper decides what counts as a run rather than two copies of the same
    /// `Command` construction deciding it apart from each other. The
    /// environment arrives as a closure because the two spell their values
    /// differently: the hook test holds `&str` and the redirected test holds
    /// `PathBuf`.
    ///
    /// A run counts only when the child exits 0 **and** prints
    /// [`CHILD_RAN`](CHILD_RAN). The second half is the whole point of the one
    /// helper: `filter` is a string, nothing ties it to the test it names, and
    /// a rename leaves it matching nothing - which libtest reports as a
    /// success. See
    /// [`a_child_half_that_matched_no_test_is_a_failure_not_a_pass`].
    fn run_child_half(
        marker: &str,
        filter: &str,
        configure: impl FnOnce(&mut std::process::Command),
    ) -> Result<(), String> {
        let mut child = std::process::Command::new(
            std::env::current_exe().expect("path of the running test binary"),
        );
        child
            .args([filter, "--exact", "--nocapture"])
            .env(marker, "1");
        configure(&mut child);

        let output = child.output().expect("re-run this test binary");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Err(format!("the child half failed:\n{stdout}{stderr}"));
        }

        if !stdout.contains(CHILD_RAN) {
            return Err(format!(
                "`{filter}` matched no test, so the child exited 0 with nothing run and this \
                 guard checked nothing. libtest calls an empty filter a success, so the exit \
                 status cannot tell the two apart; the child prints `{CHILD_RAN}` when it \
                 reaches the end of its body, and it printed nothing. Point the filter back at \
                 the test it names.\n{stdout}{stderr}"
            ));
        }

        Ok(())
    }

    /// Marks the child of
    /// [`a_child_half_that_matched_no_test_is_a_failure_not_a_pass`]. Nothing
    /// ever reads it: the filter that child runs under matches no test, so no
    /// child branch runs to look for it.
    const UNMATCHED_CHILD_MARKER: &str = "GITSCRATCH_UNMATCHED_FILTER_CHILD";

    /// A libtest filter naming a test this file does not define, which is what
    /// a renamed test looks like from the parent's side.
    const UNMATCHED_TEST_PATH: &str = "git::tests::no_test_in_this_file_carries_this_name";

    /// A filter is a string, and nothing ties a string to the test it names.
    /// Rename the test, the `tests` module or the `git` module and the filter
    /// stays as it was, so the child matches nothing — and libtest exits 0 when
    /// a filter matches nothing. A parent that reads the exit status alone
    /// therefore calls the rename a pass, and the two guards below check
    /// nothing from that commit on, in silence.
    ///
    /// So [`run_child_half`] has to refuse a child that ran no test, and this
    /// pins that it does. The rename that breaks a filter is the rename that
    /// hides the breakage, which is why the refusal cannot live in the filter
    /// constants themselves.
    #[test]
    fn a_child_half_that_matched_no_test_is_a_failure_not_a_pass() {
        let outcome = run_child_half(UNMATCHED_CHILD_MARKER, UNMATCHED_TEST_PATH, |_| {});

        let report = outcome.expect_err(
            "a child that matched no test must be a failure: libtest exits 0 on an empty filter, \
             so accepting that exit means a renamed test reports a pass while nothing runs",
        );
        assert!(
            report.contains(UNMATCHED_TEST_PATH),
            "the refusal has to name the filter that matched nothing, because the filter is the \
             thing that went stale: {report}"
        );
        assert!(
            report.contains("matched no test"),
            "the refusal has to say what went wrong - a child that ran nothing, not a child that \
             failed - or a reader repairs the wrong half: {report}"
        );
    }

    /// Marks the re-executed child half of
    /// [`the_pinned_identity_survives_a_hook_environment`].
    const CHILD_MARKER: &str = "GITSCRATCH_HOOK_ENVIRONMENT_CHILD";

    /// libtest's exact filter for the one test the child half runs. The
    /// compiler never checks this string against the test it names, and a
    /// rename that leaves it matching nothing is a run libtest calls a success,
    /// so [`run_child_half`] requires the child to say it ran rather than
    /// trusting this constant to stay current.
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
            // Reached only when the assertion above held, and read by the
            // parent as the one proof that this branch ran at all.
            println!("{CHILD_RAN}");
            return;
        }

        let outcome = run_child_half(CHILD_MARKER, HOOK_TEST_PATH, |child| {
            for (name, value) in HOOK_ENVIRONMENT {
                child.env(name, value);
            }
        });

        if let Err(report) = outcome {
            panic!("the hook-environment guard did not report a pass:\n{report}");
        }
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
    /// Both halves are asserted together, in one test, because they are one
    /// leak arriving by one route.
    ///
    /// The environment belongs to a re-executed child of this test binary, for
    /// the same reason [`the_pinned_identity_survives_a_hook_environment`]
    /// re-executes: `std::env::set_var` mutates a process-wide global while the
    /// rest of the suite is running in sibling threads, so the mutation this
    /// test needs would reach every other test in the binary and every
    /// concurrent run of it.
    #[test]
    fn ignores_an_inherited_git_environment_naming_another_identity_or_repository() {
        if std::env::var_os(REDIRECTED_CHILD_MARKER).is_some() {
            assert_the_inherited_environment_is_ignored();
            // Reached only when the assertions above held, and read by the
            // parent as the one proof that this branch ran at all.
            println!("{CHILD_RAN}");
            return;
        }

        // Stands in for the developer's real repository - the place a leaked
        // environment would redirect the replay to. It is built here, in the
        // parent, and outlives the child because the parent blocks on it.
        let elsewhere = TempDir::new().expect("create the stand-in for a real repository");

        let outcome = run_child_half(REDIRECTED_CHILD_MARKER, REDIRECTED_TEST_PATH, |child| {
            child
                .env("GIT_AUTHOR_NAME", "A Developer")
                .env("GIT_AUTHOR_EMAIL", "developer@example.com")
                .env("GIT_COMMITTER_NAME", "A Developer")
                .env("GIT_COMMITTER_EMAIL", "developer@example.com")
                .env("GIT_DIR", elsewhere.path().join("their-repo.git"))
                .env("GIT_WORK_TREE", elsewhere.path())
                .env("GIT_INDEX_FILE", elsewhere.path().join(THEIR_INDEX));
        });

        if let Err(report) = outcome {
            panic!("the inherited-environment guard did not report a pass:\n{report}");
        }
    }

    /// Marks the re-executed child half of
    /// [`ignores_an_inherited_git_environment_naming_another_identity_or_repository`].
    const REDIRECTED_CHILD_MARKER: &str = "GITSCRATCH_REDIRECTED_ENVIRONMENT_CHILD";

    /// libtest's exact filter for the one test the child half runs. The
    /// compiler never checks this string against the test it names, and a
    /// rename that leaves it matching nothing is a run libtest calls a success,
    /// so [`run_child_half`] requires the child to say it ran rather than
    /// trusting this constant to stay current.
    const REDIRECTED_TEST_PATH: &str =
        "git::tests::ignores_an_inherited_git_environment_naming_another_identity_or_repository";

    /// The index file the inherited `GIT_INDEX_FILE` names. Spelled once
    /// because the parent sets it and the child asserts against it, and a
    /// second spelling that drifted would leave the child looking for a name
    /// nothing ever sets - which passes.
    const THEIR_INDEX: &str = "their-index";

    /// The child half: run under an environment naming another identity and
    /// another repository, and pin that neither reached git.
    fn assert_the_inherited_environment_is_ignored() {
        let here = TempDir::new().expect("create the scratch stand-in");
        let git = Git::new(here.path(), "");
        git.run("init", &["-q", "-b", "main"])
            .expect("initialise the repository the runner is rooted in");

        for variable in ["GIT_AUTHOR_IDENT", "GIT_COMMITTER_IDENT"] {
            let ident = git
                .run("var", &[variable])
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
            .path("rev-parse", &["--absolute-git-dir"])
            .expect("ask git which repository it is operating on");
        assert!(
            std::fs::canonicalize(&git_dir)
                .expect("canonicalise git's answer")
                .starts_with(&expected),
            "the runner must operate on the repository it is rooted in ({}), not the one an \
             inherited GIT_DIR names ({})",
            expected.display(),
            git_dir.display()
        );

        let index = git
            .path("rev-parse", &["--git-path", "index"])
            .expect("ask git which index it would write");
        assert!(
            !index.to_string_lossy().contains(THEIR_INDEX),
            "an inherited GIT_INDEX_FILE must not become the index a replay stages into: {}",
            index.display()
        );
    }
}
