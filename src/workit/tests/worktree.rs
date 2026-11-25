//! The worktree filter judges what is below the search path, not the path.
//!
//! `--include-worktrees` answers one question: this tree holds a second
//! checkout of packages the repository already has, so do not list them twice.
//! That is a statement about the directories *below* the search path. The
//! filter used to walk up from each package to the root of the file system
//! instead, so a search path that itself sat inside a git worktree lost every
//! package under it — and said only "No Cargo.toml files found in
//! subdirectories". The tree this repository is developed in is such a tree, so
//! under the workflow it mandates the tool found nothing and never said why.
//!
//! The rules under test: a package is in a worktree only when a `.git` file
//! whose git directory holds a `commondir` file is found between it and the
//! search path; a `.git` directory is a repository rather than a worktree, and
//! a package under one is listed; a submodule, whose `.git` is a file too,
//! goes with the checkout that holds it, so its package is listed under the
//! main checkout and left out under a linked worktree; any other `.git` file,
//! such as the one `git init --separate-git-dir` leaves or one that names no
//! git directory, stops the walk as a `.git` directory does; and a run that
//! found packages and then filtered every one of them out says so, and names
//! the flag that would have kept them.
//!
//! # Hermetic git
//!
//! Every fixture here builds real repositories and real worktrees, and this
//! repository's pre-commit hook runs `cargo test`. Git exports `GIT_DIR`,
//! `GIT_INDEX_FILE` and their kin into every hook it runs, and `GIT_DIR`
//! outranks both the working directory and `git -C`, so a `git init` spawned
//! from a hook without a scrub re-initialises the developer's own repository.
//! That has happened to this repository before.
//!
//! So every child spawned here starts through [`childenv::fixture_command`],
//! whose rule is the `GIT_` prefix read off the live environment rather than a
//! list of names that goes stale, and every one of them is pointed at a `HOME`
//! and an `XDG_CONFIG_HOME` of its own inside a temporary directory, so no
//! global config is read and none is written. No test runs `git config`:
//! identity and signing are stated per invocation with `-c`. After each
//! repository and each worktree is built, [`Fixture::assert_toplevel`] asks git
//! itself which working tree it just acted on and refuses to continue unless
//! the answer is the fixture.
//!
//! Every fixture lives in its own [`TempDir`] and every branch it creates
//! carries this process's id and a nanosecond stamp, so two copies of this file
//! can run at the same time. `--output` always names a file inside the fixture
//! and every run passes `--dry-run`, so no run can write a manifest anywhere.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::TempDir;

/// The name every fixture commit is authored and committed under. Stated per
/// invocation with `-c`, never written into a config file.
const FIXTURE_USER_NAME: &str = "workit test";

/// The email every fixture commit is authored and committed under.
const FIXTURE_USER_EMAIL: &str = "workit-test@example.invalid";

/// A temporary tree, a temporary home, and the git spawns that stay inside
/// both.
struct Fixture {
    /// Deletes the tree when it drops.
    _tree: TempDir,
    /// Deletes the home when it drops.
    _home: TempDir,
    /// The canonical path of the tree every fixture path is built under.
    ///
    /// Canonical because `workit` subtracts canonical paths to name a member,
    /// and a temporary directory on macOS is reached through a symbolic link.
    root: PathBuf,
    /// The canonical path handed to every git child as `HOME`.
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tree = TempDir::new().expect("a temporary tree is created");
        let home = TempDir::new().expect("a temporary home is created");
        let root = std::fs::canonicalize(tree.path()).expect("the tree has a canonical path");
        let home_path = std::fs::canonicalize(home.path()).expect("the home has a canonical path");

