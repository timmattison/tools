//! Throwaway git repositories with known conflict shapes.
//!
//! Every tool built on `gitscratch` needs the same handful of repositories to
//! test against — a contested region, a stacked branch, a pair of branches that
//! tie on hunks but not on stops — so they live here once instead of being
//! rebuilt per crate and drifting apart.
//!
//! Every repo lives in its own `TempDir`, so concurrent `cargo test` runs (the
//! pre-commit hook's and yours) never share a path.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// A throwaway repository that deletes itself when dropped.
pub struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    /// Initialise a repo with `main` checked out and identity configured.
    ///
    /// # Panics
    ///
    /// Panics if the temporary directory cannot be created, if `git` is not
    /// installed, or if any of the setup commands fail.
    pub fn init() -> Self {
        let dir = TempDir::new().expect("create temp dir");
        let repo = Self { dir };

        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.email", "gitscratch@example.com"]);
        repo.git(&["config", "user.name", "gitscratch test"]);
        // A commit-signing config in the developer's global gitconfig would
        // otherwise make every fixture commit prompt or fail.
        repo.git(&["config", "commit.gpgsign", "false"]);

        repo
    }

    /// The repository's working directory.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Run a git command in the repo, panicking on failure.
    ///
    /// # Panics
    ///
    /// Panics if `git` cannot be spawned or exits non-zero; the panic message
    /// carries the command's stdout and stderr.
    pub fn git(&self, args: &[&str]) -> String {
        self.git_in(self.dir.path(), args)
    }

    fn git_in(&self, cwd: &Path, args: &[&str]) -> String {
        let mut command = Command::new("git");
        // The same immunity the runner has, from the same list, because a
        // fixture is not exempt from an inherited environment just because it is
        // only building something to test with. A test suite run from inside a
        // git hook inherits a *relative* `GIT_INDEX_FILE`, and a linked
        // worktree's `.git` is a file, so a fixture that keeps it cannot add one
        // at all - it fails before the code under test is ever reached.
        crate::git::shed_inherited_git_environment(&mut command);

        let output = command
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn git {args:?}: {e}"));

        assert!(
            output.status.success(),
            "git {args:?} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Write `contents` to `name` and commit it.
    ///
    /// # Panics
    ///
    /// Panics if the fixture file cannot be written or if git fails.
    pub fn commit_file(&self, name: &str, contents: &str, message: &str) {
        self.commit_files(&[(name, contents)], message);
    }

    /// Write every `(name, contents)` pair and land them in one commit, so a
    /// fixture can control whether an edit set arrives as one commit or several.
    ///
    /// # Panics
    ///
    /// Panics if a fixture file cannot be written or if git fails.
    pub fn commit_files(&self, files: &[(&str, &str)], message: &str) {
        for (name, contents) in files {
            std::fs::write(self.dir.path().join(name), contents).expect("write fixture file");
            self.git(&["add", name]);
        }
        self.git(&["commit", "-q", "-m", message]);
    }

    /// Create `name` at HEAD and check it out.
    ///
    /// # Panics
    ///
    /// Panics if git fails — most likely because `name` already exists.
    pub fn branch(&self, name: &str) {
        self.git(&["checkout", "-q", "-b", name]);
    }

    /// Check `name` out.
    ///
    /// # Panics
    ///
    /// Panics if git fails — most likely because `name` does not exist.
    pub fn checkout(&self, name: &str) {
        self.git(&["checkout", "-q", name]);
    }

    /// Resolve `reference` to its full object id.
    ///
    /// # Panics
    ///
    /// Panics if git fails — most likely because `reference` does not resolve.
    pub fn rev_parse(&self, reference: &str) -> String {
        self.git(&["rev-parse", reference])
    }

    /// Check `branch` out into a second worktree, the way a real developer
    /// juggling several branches would have it.
    ///
    /// # Panics
    ///
    /// Panics if the worktree path is not valid UTF-8 or if git fails — most
    /// likely because `branch` is already checked out somewhere.
    pub fn add_worktree(&self, branch: &str) -> PathBuf {
        let path = self.dir.path().join(format!("wt-{branch}"));
        self.git(&[
            "worktree",
            "add",
            "-q",
            path.to_str().expect("utf-8 worktree path"),
            branch,
        ]);
        path
    }
}

#[cfg(unix)]
impl TestRepo {
    /// Make the repository's object database unwritable until the returned
    /// guard is dropped, so any git command that has to add an object fails.
    ///
    /// This is the one cause of a failed commit write that is reachable through
    /// this harness. Signing, hooks and the editor — the other everyday ways a
    /// commit fails to be written — are all pinned off by `Git::safety_config`,
    /// and a scratch worktree does not get an object database of its own: it
    /// writes its objects straight into the developer's real one. Sealing that
    /// database is therefore how a test puts a replay in the state where git
    /// halts the rebase with nothing left to merge and the commit *not*
    /// written.
    ///
    /// Only directories are sealed, and only their write bits, so git can still
    /// read and traverse the store — it simply cannot add to it. Every original
    /// mode is restored on drop, which the temporary directory's own removal
    /// depends on.
    ///
    /// # Panics
    ///
    /// Panics if the object database cannot be walked or its permissions cannot
    /// be changed.
    #[must_use]
    pub fn seal_object_store(&self) -> SealedObjectStore {
        let mut restore = Vec::new();
        seal_directories_under(&self.dir.path().join(".git").join("objects"), &mut restore);
        SealedObjectStore { restore }
    }
}

/// A repository object database held read-only for as long as this guard lives.
///
/// Built by [`TestRepo::seal_object_store`], which explains what it is for.
#[cfg(unix)]
pub struct SealedObjectStore {
    /// Every directory sealed, with the mode it had beforehand, in walk order.
    restore: Vec<(PathBuf, std::fs::Permissions)>,
}

#[cfg(unix)]
impl Drop for SealedObjectStore {
    fn drop(&mut self) {
        // Exactly the walk, run backwards. The order is not load-bearing -
        // only write bits were ever touched, so traversal never stopped
        // working - but an unwind that mirrors the walk is one less thing to
        // reason about. Best effort: a panic here would replace whatever
        // failure the test was actually reporting.
        for (path, permissions) in self.restore.drain(..).rev() {
            let _ = std::fs::set_permissions(&path, permissions);
        }
    }
}

/// Strip the write bits from `path` and every directory beneath it, recording
/// what each one had so the guard can put it back.
#[cfg(unix)]
fn seal_directories_under(path: &Path, restore: &mut Vec<(PathBuf, std::fs::Permissions)>) {
    use std::os::unix::fs::PermissionsExt as _;

    let original = std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("read permissions of {}: {e}", path.display()))
        .permissions();

    // Children before their parent, so the root of the walk is recorded last
    // and the guard's reverse unwind starts there.
    for entry in std::fs::read_dir(path)
        .unwrap_or_else(|e| panic!("list {}: {e}", path.display()))
        .flatten()
    {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            seal_directories_under(&entry.path(), restore);
        }
    }

    let mut sealed = original.clone();
    sealed.set_mode(original.mode() & !0o222);
    std::fs::set_permissions(path, sealed)
        .unwrap_or_else(|e| panic!("seal {}: {e}", path.display()));
    restore.push((path.to_path_buf(), original));
}

