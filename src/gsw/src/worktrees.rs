//! The worktrees of one repository, for the arrow keys of watch mode.
//!
//! The list comes from `gix`, in this process. No child process runs. It holds
//! the main worktree and every linked worktree, sorted by path, which is the
//! order of `cwt`. Right and Left then visit the worktrees in the same order as
//! `cwt -f` and `cwt -p`.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "watch mode does not use the list yet. The expectation fails the build when it \
                  does, so this attribute cannot stay after that"
    )
)]

use std::path::{Path, PathBuf};

use crate::repo::{branch_name, DETACHED_HEAD};

/// The root of a worktree, in the one spelling that every comparison uses.
///
/// Built only through [`resolve`](Self::resolve) (canonicalize) in production,
/// so a path from the list and a path from the loop always compare correctly.
/// macOS gives `/var/...` and `/private/var/...` for one directory, and a
/// symlinked home directory does the same.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct WorktreePath(PathBuf);

impl WorktreePath {
    /// `std::fs::canonicalize(path)`, or `None` when no directory is there.
    ///
    /// A file at `path` gives `None` too, because a file is not the root of a
    /// worktree.
    pub(crate) fn resolve(path: &Path) -> Option<Self> {
        std::fs::canonicalize(path)
            .ok()
            .filter(|resolved| resolved.is_dir())
            .map(Self)
    }

    /// The path, for display and for the calls that open the worktree.
    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }

    /// A path that no filesystem call touched, for the pure tests only.
    #[cfg(test)]
    pub(crate) fn fake(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }
}

/// One worktree of the repository, as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeEntry {
    /// The root of the worktree.
    pub(crate) path: WorktreePath,
    /// The branch name, or `HEAD@<7 hex chars>` for a detached HEAD.
    pub(crate) label: String,
}

/// Every worktree of the repository that holds `repo`, sorted by path.
/// Paths only: no HEAD is read. The loop calls this on every walk.
///
/// [`enumerate`] says which worktrees are in the list.
pub(crate) fn worktree_paths(repo: &gix::Repository) -> Vec<WorktreePath> {
    enumerate(repo)
}

/// Every worktree of the repository that holds `repo`, sorted by path.
///
/// - The main worktree, from [`gix::Repository::main_repo`], unless the main
///   repository is bare.
/// - Every linked worktree, from [`gix::Repository::worktrees`].
///
/// A worktree whose directory does not exist is skipped (git calls it
/// prunable). So is a worktree whose `gitdir` file gix cannot read. A
/// `worktrees` directory that cannot be read hides every linked worktree, and
/// the main worktree stays in the list. No read fails the whole list.
///
/// gix gives the linked worktrees sorted by their admin dir
/// (`.git/worktrees/<id>`), which is not the order of their paths. The sort
/// here is by [`WorktreePath`], component by component, as `cwt` sorts by
/// `PathBuf`.
fn enumerate(repo: &gix::Repository) -> Vec<WorktreePath> {
    let mut found: Vec<WorktreePath> = main_worktree(repo).into_iter().collect();
    found.extend(
        repo.worktrees()
            .unwrap_or_default()
            .iter()
            .filter_map(linked_worktree),
    );
    found.sort();
    found
}

/// The main worktree of the repository that holds `repo`. `None` when the
/// main repository is bare, when gix cannot open it, or when its directory
/// does not exist.
fn main_worktree(repo: &gix::Repository) -> Option<WorktreePath> {
    let main = repo.main_repo().ok().filter(|main| !main.is_bare())?;
    WorktreePath::resolve(main.workdir()?)
}

/// The linked worktree that `proxy` names. `None` when gix cannot read its
/// `gitdir` file, or when its directory does not exist.
fn linked_worktree(proxy: &gix::worktree::Proxy<'_>) -> Option<WorktreePath> {
    WorktreePath::resolve(&proxy.base().ok()?)
}

/// The same worktrees in the same order, each with its label.
pub(crate) fn list_worktrees(_repo: &gix::Repository) -> Vec<WorktreeEntry> {
    Vec::new()
}

/// How many hex digits of the commit the label of a detached HEAD shows.
///
/// The length that `cwt` shows (`SHORT_COMMIT_HASH_LENGTH` in
/// `src/cwt/src/worktree.rs`), so the two tools name a detached worktree the
/// same way. `cwt` is a binary crate, so gsw cannot take its constant.
const SHORT_HASH_LEN: usize = 7;