        Self {
            _tree: tree,
            _home: home,
            root,
            home: home_path,
        }
    }

    /// A path inside the fixture tree.
    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Runs git in `cwd` and hands back its standard output, trimmed.
    ///
    /// The whole inherited `GIT_` family is shed first, so `cwd` is the only
    /// repository this command can reach — `current_dir` alone loses to an
    /// inherited `GIT_DIR`. `HOME` and `XDG_CONFIG_HOME` are then pointed at
    /// the fixture's own empty home, so nothing is read from, or written to,
    /// the developer's global config.
    ///
    /// # Panics
    ///
    /// Panics if git cannot be spawned or exits non-zero, carrying the command
    /// and both of its streams.
    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let output = childenv::fixture_command("git")
            .args(args)
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            // Set after the scrub, which strips by the `GIT_` prefix and would
            // otherwise take this straight back off: a fixture command that
            // stopped on a credential prompt would hang the suite.
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn git {args:?}: {e}"));

        assert!(
            output.status.success(),
            "git {args:?} failed in {}:\n{}\n{}",
            cwd.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Asks git which working tree it acts on in `expected`, and refuses to
    /// continue unless that is `expected` itself.
    ///
    /// The one assertion that proves the scrub held. Every other check in this
    /// file would pass just as happily against a fixture whose git commands had
    /// been redirected into the real repository.
    fn assert_toplevel(&self, expected: &Path) {
        let toplevel = self.git(expected, &["rev-parse", "--show-toplevel"]);
        let toplevel = std::fs::canonicalize(&toplevel)
            .unwrap_or_else(|e| panic!("git named {toplevel}, which cannot be resolved: {e}"));

        assert_eq!(
            toplevel,
            expected,
            "git acted on {}, not on the fixture at {}. The inherited git environment was not \
             shed, so this run is touching a repository it did not create.",
            toplevel.display(),
            expected.display(),
        );
    }

    /// Creates a repository at `relative` with one commit in it, and proves git
    /// built it where it was told to.
    fn repository_at(&self, relative: &str) -> PathBuf {
        let repo = self.path(relative);
        std::fs::create_dir_all(&repo).expect("the fixture repository directory is created");

        self.git(&repo, &["init", "-q", "-b", &unique("main")]);
        std::fs::write(repo.join("README"), "seed\n").expect("the seed file is written");
        self.git(&repo, &["add", "README"]);
        self.commit_all(&repo, "seed");

        self.assert_toplevel(&repo);
        assert!(
            repo.join(".git").is_dir(),
            "a repository keeps its git directory as a directory: {}",
            repo.display()
        );

        repo
    }

    /// Adds a worktree of `repository` at `relative`, and proves it is one.
    ///
    /// A linked worktree is exactly the shape the filter is about: its `.git`
    /// is a *file* naming the directory it borrows, where a repository's is a
    /// directory. The assertion is what keeps a test that meant to build a
    /// worktree from quietly measuring a second repository.
    fn worktree_at(&self, repository: &Path, relative: &str) -> PathBuf {
        let worktree = self.path(relative);

        self.git(
            repository,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &unique("worktree"),
                &worktree.to_string_lossy(),
            ],
        );

        self.assert_toplevel(&worktree);
        assert!(
            worktree.join(".git").is_file(),
            "a linked worktree keeps its git link as a file: {}",
            worktree.join(".git").display()
        );

        worktree
    }

    /// Stages every change in `repository` and commits it as `message`.
    ///
    /// Identity and signing are stated per invocation with `-c`, never written
    /// into a config file.
    fn commit_all(&self, repository: &Path, message: &str) {
        self.git(repository, &["add", "-A"]);
        self.git(
            repository,
            &[
                "-c",
                &format!("user.name={FIXTURE_USER_NAME}"),
                "-c",
                &format!("user.email={FIXTURE_USER_EMAIL}"),
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
    }

    /// Adds the repository at `source` to `superproject` as a submodule at
    /// `relative`, and proves it is one.
    ///
    /// A submodule is the look-alike of a linked worktree: its `.git` is a
    /// *file* naming a git directory, as a worktree's is. But the directory it
    /// names is a repository of its own, kept under `.git/modules`, not a
    /// borrowed one. The assertion keeps a test that meant to build a submodule
    /// from quietly measuring a plain repository.
    fn submodule_at(&self, superproject: &Path, source: &Path, relative: &str) -> PathBuf {
        let submodule = superproject.join(relative);

        self.git(
            superproject,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "--quiet",
                "add",
                &source.to_string_lossy(),
                relative,
            ],
        );

        self.assert_toplevel(&submodule);
        assert!(
            submodule.join(".git").is_file(),
            "a submodule keeps its git link as a file, as a linked worktree does: {}",
            submodule.join(".git").display()
        );

        submodule
    }

    /// Writes a package manifest at `relative`, so the walk has one to find.
    fn package_at(&self, relative: &str, name: &str) {
        let dir = self.path(relative);
        std::fs::create_dir_all(&dir).expect("the fixture package directory is created");
        std::fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("the fixture manifest is written");
    }

    /// Runs the binary over `search_root` with `extra` arguments appended.
    ///
    /// `--dry-run` keeps it from writing a manifest, and `--output` names a
    /// file inside the fixture, so no run here can reach a real manifest. It
    /// names one inside `search_root` in particular, because the directory
    /// that would hold the manifest is the directory every member is measured
    /// from — an output beside the fixture root would name a package under a
    /// worktree search path `second/alpha` rather than `alpha`, and the test
    /// would then be reading the output resolution rather than the filter.
    ///
    /// The inherited git environment is shed from this child too: it spawns no
    /// git today, and a test suite that only stays hermetic while the code
    /// under test happens not to call git is hermetic by luck.
    fn workit(&self, search_root: &Path, extra: &[&str]) -> Output {
        childenv::fixture_command(env!("CARGO_BIN_EXE_workit"))
            .current_dir(search_root)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .arg("--path")
            .arg(search_root)
            .arg("--output")
            .arg(search_root.join("workspace-manifest.toml"))
            .arg("--dry-run")
            .args(extra)
            .output()
            .expect("the workit binary runs")
    }
}

