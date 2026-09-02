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
//! spawn here goes through [`NoInheritedRepository`] as well. See
//! [`REPOSITORY_LOCATION_VARS`](crate::git::REPOSITORY_LOCATION_VARS).

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use crate::git::NoInheritedRepository;
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
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .without_inherited_repository()
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
/// The probe is scrubbed like every other spawn here, and for a sharper reason
/// than most: an inherited `GIT_DIR` makes `rev-parse` succeed from *anywhere*,
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
        .without_inherited_repository()
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