/// The label of `repo`'s own HEAD: the branch, or `HEAD@<short hash>`.
///
/// [`branch_name`] gives the branch. For a detached HEAD it gives
/// [`DETACHED_HEAD`], and the label then adds `@` and the first
/// [`SHORT_HASH_LEN`] hex digits of the commit, as `cwt` does. A detached HEAD
/// whose commit gix cannot read gives [`DETACHED_HEAD`] alone: the HEAD is
/// detached, and no hash is there to show.
pub(crate) fn head_label(repo: &gix::Repository) -> String {
    let branch = branch_name(repo);
    if branch != DETACHED_HEAD {
        return branch;
    }
    match repo.head_id() {
        Ok(id) => format!("{DETACHED_HEAD}@{}", id.to_hex_with_len(SHORT_HASH_LEN)),
        Err(_) => branch,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::{head_label, worktree_paths, WorktreePath};
    use crate::testrepo::{git, git_stdout, init_repo, init_repo_at};

    /// How many hex digits of the commit a detached HEAD shows: the length
    /// that `cwt` shows (`SHORT_COMMIT_HASH_LENGTH` in
    /// `src/cwt/src/worktree.rs`). Stated here as the oracle, apart from the
    /// constant of the code under test.
    const CWT_SHORT_HASH: usize = 7;

    /// Open the repository at `path` through the discovery that
    /// `RepoHandle::discover` makes, which is how gsw opens a worktree.
    fn open(path: &Path) -> gix::Repository {
        gix::discover(path).expect("the fixture is a repository")
    }

    /// The label that `cwt` shows for a detached HEAD at `dir`: `HEAD@` and
    /// the first [`CWT_SHORT_HASH`] hex digits of the id that git reports.
    fn detached_label(dir: &Path) -> String {
        let full = git_stdout(dir, &["rev-parse", "HEAD"]);
        let short: String = full.chars().take(CWT_SHORT_HASH).collect();
        format!("HEAD@{short}")
    }

    /// `head_label` gives the branch of a worktree on a branch. For a detached
    /// HEAD it gives `HEAD@` and the short hash, as `cwt` does, because a
    /// detached worktree has no branch to show.
    #[test]
    fn head_label_gives_the_branch_or_the_short_hash_of_a_detached_head() {
        let dir = init_repo();
        assert_eq!(head_label(&open(dir.path())), "main");

        git(dir.path(), &["checkout", "-q", "--detach"]);
        assert_eq!(head_label(&open(dir.path())), detached_label(dir.path()));
    }

    /// Add a linked worktree of the repository at `main`, at `path`. `how`
    /// holds the options of `git worktree add` that pick the HEAD: a new
    /// branch (`-b <name>`) or `--detach`.
    fn add_worktree(main: &Path, path: &Path, how: &[&str]) {
        let parent = path.parent().expect("a worktree path has a parent");
        std::fs::create_dir_all(parent).expect("make the parent directory");
        let mut args = vec!["worktree", "add", "-q"];
        args.extend_from_slice(how);
        args.push(path.to_str().expect("utf-8 tempdir path"));
        git(main, &args);
    }

    /// The [`WorktreePath`] of a directory that the fixture made.
    fn resolved(path: &Path) -> WorktreePath {
        WorktreePath::resolve(path).expect("the fixture made this directory")
    }

    /// A repository whose worktrees gix gives in an order that is not the
    /// order of their paths. One [`TempDir`] holds every checkout:
    ///
    /// | Path      | Worktree                    | Admin dir         |
    /// | --------- | --------------------------- | ----------------- |
    /// | `a/zulu`  | linked, on branch `zulu`    | `worktrees/zulu`  |
    /// | `b/mike`  | linked, detached            | `worktrees/mike`  |
    /// | `bb/gone` | linked, directory deleted   | `worktrees/gone`  |
    /// | `c/repo`  | main, on branch `main`      | `.git`            |
    /// | `d/alpha` | linked, on branch `alpha`   | `worktrees/alpha` |
    ///
    /// gix gives the linked worktrees sorted by admin dir: `alpha`, `gone`,
    /// `mike`, `zulu`. That is the reverse of their path order. The main
    /// worktree sorts third, so a list that puts it first is wrong too. The
    /// fixture makes the worktrees in a third order (`alpha`, `gone`, `zulu`,
    /// `mike`), so the order of creation cannot give the right answer by
    /// chance.
    struct Layout {
        /// Holds every checkout. The drop deletes the fixture.
        dir: TempDir,
    }

    impl Layout {
        /// Make the repository, its worktrees, and the deletion.
        fn new() -> Self {
            let layout = Self {
                dir: tempfile::tempdir().expect("tempdir"),
            };
            let main = layout.main();
            init_repo_at(&main);
            add_worktree(&main, &layout.alpha(), &["-b", "alpha"]);
            add_worktree(&main, &layout.gone(), &["-b", "gone"]);
            add_worktree(&main, &layout.zulu(), &["-b", "zulu"]);
            add_worktree(&main, &layout.mike(), &["--detach"]);
            std::fs::remove_dir_all(layout.gone()).expect("delete the directory of `gone`");
            layout
        }

        /// The checkout `name` in the directory `parent` of the fixture.
        fn at(&self, parent: &str, name: &str) -> PathBuf {
            self.dir.path().join(parent).join(name)
        }

        /// The main worktree.
        fn main(&self) -> PathBuf {
            self.at("c", "repo")
        }

        /// The linked worktree on branch `zulu`. Its path sorts first.
        fn zulu(&self) -> PathBuf {
            self.at("a", "zulu")
        }

        /// The detached linked worktree.
        fn mike(&self) -> PathBuf {
            self.at("b", "mike")
        }

        /// The linked worktree whose directory the fixture deleted.
        fn gone(&self) -> PathBuf {
            self.at("bb", "gone")
        }

        /// The linked worktree on branch `alpha`. Its path sorts last.
        fn alpha(&self) -> PathBuf {
            self.at("d", "alpha")
        }

        /// Every worktree whose directory exists, in path order.
        fn sorted(&self) -> Vec<WorktreePath> {
            [self.zulu(), self.mike(), self.main(), self.alpha()]
                .iter()
                .map(|path| resolved(path))
                .collect()
        }
    }

    /// The paths are sorted by path. The main worktree is among them and is
    /// not first. A detached worktree is in the list, and a worktree whose
    /// directory was deleted is not.
    #[test]
    fn the_paths_are_sorted_by_path_with_the_main_worktree_among_them() {
        let layout = Layout::new();
        let repo = open(&layout.main());

        // The fixture proves the sort and the skip only if gix gives the
        // linked worktrees out of path order and still knows the deleted one.
        let admin_order: Vec<PathBuf> = repo
            .worktrees()
            .expect("read the admin dirs")
            .iter()
            .map(|proxy| proxy.base().expect("read the gitdir file"))
            .collect();
        assert_eq!(
            admin_order.len(),
            4,
            "git must still hold the admin dir of the deleted worktree: {admin_order:?}",
        );
        assert!(
            !admin_order.windows(2).all(|pair| pair[0] <= pair[1]),
            "gix must give the linked worktrees out of path order: {admin_order:?}",
        );
        assert_eq!(
            git_stdout(&layout.mike(), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "HEAD",
            "one worktree of the fixture must be detached",
        );

        assert_eq!(worktree_paths(&repo), layout.sorted());
    }

    /// The paths are the same whether the loop asks from the main worktree or
    /// from a linked worktree, because the user can start gsw in either.
    #[test]
    fn the_paths_are_the_same_from_the_main_worktree_and_from_a_linked_worktree() {
        let layout = Layout::new();
        let from_main = worktree_paths(&open(&layout.main()));
        assert_eq!(from_main, layout.sorted());

        for linked in [layout.zulu(), layout.mike(), layout.alpha()] {
            assert_eq!(
                worktree_paths(&open(&linked)),
                from_main,
                "asked from {}",
                linked.display(),
            );
        }
    }

    /// A bare main repository has no checkout to show, so the list skips it.
    /// Its linked worktree is in the list.
    #[test]
    fn a_bare_main_repository_is_skipped_and_its_linked_worktree_is_listed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let origin = dir.path().join("origin");
        init_repo_at(&origin);
        let bare = dir.path().join("bare.git");
        git(
            dir.path(),
            &[
                "clone",
                "-q",
                "--bare",
                origin.to_str().expect("utf-8 tempdir path"),
                bare.to_str().expect("utf-8 tempdir path"),
            ],
        );
        let linked = dir.path().join("linked");
        add_worktree(&bare, &linked, &["-b", "linked"]);

        let repo = open(&linked);
        assert!(
            repo.main_repo().expect("open the main repository").is_bare(),
            "the main repository of the fixture must be bare",
        );

        assert_eq!(worktree_paths(&repo), vec![resolved(&linked)]);
    }

    /// A worktree directory whose name holds multi-byte characters is in the
    /// list, and its path keeps every character.
    #[test]
    fn a_multibyte_path_is_listed_and_round_trips() {
        const NAME: &str = "日本語-🎉-café";
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("repo");
        init_repo_at(&main);
        let linked = dir.path().join(NAME);
        add_worktree(&main, &linked, &["-b", NAME]);

        let paths = worktree_paths(&open(&main));

        // `repo` sorts before `日本語…`: `r` is one byte, and every byte of
        // `日` is higher.
        assert_eq!(paths, vec![resolved(&main), resolved(&linked)]);
        assert_eq!(
            paths[1].as_path().file_name(),
            Some(OsStr::new(NAME)),
            "the path must keep every character of the directory name",
        );
    }

    /// Takes every permission away from a path, and gives the permissions
    /// back at the drop, so that the [`TempDir`] can delete the fixture.
    #[cfg(unix)]
    struct Unreadable {
        /// The path that has no permissions.
        path: PathBuf,
        /// The permissions that the drop gives back.
        before: std::fs::Permissions,
    }

    #[cfg(unix)]
    impl Unreadable {
        /// Take every permission away from `path`.
        ///
        /// # Panics
        ///
        /// Panics when the path stays readable, for example for root. A test
        /// that reads it then proves nothing about a read that fails.
        fn new(path: PathBuf) -> Self {
            use std::os::unix::fs::PermissionsExt;

            let before = std::fs::metadata(&path)
                .expect("read the permissions")
                .permissions();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
                .expect("take the permissions away");
            assert!(
                std::fs::File::open(&path).is_err(),
                "{} must be unreadable, or the test proves nothing",
                path.display(),
            );
            Self { path, before }
        }
    }

    #[cfg(unix)]
    impl Drop for Unreadable {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.path, self.before.clone());
        }
    }

    /// A worktree whose admin files gix cannot read is skipped. The read does
    /// not panic and does not fail the list: the other worktrees are in it.
    /// One worktree has a `gitdir` file that cannot be read, and one has an
    /// admin dir that cannot be read.
    #[cfg(unix)]
    #[test]
    fn a_worktree_whose_admin_files_cannot_be_read_is_skipped_and_the_rest_are_listed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("repo");
        init_repo_at(&main);
        let kept = dir.path().join("kept");
        add_worktree(&main, &kept, &["-b", "kept"]);
        add_worktree(&main, &dir.path().join("no-gitdir"), &["-b", "no-gitdir"]);
        add_worktree(&main, &dir.path().join("no-admin"), &["-b", "no-admin"]);

        let admin = main.join(".git").join("worktrees");
        let _gitdir_file = Unreadable::new(admin.join("no-gitdir").join("gitdir"));
        let _admin_dir = Unreadable::new(admin.join("no-admin"));

        assert_eq!(
            worktree_paths(&open(&main)),
            vec![resolved(&kept), resolved(&main)],
        );
    }

    /// A `worktrees` directory that cannot be read hides every linked
    /// worktree, and the list still holds the main worktree.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_worktrees_directory_still_lists_the_main_worktree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("repo");
        init_repo_at(&main);
        add_worktree(&main, &dir.path().join("linked"), &["-b", "linked"]);

        let _worktrees = Unreadable::new(main.join(".git").join("worktrees"));

        assert_eq!(worktree_paths(&open(&main)), vec![resolved(&main)]);
    }

    /// Two spellings of one directory resolve to one value.
    ///
    /// The loop compares the path of the worktree on the screen with the paths
    /// of the list. A symlink gives a second spelling on every Unix, and on
    /// macOS every temporary directory has two: `/var/...` and
    /// `/private/var/...`. Two values for one directory make Right skip a
    /// worktree or stop on the same one twice.
    #[cfg(unix)]
    #[test]
    fn resolve_gives_one_value_for_two_spellings_of_one_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("make the directory");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("make the symlink");

        let through_real = WorktreePath::resolve(&real).expect("a directory is there");
        let through_link = WorktreePath::resolve(&link).expect("the symlink names a directory");

        assert_eq!(
            through_real, through_link,
            "two spellings of one directory must give one value",
        );
    }

    /// `resolve` gives a directory, and gives `None` for a path where no
    /// directory is. `fake` keeps the path that `resolve` refuses, because it
    /// touches no filesystem.
    #[test]
    fn resolve_gives_a_directory_and_refuses_a_missing_path_and_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(
            WorktreePath::resolve(dir.path()).is_some(),
            "a directory is there, so resolve must give it",
        );

        let missing = dir.path().join("no-such-directory");
        assert_eq!(WorktreePath::resolve(&missing), None, "no directory is there");

        let file = dir.path().join("a-file");
        std::fs::write(&file, "").expect("write the file");
        assert_eq!(
            WorktreePath::resolve(&file),
            None,
            "a file is not the root of a worktree",
        );

        assert_eq!(
            WorktreePath::fake(&missing).as_path(),
            missing,
            "fake touches no filesystem, so it keeps a path that resolve refuses",
        );
    }
}