/// A name no concurrent copy of this file can also choose.
///
/// Two `cargo test` runs share `./target` and can execute this very test at the
/// same time. Each fixture already has a temporary directory of its own, so a
/// branch name could only collide inside one repository — but a name keyed on
/// the process and the clock costs nothing and removes the question.
fn unique(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

/// The members the run printed, in the order it printed them.
fn members_of(output: &Output) -> Vec<String> {
    let printed = std::str::from_utf8(&output.stdout).expect("workit writes UTF-8");
    printed
        .lines()
        .filter_map(|line| line.strip_prefix("  - "))
        .map(str::to_string)
        .collect()
}

/// Everything the run wrote, both streams, for an assertion about what it said.
fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Asserts the run succeeded, and says what it printed when it did not.
fn assert_succeeded(output: &Output) {
    assert!(
        output.status.success(),
        "workit exited with {}:\n{}",
        output.status,
        said(output)
    );
}

#[test]
fn a_search_path_inside_a_worktree_still_finds_the_packages_under_it() {
    let fixture = Fixture::new();
    let repository = fixture.repository_at("checkout");
    let worktree = fixture.worktree_at(&repository, "second");
    fixture.package_at("second/alpha", "workit-fixture-worktree-root-alpha");
    fixture.package_at("second/beta", "workit-fixture-worktree-root-beta");

    let output = fixture.workit(&worktree, &[]);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec!["alpha".to_string(), "beta".to_string()],
        "the flag asks about worktrees below the search path. Where the search path itself sits \
         is the user's own choice of where to work, and they did not ask to be filtered on it:\n{}",
        said(&output)
    );
}

#[test]
fn a_search_path_that_is_a_repository_still_finds_the_packages_under_it() {
    let fixture = Fixture::new();
    let repository = fixture.repository_at("checkout");
    fixture.package_at("checkout/alpha", "workit-fixture-repo-root-alpha");

    let output = fixture.workit(&repository, &[]);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec!["alpha".to_string()],
        "a plain repository is not a worktree, and never was:\n{}",
        said(&output)
    );
}

#[test]
fn a_worktree_below_the_search_path_is_left_out() {
    let fixture = Fixture::new();
    let repository = fixture.repository_at("checkout");
    fixture.worktree_at(&repository, "second");
    fixture.package_at("plain/alpha", "workit-fixture-below-alpha");
    fixture.package_at("second/beta", "workit-fixture-below-beta");

    let root = fixture.root.clone();
    let output = fixture.workit(&root, &[]);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec!["plain/alpha".to_string()],
        "a worktree below the search path is a second checkout of packages the repository \
         already holds, so it is left out:\n{}",
        said(&output)
    );
}

#[test]
fn a_worktree_below_the_search_path_is_listed_when_the_flag_asks_for_it() {
    let fixture = Fixture::new();
    let repository = fixture.repository_at("checkout");
    fixture.worktree_at(&repository, "second");
    fixture.package_at("plain/alpha", "workit-fixture-flag-alpha");
    fixture.package_at("second/beta", "workit-fixture-flag-beta");

    let root = fixture.root.clone();
    let output = fixture.workit(&root, &["--include-worktrees"]);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec!["plain/alpha".to_string(), "second/beta".to_string()],
        "--include-worktrees puts the second checkout back:\n{}",
        said(&output)
    );
}

