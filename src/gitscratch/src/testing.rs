//! Throwaway git repositories with known conflict shapes.
//!
//! Every tool built on `gitscratch` needs the same handful of repositories to
//! test against — a contested region, a stacked branch, a pair of branches that
//! tie on hunks but not on stops — so they live here once instead of being
//! rebuilt per crate and drifting apart.
//!
//! Every repo lives in its own `TempDir`, so concurrent `cargo test` runs (the
//! pre-commit hook's and yours) never share a path. A private path is only half
//! of it, though: a `cargo test` run *from* the pre-commit hook inherits the
//! hook's git environment, which names the developer's real repository, so every
//! spawn here goes through [`NoInheritedGitEnvironment`] as well.
//!
//! The same hook exports who is committing, and an identity variable outranks
//! the `user.name` [`TestRepo::init`] configures — so a fixture built under a
//! hand-typed `git commit` would otherwise stamp the developer's own name, and
//! one timestamp, on every commit it makes. The one sweep takes that second set
//! off too, because the rule it applies is the `GIT_` prefix rather than a list
//! of names.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

use crate::git::NoInheritedGitEnvironment;
use crate::repo::Repo;
use crate::scratch::Scratch;

/// The name every fixture commit is authored and committed under.
const FIXTURE_USER_NAME: &str = "gitscratch test";

