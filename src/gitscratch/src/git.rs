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
    use pulldown_cmark::{Event, Parser, Tag};
    use tempfile::TempDir;

    use super::{Git, HARNESS_EMAIL, HARNESS_NAME, INHERITED_ATTRIBUTION};
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
    /// at a wrapped `#329`, with a fenced `# comment` and a setext heading two
    /// more spellings still to come. A parse settles all four at once.
    fn inventory_section<'a>(document: &'a str, heading: &str) -> &'a str {
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
                    "the README has no `{heading}` section, so there is no inventory left to \
                     check the guards against; a renamed or deleted heading has to fail here \
                     rather than let this test pass by finding nothing"
                )
            });
        let opened_at = headings[opens];
        let closed_at = *headings.get(opens + 1).unwrap_or_else(|| {
            panic!(
                "the `{heading}` section runs to the end of the document with no heading after \
                 it, so nothing bounds the inventory and its scope would silently become the \
                 whole rest of the file; a check that is supposed to be asking about one table \
                 cannot be handed everything below it"
            )
        });

        // The heading's own line is left out, so an inventory of nothing but a
        // heading still reads as empty below.
        let inventory = document
            .get(opened_at + line_at(opened_at).len()..closed_at)
            .expect("both ends are character boundaries the parser reported");
        assert!(
            !inventory.trim().is_empty(),
            "the `{heading}` section is empty, which would make every check below succeed \
             against nothing"
        );

        inventory
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
    /// the way back in, it is load-bearing enough that removing it makes a
    /// `tests/halts.rs` case misclassify a commit as adding nothing to the new
    /// base — the silent skip that throws work away — and it reached the table
    /// in neither the row list nor the prose beneath it. Nothing failed. The
    /// inventory just quietly became a subset.
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

        let inventory = inventory_section(README, INVENTORY_HEADING);

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
    /// [`inventory_section`] gets this for free, since every level is a heading
    /// to a CommonMark parser, and what is pinned here is that it is *asked*
    /// about every level rather than filtered back down to one. So each level
    /// gets its own fixture rather than the one that prompted this, and each
    /// fixture's stray sentence is the sentence that would do the damage.
    #[test]
    fn the_inventory_section_stops_at_the_next_heading_of_any_level() {
        const STRAY_GUARD: &str = "--literal-pathspecs";

        for level in ["# ", "### ", "#### "] {
            let document = format!(
                "# gitscratch\n\n{INVENTORY_HEADING}\n\n\
                 | Guard | Why |\n| --- | --- |\n\
                 | `gc.auto=0` | A gc could collect a loose simulated commit. |\n\n\
                 {level}Testing\n\n\
                 The suite pins the {STRAY_GUARD} guard by mutation.\n\n\
                 ## Used by\n\ngrist.\n"
            );

            let inventory = inventory_section(&document, INVENTORY_HEADING);

            assert!(
                !inventory.contains(STRAY_GUARD),
                "a `{level}` heading ends the `{INVENTORY_HEADING}` section as surely as a `## ` \
                 one does, so the prose under it is outside the inventory; swallowing it lets a \
                 sentence stand in for the row `{STRAY_GUARD}` needs: {inventory}"
            );
            assert!(
                inventory.contains("gc.auto=0"),
                "the cut has to keep the table it is scoping to, or the check below would pass \
                 against nothing for the opposite reason: {inventory}"
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

        inventory_section(&document, INVENTORY_HEADING);
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
    /// is meant to stop.
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
            let inventory = inventory_section(document, INVENTORY_HEADING);

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

        let refusal = std::panic::catch_unwind(|| inventory_section(&unclosed, INVENTORY_HEADING))
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

        let section = inventory_section(README, TESTING_HEADING);
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

    /// The identity variables this process holds, named and valued, for a
    /// failure message. Reports their absence just as plainly: a mismatch with
    /// nothing inherited means something other than the environment outranked
    /// the pin, and that is a different bug worth saying out loud.
    ///
    /// Reads [`INHERITED_ATTRIBUTION`] rather than a list of its own, so the
    /// diagnosis names the variables the runner actually acts on. A second copy
    /// here would go stale the first time that list changed, and it would go
    /// stale silently: the message would simply stop mentioning the variable
    /// that caused the failure it is trying to explain.
    fn inherited_identity() -> String {
        let held: Vec<String> = INHERITED_ATTRIBUTION
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
            return;
        }

        // Stands in for the developer's real repository - the place a leaked
        // environment would redirect the replay to. It is built here, in the
        // parent, and outlives the child because the parent blocks on it.
        let elsewhere = TempDir::new().expect("create the stand-in for a real repository");

        let mut child = std::process::Command::new(
            std::env::current_exe().expect("path of the running test binary"),
        );
        child
            .args([REDIRECTED_TEST_PATH, "--exact", "--nocapture"])
            .env(REDIRECTED_CHILD_MARKER, "1")
            .env("GIT_AUTHOR_NAME", "A Developer")
            .env("GIT_AUTHOR_EMAIL", "developer@example.com")
            .env("GIT_COMMITTER_NAME", "A Developer")
            .env("GIT_COMMITTER_EMAIL", "developer@example.com")
            .env("GIT_DIR", elsewhere.path().join("their-repo.git"))
            .env("GIT_WORK_TREE", elsewhere.path())
            .env("GIT_INDEX_FILE", elsewhere.path().join(THEIR_INDEX));

        let output = child.output().expect("re-run this test binary");

        assert!(
            output.status.success(),
            "an inherited git environment reached the runner:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Marks the re-executed child half of
    /// [`ignores_an_inherited_git_environment_naming_another_identity_or_repository`].
    const REDIRECTED_CHILD_MARKER: &str = "GITSCRATCH_REDIRECTED_ENVIRONMENT_CHILD";

    /// libtest's exact filter for the one test the child half runs.
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
            !index.contains(THEIR_INDEX),
            "an inherited GIT_INDEX_FILE must not become the index a replay stages into: {index}"
        );
    }
}