/// A numbered file with `count` lines, so edits can be placed far enough apart
/// that git's 3-line diff context does not make them overlap by accident.
pub fn numbered_lines(count: usize) -> String {
    (1..=count)
        .map(|n| format!("line{n}\n"))
        .collect::<Vec<_>>()
        .join("")
}

/// Replace the 1-indexed `line` of `numbered_lines`-style content.
///
/// # Panics
///
/// Panics if `line` is zero or beyond the end of `contents`.
pub fn replace_line(contents: &str, line: usize, replacement: &str) -> String {
    let mut lines: Vec<&str> = contents.lines().collect();
    lines[line - 1] = replacement;
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// A branch that rewrites the same region three times, and one that touches it
/// once. Landing the iterated branch second makes each of its three commits
/// collide with the already-squashed change; landing it first means only the
/// single-commit branch has to be replayed.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn contested_region_repo() -> TestRepo {
    const CONTESTED_LINE: usize = 15;

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_file("shared.txt", &base, "base");

    repo.branch("iterated");
    for revision in 1..=3 {
        let contents = replace_line(&base, CONTESTED_LINE, &format!("iterated-v{revision}"));
        repo.commit_file("shared.txt", &contents, &format!("iterate {revision}"));
    }

    repo.checkout("main");
    repo.branch("single");
    let contents = replace_line(&base, CONTESTED_LINE, "single-edit");
    repo.commit_file("shared.txt", &contents, "single edit");

    repo.checkout("main");
    repo
}