#[test]
fn a_repository_below_the_search_path_is_not_a_worktree() {
    let fixture = Fixture::new();
    fixture.repository_at("clone");
    fixture.package_at("clone/gamma", "workit-fixture-nested-clone-gamma");

    let root = fixture.root.clone();
    let output = fixture.workit(&root, &[]);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec!["clone/gamma".to_string()],
        "a repository of its own below the search path holds packages nothing else holds, so \
         the worktree filter has no opinion about it:\n{}",
        said(&output)
    );
}

#[test]
fn a_package_inside_a_submodule_below_the_search_path_is_listed() {
    let fixture = Fixture::new();

    // The repository the submodule is cloned from lies outside the search
    // path, so the walk meets its package once: inside the submodule.
    let source = fixture.repository_at("source");
    fixture.package_at("source", "workit-fixture-submodule-sublib");
    fixture.commit_all(&source, "add the package");

    let superproject = fixture.repository_at("superproject");
    fixture.package_at("superproject/app", "workit-fixture-submodule-app");
    fixture.submodule_at(&superproject, &source, "vendor/lib");

    let output = fixture.workit(&superproject, &[]);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec!["app".to_string(), "vendor/lib".to_string()],
        "a submodule keeps a `.git` file as a linked worktree does, but it is a repository \
         of its own, not a second checkout of packages the superproject holds:\n{}",
        said(&output)
    );
}

#[test]
fn a_package_inside_a_submodule_of_a_worktree_below_the_search_path_is_left_out() {
    let fixture = Fixture::new();

    let source = fixture.repository_at("source");
    fixture.package_at("source", "workit-fixture-worktree-submodule-sublib");
    fixture.commit_all(&source, "add the package");

    let superproject = fixture.repository_at("superproject");
    fixture.package_at("superproject/app", "workit-fixture-worktree-submodule-app");
    fixture.submodule_at(&superproject, &source, "vendor/lib");
    fixture.commit_all(&superproject, "add the app and the submodule");

    // A submodule checked out in a linked worktree keeps its git directory
    // under `.git/worktrees/<name>/modules`, which holds no `commondir`. Its
    // package is still a second checkout of the one the superproject holds.
    let worktree = fixture.worktree_at(&superproject, "superproject/wt/feat");
    fixture.git(
        &worktree,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "--quiet",
            "update",
            "--init",
        ],
    );
    let checked_out = worktree.join("vendor/lib");
    fixture.assert_toplevel(&checked_out);
    assert!(
        checked_out.join(".git").is_file(),
        "a submodule in a linked worktree keeps its git link as a file: {}",
        checked_out.join(".git").display()
    );

    let output = fixture.workit(&superproject, &[]);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec!["app".to_string(), "vendor/lib".to_string()],
        "a submodule belongs to the checkout of its superproject, so a submodule inside a \
         linked worktree is part of that second checkout and is left out with it:\n{}",
        said(&output)
    );
}

#[test]
fn a_linked_worktree_of_a_submodule_below_the_search_path_is_left_out() {
    let fixture = Fixture::new();

    let source = fixture.repository_at("source");
    fixture.package_at("source", "workit-fixture-submodule-worktree-sublib");
    fixture.commit_all(&source, "add the package");

    let superproject = fixture.repository_at("superproject");
    fixture.package_at("superproject/app", "workit-fixture-submodule-worktree-app");
    let submodule = fixture.submodule_at(&superproject, &source, "vendor/lib");

    // A linked worktree of the submodule keeps its git directory under
    // `.git/modules/vendor/lib/worktrees/<name>`. That directory holds a
    // `commondir`, and it also lies in the `modules` directory of the
    // superproject, beside a `HEAD` file. It is a second checkout of the
    // submodule all the same, so the `commondir` decides.
    fixture.worktree_at(&submodule, "superproject/wt/lib-feat");

    let output = fixture.workit(&superproject, &[]);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec!["app".to_string(), "vendor/lib".to_string()],
        "a linked worktree of a submodule is a second checkout of the submodule, so its package \
         is left out, and the package of the submodule itself is listed one time:\n{}",
        said(&output)
    );
}