/// The email every fixture commit is authored and committed under.
const FIXTURE_USER_EMAIL: &str = "gitscratch@example.com";

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
        repo.git(&["config", "user.email", FIXTURE_USER_EMAIL]);
        repo.git(&["config", "user.name", FIXTURE_USER_NAME]);
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
        let output = command
            .args(args)
            .current_dir(cwd)
            // The same immunity the runner has, from the same rule, because a
            // fixture is not exempt from an inherited environment just because
            // it is only building something to test with. A test suite run from
            // inside a git hook inherits a *relative* `GIT_INDEX_FILE`, and a
            // linked worktree's `.git` is a file, so a fixture that keeps it
            // cannot add one at all - it fails before the code under test is
            // ever reached.
            .without_inherited_git_environment()
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

    /// Run a git command in the repo and hand its outcome back whatever that
    /// outcome is, with `env` applied on top.
    ///
    /// The spawn [`TestRepo::git`] cannot be. That one raises a non-zero exit
    /// as a panic, which is right while a fixture is being built and wrong for
    /// a *control* — a command run to demonstrate that some hazard really is
    /// armed, whose failure is the demonstration and therefore has to come back
    /// to be read rather than be raised.
    ///
    /// It exists so that permission need not be bought by reaching around the
    /// fixture for a raw [`Command`], which is where the scrub gets lost:
    /// `current_dir` does not settle which repository git uses, because
    /// `GIT_DIR` outranks the working directory, so an unscrubbed control run
    /// from a `pre-push` gate, `git bisect run`, `rebase --exec` or a git hook
    /// merges, or commits, in the developer's own repository instead of in the
    /// fixture. This spawn sheds the inherited git environment exactly as
    /// [`TestRepo::git`] does; only the assertion is gone.
    ///
    /// `env` is applied **after** that sweep, and the order is load-bearing: the
    /// rule the sweep applies is the `GIT_` prefix, so a `GIT_TERMINAL_PROMPT`
    /// set beforehand would be taken straight back off by the very call meant to
    /// leave it standing. Anything a control's own assertions depend on goes
    /// here — `GIT_TERMINAL_PROMPT=0` so a command expected to fail fails
    /// instead of stopping on a prompt, `LC_ALL`/`LANG` pinned for a control
    /// that matches git's own words rather than their translation.
    ///
    /// # Panics
    ///
    /// Panics if `git` cannot be spawned at all — most likely because it is not
    /// installed. A non-zero exit is not a panic; it is the answer.
    pub fn try_git(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(self.dir.path())
            .without_inherited_git_environment();

        // After the sweep, never before it - see the note above on why the
        // order decides whether a caller's `GIT_`-prefixed variable survives.
        for (name, value) in env {
            command.env(name, value);
        }

        command
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn git {args:?}: {e}"))
    }

    /// Write `contents` to `name` and leave it there uncommitted.
    ///
    /// Dirties the working tree the way a developer mid-edit does - a tracked
    /// file modified, or a new file never added - which is the state a replay
    /// cannot see, because it simulates from HEAD.
    ///
    /// # Panics
    ///
    /// Panics if the fixture file cannot be written.
    pub fn write_file(&self, name: &str, contents: &str) {
        std::fs::write(self.dir.path().join(name), contents).expect("write fixture file");
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
            self.write_file(name, contents);
            self.git(&["add", name]);
        }
        self.git(&["commit", "-q", "-m", message]);
    }

    /// Commit `contents` under a file name given as raw bytes, and return the
    /// id of the commit that holds it.
    ///
    /// `name` never reaches the filesystem: the blob, the tree and the commit
    /// are written straight into the object database, by `hash-object`,
    /// `mktree` and `commit-tree`. That is not a shortcut, it is the only route
    /// there. A name that is not valid UTF-8 cannot be created on disk on macOS
    /// at all — APFS rejects it with `EILSEQ`, so `std::fs::write` fails before
    /// git is asked anything — and `#[cfg(unix)]` does not rescue an on-disk
    /// fixture either, because macOS *is* unix. The object store has no such
    /// opinion on any platform, which is what makes this portable.
    ///
    /// It is also the honest fixture rather than a contrivance. Git records a
    /// path as bytes, so a repository cloned from a filesystem that does permit
    /// such a name — a latin-1 name on Linux, say — holds exactly what this
    /// builds, and the developer running the replay is on the machine that
    /// cannot spell it.
    ///
    /// The tree record goes in on stdin, which is what lets a byte no `&str`
    /// argument could carry become a path. The commit is parentless and nothing
    /// references it, so it is reachable only by the id returned here.
    ///
    /// # Panics
    ///
    /// Panics if `git` cannot be spawned or any of the three steps fails.
    pub fn commit_file_named_by_bytes(&self, name: &[u8], contents: &str, message: &str) -> String {
        let blob = self.git_with_stdin(&["hash-object", "-w", "--stdin"], contents.as_bytes());

        // `mktree`'s build-tree-entry format: mode, type and object id
        // space-separated, then a tab, then the name. `-z` terminates the
        // record with a NUL instead of a newline, and turns off the quoting git
        // would otherwise apply to the name on the way *in* as well as out.
        let mut record = format!("100644 blob {blob}\t").into_bytes();
        record.extend_from_slice(name);
        record.push(0);
        let tree = self.git_with_stdin(&["mktree", "-z"], &record);

        self.git(&["commit-tree", &tree, "-m", message])
    }

    /// Run a git command in the repo with `stdin` piped to it, panicking on
    /// failure.
    ///
    /// Separate from [`TestRepo::git`] because that one's arguments are `&str`,
    /// and the one thing a fixture cannot say in UTF-8 is a path git records as
    /// bytes. Stdin is the way in that has no such constraint.
    fn git_with_stdin(&self, args: &[&str], stdin: &[u8]) -> String {
        use std::io::Write as _;

        let mut command = Command::new("git");
        // The same immunity `git_in` takes, for the same reason: a fixture that
        // inherits a redirected `GIT_DIR` or `GIT_INDEX_FILE` writes its
        // objects into the developer's real repository instead of this one.
        crate::git::shed_inherited_git_environment(&mut command);

        let mut child = command
            .args(args)
            .current_dir(self.dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn git {args:?}: {e}"));

        // Taken and dropped, so git sees end-of-input rather than waiting on a
        // pipe this process still holds open.
        child
            .stdin
            .take()
            .expect("git's stdin was piped")
            .write_all(stdin)
            .unwrap_or_else(|e| panic!("failed to write stdin to git {args:?}: {e}"));

        let output = child
            .wait_with_output()
            .unwrap_or_else(|e| panic!("failed to wait for git {args:?}: {e}"));

        assert!(
            output.status.success(),
            "git {args:?} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        String::from_utf8_lossy(&output.stdout).trim().to_string()
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

    /// A bare clone of this fixture, standing on `head`.
    ///
    /// The one repository shape where the cheap questions and the expensive ones
    /// part company. `git worktree add --detach HEAD` succeeds against a bare
    /// repository, so a replay can be run in it and measured exactly as usual,
    /// while `git status` cannot run at all — there is no working tree to take a
    /// status of. A pre-flight that treats an informational query as fatal
    /// therefore refuses a question it could have answered, which is a stricter
    /// failure than the replay it was meant to guard against.
    ///
    /// A clone rather than `git init --bare`, so the branches and the conflict
    /// shape are the fixture's own and the answer can be compared against the
    /// one the same fixture gives through its working tree. It lives in a
    /// temporary directory of its own rather than under the source's worktree,
    /// so it neither dirties the source nor moves the numbers a caller is about
    /// to assert on.
    ///
    /// # Panics
    ///
    /// Panics if the temporary directory cannot be created, if either path is
    /// not valid UTF-8, or if git fails — most likely because `head` does not
    /// name a branch of this fixture.
    pub fn bare_clone(&self, head: &str) -> BareRepo {
        let dir = TempDir::new().expect("create temp dir");
        let bare = BareRepo { dir };

        self.git(&[
            "clone",
            "-q",
            "--bare",
            self.dir.path().to_str().expect("utf-8 fixture path"),
            bare.path().to_str().expect("utf-8 bare clone path"),
        ]);
        // A clone copies the source's HEAD, and what makes this shape worth
        // building is standing somewhere specific.
        self.git_in(
            bare.path(),
            &["symbolic-ref", "HEAD", &format!("refs/heads/{head}")],
        );
        // The fixture proves its own premise: a bare HEAD that resolves is the
        // whole point, so a `head` that names nothing fails here rather than
        // surfacing later as a replay that could not start.
        self.git_in(bare.path(), &["rev-parse", "HEAD^{commit}"]);

        bare
    }

    /// A scratch worktree of this fixture, checked out at `at`.
    ///
    /// Deliberately routed through [`Repo::open`] rather than straight at
    /// `Scratch`'s own crate-private constructor, even though this module lives
    /// inside the crate and could reach either. Two reasons, and the second is
    /// the important one.
    /// It spares every consumer the two-step incantation; and it means the
    /// suites exercise the entrance a real consumer is now obliged to use,
    /// rather than a shortcut only in-crate code can spell. A door nothing
    /// knocks on is a door that stops working quietly.
    ///
    /// # Panics
    ///
    /// Panics if the fixture is somehow not a repository, or if git refuses to
    /// add the worktree — most likely because `at` does not name a commit.
    pub fn scratch(&self, at: &str) -> Scratch {
        Repo::open(self.dir.path())
            .expect("a fixture is a git repository")
            .scratch(at)
            .expect("create the scratch worktree")
    }
}

/// A bare repository — refs and objects, and no working tree at all.
///
/// Built by [`TestRepo::bare_clone`], and mirroring [`NotARepo`]'s shape — a
/// `TempDir` behind a `path()` — so a consumer never has to name `tempfile`'s
/// types or remember to keep the guard alive for the right reason.
pub struct BareRepo {
    dir: TempDir,
}

impl BareRepo {
    /// The repository directory, which for a bare repository is the git
    /// directory itself.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// A directory that is not inside any git repository, so a tool can be run
/// somewhere it has no question to answer.
///
/// Mirrors [`TestRepo`]'s shape - a `TempDir` behind a `path()` - so a consumer
/// never has to name `tempfile`'s types or remember to keep the guard alive for
/// the right reason.
pub struct NotARepo {
    dir: TempDir,
}

impl NotARepo {
    /// The directory, guaranteed to sit outside every repository.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// A throwaway directory that is emphatically not a repository.
///
/// The premise is checked rather than assumed: a developer whose `TMPDIR` sits
/// inside a git repository would otherwise get a test that fails somewhere far
/// away from the reason, so the fixture proves its own claim up front and
/// panics with the offending path if it cannot.
///
/// The probe takes the repository scrub like every other spawn here, and for a
/// sharper reason than most: an inherited `GIT_DIR` makes `rev-parse` succeed
/// from *anywhere*,
/// so the check would fail on a perfectly good directory and blame it for the
/// hook's environment.
///
/// # Panics
///
/// Panics if the temporary directory cannot be created, if `git` is not
/// installed, or if the temporary directory turns out to be inside a
/// repository after all.
pub fn not_a_repository() -> NotARepo {
    let dir = TempDir::new().expect("create temp dir");

    let probe = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(dir.path())
        .without_inherited_git_environment()
        .output()
        .expect("failed to spawn git rev-parse --git-dir");

    assert!(
        !probe.status.success(),
        "{} is inside a git repository, so it cannot stand in for somewhere that is not: {}",
        dir.path().display(),
        String::from_utf8_lossy(&probe.stdout).trim(),
    );

    NotARepo { dir }
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

/// A conflict in a file named `日本語.txt` beside one named `readme.md`, on
/// branches named `left-左` and `right-右`.
///
/// Three separate things go wrong with a non-ASCII name, and this one shape is
/// built to expose all three at once rather than needing a fixture apiece.
///
/// **The name has to survive git.** Under git's default `core.quotePath`, a
/// path outside ASCII comes back from `git diff --name-only` C-quoted and
/// octal-escaped - `"\346\227\245\346\234\254\350\252\236.txt"` rather than
/// `日本語.txt` - so a replay that takes git at its word goes looking for a file
/// that does not exist and reports a name nobody typed.
///
/// **The hunk count has to survive it too**, which is why `日本語.txt` is
/// contested in *two* regions while `readme.md` is contested in one. A
/// conflicted file that cannot be read is floored at a single hunk, so a fixture
/// whose real answer were also one would report the right number by accident and
/// let an escaped name hide behind a correct total.
///
/// **The column has to line up.** `readme.md` is 9 bytes, 9 characters and 9
/// terminal columns; `日本語.txt` is 13 bytes, 7 characters and 10 columns. The
/// two names disagree about which is wider depending on which of the three
/// measures you ask for, so a breakdown padded by anything except display width
/// comes out visibly ragged - and, being a pair, they say so on one screen.
///
/// The branch names carry multi-byte characters for the same reason: a branch
/// name is echoed back in the verdict, so it travels the same path a file name
/// does, and mixing scripts within one name catches a truncation that a purely
/// non-ASCII name would not.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn multi_byte_names_repo() -> TestRepo {
    /// The one region `readme.md` is contested in.
    const ASCII_LINE: usize = 15;
    /// The two regions `日本語.txt` is contested in, far enough apart that
    /// git's 3-line diff context cannot merge them into one conflict.
    const WIDE_LINES: [usize; 2] = [5, 25];

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_files(&[("readme.md", &base), ("日本語.txt", &base)], "base");

    // Both branches make the same three edits with different content, so every
    // one of them collides and the two branches are otherwise symmetric.
    for (branch, edit) in [("left-左", "左-edit"), ("right-右", "右-edit")] {
        repo.checkout("main");
        repo.branch(branch);

        let mut wide = base.clone();
        for line in WIDE_LINES {
            wide = replace_line(&wide, line, edit);
        }

        repo.commit_files(
            &[
                ("readme.md", &replace_line(&base, ASCII_LINE, edit)),
                ("日本語.txt", &wide),
            ],
            &format!("{branch} rewrites both files"),
        );
    }

    repo.checkout("main");
    repo
}

/// A conflict in five files whose names a line of git output cannot carry
/// intact, beside a `plain.txt` that it can.
///
/// [`multi_byte_names_repo`] covers the one class `core.quotePath=false` fixes.
/// These are the classes it does not, and they split into two mechanisms that a
/// single fixture is cheaper to hold than two:
///
/// **Git quotes these whatever `core.quotePath` says.** `quote_c_style` escapes
/// a double quote and a backslash independently of that setting - `quotePath`
/// only governs bytes at or above `0x80` - so `back\slash.txt` comes back as
/// `"back\\slash.txt"` and `quo"te.txt` as `"quo\"te.txt"`, wrapped in the
/// quotes git added. Those names open no file on disk.
///
/// **A reader that trims kills the rest.** Nothing quotes a path that merely
/// begins or ends with whitespace, so git hands the real name over and a
/// whitespace-trimming reader is what destroys it. `\u{3000}wide.txt ` opens
/// with an IDEOGRAPHIC SPACE, which Rust's Unicode-aware `str::trim` strips as
/// readily as the ASCII spaces on ` lead.txt` and `trail.txt `.
///
/// Where each name lands in git's byte-sorted output is load-bearing, because a
/// reader can trim per line, or trim the whole of stdout once, or both, and only
/// the middle of the list survives the second. ` lead.txt` sorts first, so its
/// leading space is the first byte of stdout; `\u{3000}wide.txt ` sorts last, so
/// its trailing space is the last. `trail.txt ` sits between them, reachable
/// only by a per-line trim. All three have to come back for the reader to be
/// doing no trimming at all, which is the only thing that is correct.
///
/// Every file is contested in the *same two regions*, for the reason
/// [`multi_byte_names_repo`] contests its wide name twice: a conflicted file
/// that cannot be opened is floored at one hunk, so a one-region fixture would
/// report the right number by accident and let a mangled name pass. Uniformity
/// is the rest of it - the expected answer is "two hunks each", so the only
/// thing a failure can be about is the name.
///
/// Unix-only. A name containing `"` or `\` is illegal on Windows, so the fixture
/// could not be built there to be tested at all.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
#[cfg(unix)]
pub fn awkward_names_repo() -> TestRepo {
    /// The two regions every file is contested in, far enough apart that git's
    /// 3-line diff context cannot merge them into one conflict.
    const CONTESTED_LINES: [usize; 2] = [5, 25];
    /// In git's byte-sorted order, which is what puts a leading space at the
    /// very front of stdout and a trailing one at the very back.
    const AWKWARD_NAMES: [&str; 6] = [
        " lead.txt",
        "back\\slash.txt",
        "plain.txt",
        "quo\"te.txt",
        "trail.txt ",
        "\u{3000}wide.txt ",
    ];

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    let files: Vec<(&str, &str)> = AWKWARD_NAMES
        .iter()
        .map(|name| (*name, base.as_str()))
        .collect();
    repo.commit_files(&files, "base");

    // Both branches rewrite both regions of every file with different content,
    // so all six collide and the two branches are otherwise symmetric.
    for (branch, edit) in [("left", "left-edit"), ("right", "right-edit")] {
        repo.checkout("main");
        repo.branch(branch);

        let mut contested = base.clone();
        for line in CONTESTED_LINES {
            contested = replace_line(&contested, line, edit);
        }

        let files: Vec<(&str, &str)> = AWKWARD_NAMES
            .iter()
            .map(|name| (*name, contested.as_str()))
            .collect();
        repo.commit_files(&files, &format!("{branch} rewrites every file"));
    }

    repo.checkout("main");
    repo
}

/// A conflict in `sub/nested/shared.txt` beside one in `shared.txt`, so a tool
/// can be run from `sub/nested` — a committed subdirectory, two levels down.
///
/// [`Repo::open`] takes whichever directory a tool was run in, which for a
/// developer is hardly ever the repository root, and every other fixture here is
/// only ever opened at its own root. The two ways a subdirectory run goes wrong
/// are both silent, and this shape is built so that each one shows up as a
/// different wrong answer:
///
/// **A name can lose its prefix.** `sub/nested/shared.txt` is how git names the
/// conflicted file, relative to the repository root, and that has to be what a
/// breakdown prints no matter which directory the run started in. A reader that
/// named paths relative to the cwd instead would print `shared.txt` — a real
/// file, in the wrong place, indistinguishable from the root one at a glance.
///
/// **A file can vanish.** `shared.txt` conflicts *outside* the subdirectory the
/// run started in, so anything that scoped git's answers to the cwd — a
/// `diff --relative`, a pathspec of `.` — would drop it from the count entirely
/// and report less work than there is. Both files conflict in the same single
/// region, so the expected answer is one hunk each and the only thing a failure
/// can be about is which files were seen and what they were called.
///
/// Two levels rather than one because a prefix is a path, not a name: a
/// single-component subdirectory cannot distinguish a reader that keeps the whole
/// prefix from one that keeps only its last component.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, the subdirectory not
/// creatable, or a command failing.
pub fn nested_conflict_repo() -> TestRepo {
    const CONTESTED_LINE: usize = 15;
    /// The conflicted file at the repository root, outside the subdirectory a
    /// run starts in.
    const ROOT_FILE: &str = "shared.txt";
    /// The conflicted file inside it, named with the whole prefix git reports.
    const NESTED_FILE: &str = "sub/nested/shared.txt";

    let repo = TestRepo::init();
    std::fs::create_dir_all(repo.path().join("sub").join("nested"))
        .expect("create the fixture's nested directory");

    let base = numbered_lines(30);
    repo.commit_files(&[(ROOT_FILE, &base), (NESTED_FILE, &base)], "base");

    // Both branches rewrite the same region of both files with different
    // content, so each of them collides and the two branches are otherwise
    // symmetric.
    for (branch, edit) in [("left", "left-edit"), ("right", "right-edit")] {
        repo.checkout("main");
        repo.branch(branch);

        let contested = replace_line(&base, CONTESTED_LINE, edit);
        repo.commit_files(
            &[(ROOT_FILE, &contested), (NESTED_FILE, &contested)],
            &format!("{branch} rewrites both files"),
        );
    }

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

#[cfg(test)]
mod tests {
    use super::{TestRepo, FIXTURE_USER_EMAIL, FIXTURE_USER_NAME};

    /// Marks the re-executed child half of
    /// [`a_fixture_commits_under_its_own_identity_in_a_hook_environment`].
    const CHILD_MARKER: &str = "GITSCRATCH_FIXTURE_IDENTITY_CHILD";

    /// libtest's exact filter for the one test the child half runs.
    const IDENTITY_TEST_PATH: &str =
        "testing::tests::a_fixture_commits_under_its_own_identity_in_a_hook_environment";

    /// What libtest prints when exactly one test ran and passed.
    ///
    /// A filter matching nothing exits zero, so a rename that missed
    /// [`IDENTITY_TEST_PATH`] would otherwise leave the parent green over a
    /// child that ran nothing at all.
    const ONE_TEST_PASSED: &str = "1 passed";

    /// A timestamp no run of this suite can produce on its own, so a commit
    /// carrying it can only have taken it from the environment.
    ///
    /// Written in git's raw `<epoch> <timezone>` form, which `--date=raw`
    /// prints back byte for byte. Every other spelling git accepts here it
    /// also reformats on the way out — an ISO date set as `+00:00` comes back
    /// as `Z` — so an assertion against one of those could only ever fail to
    /// match, which is a test that passes for the wrong reason.
    const LEAKED_DATE: &str = "1000000000 +0000";

    /// Everything git tells a hook about who is committing, carrying values
    /// that stand in for a consuming tool's own identity.
    ///
    /// All six, not just the four that name a person. A fixture builder that
    /// scrubbed only the names would still let every commit it makes share one
    /// timestamp, which is how a fixture that depends on commit order stops
    /// meaning what it says.
    const HOOK_ENVIRONMENT: [(&str, &str); 6] = [
        ("GIT_AUTHOR_NAME", "Consuming Tool"),
        ("GIT_AUTHOR_EMAIL", "consumer@example.invalid"),
        ("GIT_AUTHOR_DATE", LEAKED_DATE),
        ("GIT_COMMITTER_NAME", "Consuming Tool"),
        ("GIT_COMMITTER_EMAIL", "consumer@example.invalid"),
        ("GIT_COMMITTER_DATE", LEAKED_DATE),
    ];

    /// Build a fixture, commit into it, and assert the commit carries the
    /// fixture's own identity rather than whatever the environment holds.
    ///
    /// Read back through `git log` rather than `git var`, because the question
    /// here is what a fixture commit actually ends up stamped with — the
    /// identity has to survive [`TestRepo::init`]'s `git config` *and* the
    /// commit that follows it.
    fn assert_fixture_identity() {
        let repo = TestRepo::init();
        repo.commit_file("seed.txt", "seed\n", "seed");

        let stamped = repo.git(&[
            "log",
            "-1",
            "--date=raw",
            "--format=%an|%ae|%cn|%ce|%ad|%cd",
        ]);

        let expected = format!(
            "{FIXTURE_USER_NAME}|{FIXTURE_USER_EMAIL}|{FIXTURE_USER_NAME}|{FIXTURE_USER_EMAIL}|"
        );
        assert!(
            stamped.starts_with(&expected),
            "a fixture commit must be authored and committed by the fixture, not \
             by whichever tool is driving the suite.\n  \
             expected: {expected}...\n  \
             got:      {stamped}\n\
             An identity variable outranks every config source, so the `git config \
             user.name` TestRepo::init sets loses to a GIT_AUTHOR_NAME the process \
             inherited — and git exports all six of them into every hook it runs."
        );
        assert!(
            !stamped.contains(LEAKED_DATE),
            "a fixture commit must be timestamped when it was made, not when an \
             inherited GIT_AUTHOR_DATE and GIT_COMMITTER_DATE say: {stamped}"
        );
    }

    /// [`TestRepo::git`] raises a failed git command as a panic, which is the
    /// right answer while a fixture is being built and the wrong one for a
    /// control — a command run to demonstrate that some hazard is armed, and
    /// which therefore has to be *allowed* to fail so that its failure can be
    /// read. [`TestRepo::try_git`] is that spawn, so the permission a control
    /// needs is available without reaching around the fixture for a raw
    /// `Command` and losing the environment scrub with it.
    ///
    /// A repository with no commits, because an unborn `HEAD` is a failure git
    /// produces identically everywhere and with nothing else set up.
    #[test]
    fn try_git_hands_back_a_failure_instead_of_raising_it() {
        let repo = TestRepo::init();

        let refused = repo.try_git(&["rev-parse", "--verify", "HEAD"], &[]);

        assert!(
            !refused.status.success(),
            "an unborn HEAD does not resolve, so this command had to fail:\n{}",
            String::from_utf8_lossy(&refused.stdout)
        );
        assert!(
            !refused.stderr.is_empty(),
            "the caller must get git's own account of the failure back"
        );
    }

    /// A consuming tool invoked from a git hook inherits `GIT_AUTHOR_NAME` and
    /// its five siblings, and those variables outrank the `user.name` this
    /// module's fixture builder configures. `.husky/pre-commit` in this
    /// repository is such a hook, and it runs `cargo test --workspace`, so
    /// this is the environment a hand-typed commit hands the whole suite.
    ///
    /// The environment belongs to a re-executed child of this test binary
    /// rather than to this process. `std::env::set_var` would leak into every
    /// other test in the binary, and a child process keeps concurrent runs of
    /// this suite isolated from each other.
    #[test]
    fn a_fixture_commits_under_its_own_identity_in_a_hook_environment() {
        if std::env::var_os(CHILD_MARKER).is_some() {
            assert_fixture_identity();
            return;
        }

        let mut child = std::process::Command::new(
            std::env::current_exe().expect("path of the running test binary"),
        );
        child
            .args([IDENTITY_TEST_PATH, "--exact", "--nocapture"])
            .env(CHILD_MARKER, "1");
        for (name, value) in HOOK_ENVIRONMENT {
            child.env(name, value);
        }

        let output = child.output().expect("re-run this test binary");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "a fixture commit did not survive a hook environment:\n{stdout}\n{stderr}"
        );
        assert!(
            stdout.contains(ONE_TEST_PASSED),
            "the child must have run exactly one test, got:\n{stdout}"
        );
    }
}