/// `built-on-top` was branched from `groundwork`, not from main - the stacked
/// shape that makes squash merging different from a real merge, because
/// squashing `built-on-top` destroys the commit identity of the `groundwork`
/// commits buried inside it.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn stacked_branches_repo() -> TestRepo {
    const CONTESTED_LINE: usize = 15;

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_file("shared.txt", &base, "base");

    repo.branch("groundwork");
    let groundwork = replace_line(&base, CONTESTED_LINE, "groundwork-edit");
    repo.commit_file("shared.txt", &groundwork, "groundwork");

    repo.branch("built-on-top");
    let stacked = replace_line(&groundwork, CONTESTED_LINE, "built-on-top-edit");
    repo.commit_file("shared.txt", &stacked, "built on top");

    repo.checkout("main");
    repo
}

/// Two branches that make the same two edits, packaged differently: `one` lands
/// both in a single commit, `two` splits them across two commits. Every
/// ordering hand-merges the same two hunks in the same two files, but replaying
/// `two`'s two commits halts the rebase twice where `one`'s single commit halts
/// it once - so the orderings tie on hunks and files while differing on stops.
///
/// That is the shape that catches anything treating equal hunks as equal cost.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn equal_hunks_unequal_stops_repo() -> TestRepo {
    const CONTESTED_LINE: usize = 15;

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_files(&[("x.txt", &base), ("y.txt", &base)], "base");

    repo.branch("one");
    repo.commit_files(
        &[
            ("x.txt", &replace_line(&base, CONTESTED_LINE, "one-x")),
            ("y.txt", &replace_line(&base, CONTESTED_LINE, "one-y")),
        ],
        "one edits both files at once",
    );

    repo.checkout("main");
    repo.branch("two");
    repo.commit_file(
        "x.txt",
        &replace_line(&base, CONTESTED_LINE, "two-x"),
        "two edits x",
    );
    repo.commit_file(
        "y.txt",
        &replace_line(&base, CONTESTED_LINE, "two-y"),
        "two edits y",
    );

    repo.checkout("main");
    repo
}

/// Two branches that each add a file of their own and touch nothing else, so no
/// ordering conflicts and every ordering genuinely costs zero of everything.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn independent_branches_repo() -> TestRepo {
    let repo = TestRepo::init();
    repo.commit_file("shared.txt", &numbered_lines(30), "base");

    repo.branch("alpha");
    repo.commit_file("alpha.txt", "alpha work\n", "alpha work");

    repo.checkout("main");
    repo.branch("beta");
    repo.commit_file("beta.txt", "beta work\n", "beta work");

    repo.checkout("main");
    repo
}

/// A branch that modifies the file main deleted, so replaying `branch` onto
/// `main` is a modify/delete conflict.
///
/// That conflict is the shape for testing what happens when git cannot *write*
/// a commit, because its auto-resolution needs no new object: staging the
/// surviving version of `x.txt` stages a blob the object database already
/// holds. So `git add -A` still succeeds against a sealed object store - see
/// [`TestRepo::seal_object_store`] - the replay gets all the way to
/// `rebase --continue`, and the commit write is the only thing that fails,
/// leaving the resolution staged and the rebase halted with nothing unmerged.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn modify_delete_repo() -> TestRepo {
    let repo = TestRepo::init();
    repo.commit_file("x.txt", "base\n", "base");

    repo.branch("branch");
    repo.commit_file("x.txt", "the branch's version\n", "branch modifies x");

    repo.checkout("main");
    repo.git(&["rm", "-q", "x.txt"]);
    repo.git(&["commit", "-q", "-m", "main deletes x"]);

    repo
}

/// Two independent branches, plus a `main` that has moved on since they were
/// cut. Nothing conflicts — each branch owns a file of its own, and main's extra
/// commit touches neither — but neither branch is a fast-forward any more, so
/// replaying one onto `main` has to *write* a commit rather than just move a
/// ref.
///
/// That is what makes a failed commit write observable. The pick applies
/// cleanly, so git has nothing to leave unmerged and nothing to leave staged;
/// when it cannot write the commit it rolls the index back and reschedules the
/// pick. The rebase is then halted with nothing unmerged and nothing dirty
/// anywhere — a state that uncommitted content alone cannot tell apart from a
/// commit that genuinely became empty. Seal the object database with
/// [`TestRepo::seal_object_store`] to reach it.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn branches_behind_main_repo() -> TestRepo {
    let repo = TestRepo::init();
    repo.commit_file("shared.txt", &numbered_lines(30), "base");

    repo.branch("alpha");
    repo.commit_file("alpha.txt", "alpha work\n", "alpha work");

    repo.checkout("main");
    repo.branch("beta");
    repo.commit_file("beta.txt", "beta work\n", "beta work");

    repo.checkout("main");
    repo.commit_file("main.txt", "main moved on\n", "main moves ahead");

    repo
}