#[test]
fn a_separate_git_dir_repository_inside_a_worktree_below_the_search_path_is_listed() {
    let fixture = Fixture::new();
    let superproject = fixture.repository_at("superproject");
    let worktree = fixture.worktree_at(&superproject, "superproject/wt/feat");

    // `git init --separate-git-dir` keeps the git directory elsewhere and
    // leaves a `.git` file, as a linked worktree and a submodule do. Its git
    // directory is neither one's: it holds no `commondir`, and it lies in no
    // `modules` directory of another git directory. The repository is one of
    // its own, as a repository with a `.git` directory at the same place is.
    //
    // The git directory lies in a directory named `modules` all the same,
    // beside no `HEAD` file, so a walk that read the name alone would call
    // this repository a submodule.
    let nested = worktree.join("nested");
    let separate = fixture.path("modules/separate.git");
    std::fs::create_dir_all(fixture.path("modules"))
        .expect("the directory that holds the separate git directory is created");
    fixture.git(
        &worktree,
        &[
            "init",
            "-q",
            "-b",
            &unique("main"),
            "--separate-git-dir",
            &separate.to_string_lossy(),
            &nested.to_string_lossy(),
        ],
    );
    fixture.assert_toplevel(&nested);
    assert!(
        nested.join(".git").is_file(),
        "a repository made with --separate-git-dir keeps its git link as a file: {}",
        nested.join(".git").display()
    );
    fixture.package_at(
        "superproject/wt/feat/nested",
        "workit-fixture-separate-nested",
    );

    let output = fixture.workit(&superproject, &[]);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec!["wt/feat/nested".to_string()],
        "a repository of its own inside a linked worktree holds packages nothing else holds, \
         whether its git directory sits beside it or elsewhere:\n{}",
        said(&output)
    );
}

#[test]
fn a_git_file_that_names_no_git_directory_inside_a_worktree_ends_the_walk() {
    let fixture = Fixture::new();
    let superproject = fixture.repository_at("superproject");
    fixture.worktree_at(&superproject, "superproject/wt/feat");

    // Git itself refuses to walk past a `.git` file that it cannot follow: it
    // stops with "invalid gitfile format", or with "gitfile does not point to
    // a valid repository". Neither file puts its directory in the checkout
    // above it.
    fixture.package_at(
        "superproject/wt/feat/garbled",
        "workit-fixture-garbled-link",
    );
    std::fs::write(
        fixture.path("superproject/wt/feat/garbled/.git"),
        "not a git link\n",
    )
    .expect("the garbled git link is written");
    fixture.package_at(
        "superproject/wt/feat/dangling",
        "workit-fixture-dangling-link",
    );
    std::fs::write(
        fixture.path("superproject/wt/feat/dangling/.git"),
        format!("gitdir: {}\n", fixture.path("missing.git").display()),
    )
    .expect("the dangling git link is written");

    let output = fixture.workit(&superproject, &[]);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec![
            "wt/feat/dangling".to_string(),
            "wt/feat/garbled".to_string()
        ],
        "a `.git` file that names no git directory links no checkout, so the walk stops at it as \
         git does, and the worktree above it has no say:\n{}",
        said(&output)
    );
}

#[test]
fn an_empty_result_the_worktree_filter_caused_names_the_flag() {
    let fixture = Fixture::new();
    let repository = fixture.repository_at("checkout");
    fixture.worktree_at(&repository, "second");
    fixture.package_at("second/delta", "workit-fixture-empty-delta");

    let root = fixture.root.clone();
    let output = fixture.workit(&root, &[]);

    assert_succeeded(&output);
    let said = said(&output);
    assert!(
        said.contains("worktree"),
        "a run that found packages and then filtered every one of them out must not read like a \
         run that found none: it has to say the filter is what emptied the result:\n{said}"
    );
    assert!(
        said.contains("--include-worktrees"),
        "and it has to name the flag that would have kept them:\n{said}"
    );
}

#[test]
fn an_empty_result_nothing_filtered_does_not_name_the_flag() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.path("empty")).expect("the fixture directory is created");

    let root = fixture.root.clone();
    let output = fixture.workit(&root, &[]);

    assert_succeeded(&output);
    let said = said(&output);
    assert!(
        !said.contains("--include-worktrees"),
        "a tree that holds no package at all is not a filtering problem, and pointing at a flag \
         that would change nothing sends the reader the wrong way:\n{said}"
    );
}