/// [`branches_behind_main_repo`]'s shape with the branch's work moved into two
/// paths git will not hand back verbatim: `café.txt`, which git C-quotes into
/// `"caf\303\251.txt"` whenever it prints a path on a line of its own, and
/// ` leading space.txt`, which git prints as it is but which any trimming of
/// that line silently shortens.
///
/// Both are here because a replay reads paths out of one git invocation and
/// feeds them straight back into the next as pathspecs, and git does not dequote
/// a pathspec — so a name that changed on the way out matches nothing on the way
/// back in, and a commit whose work is nowhere in the new base looks like a
/// commit that adds nothing to it. Neither name is plainly spelled,
/// deliberately: one ordinary path in the same commit would come back matching,
/// the probe would find *its* work missing and refuse on that alone, and the
/// silence of the other two would never show.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn branches_behind_main_with_quoted_and_space_led_paths_repo() -> TestRepo {
    let repo = TestRepo::init();
    repo.commit_file("shared.txt", &numbered_lines(30), "base");

    repo.branch("branch");
    repo.commit_files(
        &[
            ("café.txt", "the branch's work\n"),
            (" leading space.txt", "more of the branch's work\n"),
        ],
        "branch work",
    );

    repo.checkout("main");
    repo.commit_file("main.txt", "main moved on\n", "main moves ahead");

    repo
}

/// [`branches_behind_main_repo`]'s shape with the branch's work moved into a
/// path git hands back verbatim and then reads back as something else entirely:
/// `:/foo.txt`, a `foo.txt` inside a directory literally named `:`.
///
/// Nothing is lost on the way out here — the name is plain ASCII, so git neither
/// quotes it nor leaves anything for a trim to eat — and that is the point. The
/// mangling happens on the way back in, because a pathspec is not a path: a
/// leading `:` is pathspec magic, and `:/` specifically means *from the top of
/// the working tree*. Fed back as a pathspec the name therefore asks about the
/// root `foo.txt` instead of the one the commit added, and `foo.txt` at the root
/// is exactly what this fixture puts there — committed in the base, touched by
/// neither side afterwards, and so identical in the replayed commit and the new
/// base. A probe asking whether the commit's work is already in the new base
/// gets an empty diff back, the honest answer about the *other* file, and reads
/// it as yes.
///
/// That points the opposite way from the quoted names in
/// [`branches_behind_main_with_quoted_and_space_led_paths_repo`], which is why
/// it is worth a fixture of its own. A pathspec that matches nothing can only
/// grow the set of paths a probe finds missing, and a bigger set only ever
/// produces a refusal nobody needed; a pathspec that matches the *wrong* file
/// can shrink that set to empty, which is a commit reclassified as adding
/// nothing to the new base, skipped, and gone.
///
/// The branch's commit touches no plainly-spelled path at all, for the same
/// reason that one does not: one ordinary file alongside would come back
/// matching, the probe would find *its* work missing and refuse on that alone,
/// and the magic name's silence would never show.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, the `:` directory or
/// the file inside it not writable, or a command failing.
pub fn branches_behind_main_with_a_pathspec_magic_path_repo() -> TestRepo {
    // The file the branch adds, and - at the repository root, where `:/` sends
    // anything that reads the name as magic - the decoy that answers for it.
    const DECOY: &str = "foo.txt";
    const MAGIC_DIRECTORY: &str = ":";

    let repo = TestRepo::init();
    repo.commit_files(
        &[
            ("shared.txt", &numbered_lines(30)),
            (DECOY, "the file pathspec magic answers about instead\n"),
        ],
        "base",
    );

    repo.branch("branch");
    // Not `commit_files`: it would neither make the `:` directory nor stage what
    // landed in it, because it stages by handing the name to `git add`, where a
    // leading `:` is read as magic exactly as it is everywhere else. Staging
    // this file needs the same literal reading the code under test needs.
    std::fs::create_dir(repo.path().join(MAGIC_DIRECTORY)).expect("create the ':' directory");
    std::fs::write(
        repo.path().join(MAGIC_DIRECTORY).join(DECOY),
        "the branch's work\n",
    )
    .expect("write fixture file");
    let magic_path = format!("{MAGIC_DIRECTORY}/{DECOY}");
    repo.git(&["--literal-pathspecs", "add", "--", &magic_path]);
    repo.git(&["commit", "-q", "-m", "branch work"]);

    repo.checkout("main");
    repo.commit_file("main.txt", "main moved on\n", "main moves ahead");

    repo
}

/// [`conflicting_repo`]'s shape, moved into `café.txt` and stretched to two
/// contested regions: both branches rewrite line 10 and line 22 of the same
/// file, twelve lines apart so git's 3-line diff context cannot merge them into
/// one hunk.
///
/// Two regions rather than one is the whole point. A conflicted file is counted
/// by reading it back off disk by the name git reported, so a name git quoted on
/// the way out names nothing on disk — and the count falls back to the one
/// decision a file with no readable content still costs. One contested region
/// would score one either way; two makes the fallback visible.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn two_region_conflict_in_a_quoted_path_repo() -> TestRepo {
    const FIRST_CONTESTED_LINE: usize = 10;
    const SECOND_CONTESTED_LINE: usize = 22;
    const CONTESTED_FILE: &str = "café.txt";

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_file(CONTESTED_FILE, &base, "base");

    repo.branch("left");
    let left = replace_line(&base, FIRST_CONTESTED_LINE, "left-edit-first");
    let left = replace_line(&left, SECOND_CONTESTED_LINE, "left-edit-second");
    repo.commit_file(CONTESTED_FILE, &left, "left work");

    repo.checkout("main");
    repo.branch("right");
    let right = replace_line(&base, FIRST_CONTESTED_LINE, "right-edit-first");
    let right = replace_line(&right, SECOND_CONTESTED_LINE, "right-edit-second");
    repo.commit_file(CONTESTED_FILE, &right, "right work");

    repo.checkout("main");
    repo
}

/// A branch whose first commit arrives at content `main` has since reached by a
/// different route, followed by a second commit that is real work. Replaying the
/// branch onto `main` empties that first commit while the second one still has
/// to survive — the legitimate half of the halt that a commit git could not write
/// shares.
///
/// The *different route* is what makes the shape work. `main` walks
/// `x1 -> x2 -> x3` in two commits and the branch jumps `x1 -> x3` in one, so no
/// commit on either side shares a patch id with a commit on the other. Without
/// that, git recognises the branch's commit as already upstream and drops it
/// before the rebase ever halts; with it, the commit applies, produces exactly
/// what HEAD already holds, and git stops on it.
///
/// `y.txt` is untouched by `main` and rewritten by the branch's second commit,
/// so a replay that walks the whole rebase leaves `x.txt` at `x3` and `y.txt` at
/// `y2`, and one that gave up somewhere in the middle does not.
///
/// Reaching the stop still takes `--empty=stop` on git's command line — see the
/// test that uses this repo for why nothing else gets there.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn commit_emptied_by_main_repo() -> TestRepo {
    let repo = TestRepo::init();
    repo.commit_files(&[("x.txt", "x1\n"), ("y.txt", "y1\n")], "base");

    repo.branch("branch");
    repo.commit_file("x.txt", "x3\n", "branch jumps x straight to x3");
    repo.commit_file("y.txt", "y2\n", "branch's real work on y");

    repo.checkout("main");
    repo.commit_file("x.txt", "x2\n", "main steps x to x2");
    repo.commit_file("x.txt", "x3\n", "main steps x to x3");

    repo
}

/// Two branches that both rewrite the same line, so the simulation is
/// guaranteed to actually conflict and resolve rather than no-op.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn conflicting_repo() -> TestRepo {
    const CONTESTED_LINE: usize = 15;

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_file("shared.txt", &base, "base");

    repo.branch("left");
    repo.commit_file(
        "shared.txt",
        &replace_line(&base, CONTESTED_LINE, "left-edit"),
        "left work",
    );

    repo.checkout("main");
    repo.branch("right");
    repo.commit_file(
        "shared.txt",
        &replace_line(&base, CONTESTED_LINE, "right-edit"),
        "right work",
    );

    repo.checkout("main");
    repo
}
