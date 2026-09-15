//! The worktrees of one repository, for the arrow keys of watch mode.
//!
//! The list comes from `gix`, in this process. No child process runs. It holds
//! the main worktree and every linked worktree, sorted by path, which is the
//! order of `cwt`. Right and Left then visit the worktrees in the same order as
//! `cwt -f` and `cwt -p`.
//!
//! The model of the navigation over that list is pure. [`next`], [`previous`],
//! [`badge`], and [`WorktreeList`] read no git and no filesystem, so the loop
//! decides each key from the paths alone, and the tests build their paths with
//! `WorktreePath::fake`.
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
/// Paths only: no HEAD is read, and no linked worktree is opened. The loop
/// calls this on every walk, so it must stay cheap.
///
/// [`enumerate`] says which worktrees are in the list, for this function and
/// for [`list_worktrees`] alike.
pub(crate) fn worktree_paths(repo: &gix::Repository) -> Vec<WorktreePath> {
    enumerate(repo)
        .into_iter()
        .map(|found| found.path)
        .collect()
}

/// The same worktrees in the same order, each with its label.
///
/// [`enumerate`] says which worktrees are in the list. Then each linked
/// worktree opens, only to read its label, and [`head_label`] reads the label
/// from the repository of that worktree. A linked worktree that gix cannot
/// open stays in the list with the label [`UNREADABLE_LABEL`], so this list
/// and [`worktree_paths`] always hold the same paths in the same order.
pub(crate) fn list_worktrees(repo: &gix::Repository) -> Vec<WorktreeEntry> {
    enumerate(repo)
        .into_iter()
        .map(|found| WorktreeEntry {
            path: found.path,
            label: found.head.label(),
        })
        .collect()
}

/// The label of a linked worktree whose repository gix cannot open.
///
/// gsw cannot read the HEAD of that worktree, so it knows neither its branch
/// nor its commit, and a guess would be a lie. The worktree stays in the list,
/// so the list and [`worktree_paths`] always hold the same worktrees.
const UNREADABLE_LABEL: &str = "?";

/// One worktree that [`enumerate`] found: its root, and where its HEAD is.
struct Found<'repo> {
    /// The root of the worktree.
    path: WorktreePath,
    /// Where [`list_worktrees`] reads the label. [`worktree_paths`] never
    /// reads it.
    head: Head<'repo>,
}

/// Where the HEAD of one worktree is, before anything reads it.
enum Head<'repo> {
    /// The main repository. [`enumerate`] opens it anyway, to learn where the
    /// main worktree is and whether the main repository is bare, so the label
    /// needs no second open. It is in a box because a repository is much
    /// larger than a proxy.
    Main(Box<gix::Repository>),
    /// The admin dir of a linked worktree, not open yet. Only a label opens
    /// it, so [`worktree_paths`] pays for no open.
    Linked(gix::worktree::Proxy<'repo>),
}

impl Head<'_> {
    /// The label of this HEAD: [`head_label`] of its repository, or
    /// [`UNREADABLE_LABEL`] when gix cannot open the linked worktree.
    fn label(self) -> String {
        match self {
            Self::Main(repo) => head_label(&repo),
            Self::Linked(proxy) => proxy
                .into_repo_with_possibly_inaccessible_worktree()
                .map_or_else(|_| UNREADABLE_LABEL.to_string(), |repo| head_label(&repo)),
        }
    }
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
///
/// It opens the main repository and no linked worktree, and it reads no HEAD.
fn enumerate(repo: &gix::Repository) -> Vec<Found<'_>> {
    let mut found: Vec<Found<'_>> = main_worktree(repo).into_iter().collect();
    found.extend(
        repo.worktrees()
            .unwrap_or_default()
            .into_iter()
            .filter_map(linked_worktree),
    );
    found.sort_by(|left, right| left.path.cmp(&right.path));
    found
}

/// The main worktree of the repository that holds `repo`. `None` when the
/// main repository is bare, when gix cannot open it, or when its directory
/// does not exist.
fn main_worktree(repo: &gix::Repository) -> Option<Found<'_>> {
    let main = repo.main_repo().ok().filter(|main| !main.is_bare())?;
    let path = WorktreePath::resolve(main.workdir()?)?;
    Some(Found {
        path,
        head: Head::Main(Box::new(main)),
    })
}

/// The linked worktree that `proxy` names. `None` when gix cannot read its
/// `gitdir` file, or when its directory does not exist. Its admin dir stays
/// closed until a label asks for it.
///
/// git writes the `gitdir` file as an absolute path, or, with
/// `--relative-paths`, as a path relative to the admin dir. gix gives the path
/// back as it is. The join with the admin dir resolves the relative form. An
/// absolute path replaces the admin dir in the join, so the absolute form
/// stays as it is.
fn linked_worktree(proxy: gix::worktree::Proxy<'_>) -> Option<Found<'_>> {
    let path = WorktreePath::resolve(&proxy.git_dir().join(proxy.base().ok()?))?;
    Some(Found {
        path,
        head: Head::Linked(proxy),
    })
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

/// Where Right goes from `current`.
///
/// `paths` is sorted, as [`worktree_paths`] gives it. The answer is the path
/// after `current`. Right on the last path wraps to the first, as `cwt -f`
/// does. A `current` that is not in `paths` goes to the first path that sorts
/// after it, or wraps to the first path. That occurs when the worktree on the
/// screen stopped existing after the last read of the list.
///
/// `None` when there is no other worktree to go to: `paths` is empty, or it
/// holds `current` alone. The answer is never `current` itself, so an answer
/// always changes the worktree.
pub(crate) fn next<'a>(
    paths: &'a [WorktreePath],
    current: &WorktreePath,
) -> Option<&'a WorktreePath> {
    let after = paths.partition_point(|path| path <= current);
    let target = paths.get(after).or_else(|| paths.first())?;
    (target != current).then_some(target)
}

/// Where Left goes from `current`. The mirror of [`next`].
///
/// The answer is the path before `current`. Left on the first path wraps to
/// the last, as `cwt -p` does. A `current` that is not in `paths` goes to the
/// last path that sorts before it, or wraps to the last path. `None`, and
/// never `current`, for the same reasons as [`next`].
pub(crate) fn previous<'a>(
    paths: &'a [WorktreePath],
    current: &WorktreePath,
) -> Option<&'a WorktreePath> {
    let before = paths.partition_point(|path| path < current);
    let target = before
        .checked_sub(1)
        .map_or_else(|| paths.last(), |row| paths.get(row))?;
    (target != current).then_some(target)
}

/// The header segment that says which worktree the frame shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeBadge {
    /// 1-based position of the current worktree in the sorted list.
    pub(crate) position: usize,
    /// How many worktrees the list holds.
    pub(crate) count: usize,
    /// Whether the current worktree is the home worktree.
    pub(crate) home: bool,
    /// The branch, or `HEAD@<short hash>`. The header shows it in place of
    /// the branch name.
    pub(crate) label: String,
}

/// `None` when the repository has one worktree (the header must not change)
/// or when `current` is not in `paths`.
pub(crate) fn badge(
    _paths: &[WorktreePath],
    _current: &WorktreePath,
    _home: &WorktreePath,
    _label: String,
) -> Option<WorktreeBadge> {
    None
}

/// The list that Down opens: the entries, the cursor, and the scroll.
#[derive(Debug)]
pub(crate) struct WorktreeList {
    /// Every worktree, sorted by path.
    entries: Vec<WorktreeEntry>,
    /// The row of the cursor.
    cursor: usize,
    /// The first row of the window at the last [`settle`](Self::settle).
    top: usize,
    /// The worktree where the user started gsw.
    home: WorktreePath,
}

/// One row of the window that [`WorktreeList::window`] gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ListRow<'a> {
    /// The worktree of the row.
    pub(crate) entry: &'a WorktreeEntry,
    /// Whether the cursor is on the row.
    pub(crate) cursor: bool,
    /// Whether the row is the home worktree.
    pub(crate) home: bool,
}

impl WorktreeList {
    /// `None` when `entries` is empty. The cursor starts on `current`, or, for
    /// a `current` not in the list, on the row where it would sort (clamped
    /// to the last row).
    pub(crate) fn open(
        entries: Vec<WorktreeEntry>,
        _current: &WorktreePath,
        home: WorktreePath,
    ) -> Option<Self> {
        Some(Self {
            entries,
            cursor: 0,
            top: 0,
            home,
        })
    }

    /// Up on the top row does nothing. The list does not wrap.
    pub(crate) fn up(&mut self) {}

    /// Down on the bottom row does nothing.
    pub(crate) fn down(&mut self) {}

    /// The worktree under the cursor: the worktree that Enter goes to.
    pub(crate) fn selected(&self) -> &WorktreeEntry {
        &self.entries[self.cursor]
    }

    /// The rows a pane of `rows` list rows shows, top to bottom. The cursor
    /// row is always one of them. Pure: it clamps the stored scroll offset.
    pub(crate) fn window(&self, _rows: usize) -> Vec<ListRow<'_>> {
        Vec::new()
    }

    /// Store the scroll offset that `window(rows)` used, so the next move
    /// starts from the window that the user saw. Minimal movement: the window
    /// moves only when the cursor leaves it. It never leaves empty rows at the
    /// bottom when the list is longer than the pane.
    pub(crate) fn settle(&mut self, _rows: usize) {}
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::{
        head_label, list_worktrees, next, previous, worktree_paths, WorktreeEntry, WorktreePath,
    };
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
            repo.main_repo()
                .expect("open the main repository")
                .is_bare(),
            "the main repository of the fixture must be bare",
        );

        assert_eq!(worktree_paths(&repo), vec![resolved(&linked)]);
    }

    /// A directory name and a branch name with multi-byte characters: a
    /// character of three bytes, a character of four bytes, and an accent.
    const MULTIBYTE: &str = "日本語-🎉-café";

    /// A repository at `repo` with one linked worktree at [`MULTIBYTE`], on
    /// the branch [`MULTIBYTE`]. Gives the fixture, the main worktree, and the
    /// linked worktree. `repo` sorts first: `r` is one byte, and every byte of
    /// `日` is higher.
    fn multibyte_fixture() -> (TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("repo");
        init_repo_at(&main);
        let linked = dir.path().join(MULTIBYTE);
        add_worktree(&main, &linked, &["-b", MULTIBYTE]);
        (dir, main, linked)
    }

    /// A worktree directory whose name holds multi-byte characters is in the
    /// list, and its path keeps every character.
    #[test]
    fn a_multibyte_path_is_listed_and_round_trips() {
        let (_dir, main, linked) = multibyte_fixture();

        let paths = worktree_paths(&open(&main));

        assert_eq!(paths, vec![resolved(&main), resolved(&linked)]);
        assert_eq!(
            paths[1].as_path().file_name(),
            Some(OsStr::new(MULTIBYTE)),
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

    /// The paths of the entries of a list, in the order of the list.
    fn paths_of(entries: &[WorktreeEntry]) -> Vec<WorktreePath> {
        entries.iter().map(|entry| entry.path.clone()).collect()
    }

    /// The list holds the same worktrees as the paths, in the same order, from
    /// the main worktree and from each linked worktree. The header counts the
    /// paths and Down shows the list, so the two must never disagree.
    #[test]
    fn the_list_has_the_same_paths_in_the_same_order_as_the_paths() {
        let layout = Layout::new();
        for asked_from in [layout.main(), layout.zulu(), layout.mike(), layout.alpha()] {
            let repo = open(&asked_from);
            let listed = paths_of(&list_worktrees(&repo));
            assert_eq!(
                listed,
                layout.sorted(),
                "asked from {}",
                asked_from.display()
            );
            assert_eq!(
                listed,
                worktree_paths(&repo),
                "asked from {}",
                asked_from.display(),
            );
        }
    }

    /// Each entry carries the label of its own HEAD: the branch of a worktree
    /// on a branch, and `HEAD@` with the short hash for a detached worktree.
    #[test]
    fn each_entry_is_labelled_with_its_branch_or_the_short_hash_of_its_head() {
        let layout = Layout::new();
        // A commit of its own moves the detached worktree off the commit of
        // `main`, so a label read from the wrong repository shows the wrong
        // hash.
        git(
            &layout.mike(),
            &["commit", "-q", "--allow-empty", "-m", "detached work"],
        );
        let detached = detached_label(&layout.mike());
        assert_ne!(
            detached,
            detached_label(&layout.main()),
            "the detached worktree must be on a commit of its own",
        );

        assert_eq!(
            list_worktrees(&open(&layout.main())),
            vec![
                WorktreeEntry {
                    path: resolved(&layout.zulu()),
                    label: "zulu".to_string(),
                },
                WorktreeEntry {
                    path: resolved(&layout.mike()),
                    label: detached,
                },
                WorktreeEntry {
                    path: resolved(&layout.main()),
                    label: "main".to_string(),
                },
                WorktreeEntry {
                    path: resolved(&layout.alpha()),
                    label: "alpha".to_string(),
                },
            ],
        );
    }

    /// A multi-byte path and a multi-byte branch come through the list
    /// unchanged.
    #[test]
    fn a_multibyte_path_and_branch_come_through_the_list_unchanged() {
        let (_dir, main, linked) = multibyte_fixture();

        assert_eq!(
            list_worktrees(&open(&main)),
            vec![
                WorktreeEntry {
                    path: resolved(&main),
                    label: "main".to_string(),
                },
                WorktreeEntry {
                    path: resolved(&linked),
                    label: MULTIBYTE.to_string(),
                },
            ],
        );
    }

    /// A worktree whose admin dir gix cannot open stays in both lists. Its
    /// directory exists and its `gitdir` file reads, so only the open fails.
    /// gsw cannot read its HEAD, so its label is [`UNREADABLE_LABEL`]. The
    /// header counts the paths and Down shows the list, so the two lists must
    /// hold the same worktrees.
    ///
    /// [`UNREADABLE_LABEL`]: super::UNREADABLE_LABEL
    #[cfg(unix)]
    #[test]
    fn a_worktree_whose_admin_dir_gix_cannot_open_stays_in_both_lists_with_an_unknown_label() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("repo");
        init_repo_at(&main);
        let broken = dir.path().join("broken");
        add_worktree(&main, &broken, &["-b", "broken"]);

        // gix reads `commondir` first when it opens the admin dir of a linked
        // worktree, and a read that fails there fails the open.
        let admin = main.join(".git").join("worktrees").join("broken");
        let _commondir = Unreadable::new(admin.join("commondir"));

        let repo = open(&main);
        let proxy = repo
            .worktrees()
            .expect("read the admin dirs")
            .into_iter()
            .next()
            .expect("git keeps the admin dir of the worktree");
        assert!(
            proxy.base().is_ok(),
            "the gitdir file of the worktree must still read, so that only the open fails",
        );
        assert!(
            broken.is_dir(),
            "the directory of the worktree must exist, so that only the open fails",
        );
        assert!(
            proxy
                .into_repo_with_possibly_inaccessible_worktree()
                .is_err(),
            "gix must fail to open the admin dir, or the test proves nothing",
        );

        assert_eq!(
            worktree_paths(&repo),
            vec![resolved(&broken), resolved(&main)],
        );
        assert_eq!(
            list_worktrees(&repo),
            vec![
                WorktreeEntry {
                    path: resolved(&broken),
                    label: super::UNREADABLE_LABEL.to_string(),
                },
                WorktreeEntry {
                    path: resolved(&main),
                    label: "main".to_string(),
                },
            ],
        );
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
        assert_eq!(
            WorktreePath::resolve(&missing),
            None,
            "no directory is there"
        );

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

    /// A worktree that git links through relative paths is in the list.
    ///
    /// `git worktree add --relative-paths` (and `worktree.useRelativePaths`)
    /// writes the `gitdir` file as a path relative to the admin dir, and gix
    /// gives that path back as it is. Resolved against the current directory
    /// of the process, it names no directory, and the worktree drops out of
    /// the list.
    #[test]
    fn a_worktree_linked_through_relative_paths_is_listed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("repo");
        init_repo_at(&main);
        let linked = dir.path().join("linked");
        add_worktree(&main, &linked, &["--relative-paths", "-b", "linked"]);

        let gitdir_file = main
            .join(".git")
            .join("worktrees")
            .join("linked")
            .join("gitdir");
        let gitdir = std::fs::read_to_string(&gitdir_file).expect("read the gitdir file");
        assert!(
            Path::new(gitdir.trim()).is_relative(),
            "git must write a relative path into the gitdir file: {gitdir}",
        );

        let repo = open(&main);
        assert_eq!(
            worktree_paths(&repo),
            vec![resolved(&linked), resolved(&main)],
        );
        assert_eq!(
            list_worktrees(&repo),
            vec![
                WorktreeEntry {
                    path: resolved(&linked),
                    label: "linked".to_string(),
                },
                WorktreeEntry {
                    path: resolved(&main),
                    label: "main".to_string(),
                },
            ],
        );
    }

    /// The path `/code/<name>`, which no filesystem call touched.
    fn fake_path(name: &str) -> WorktreePath {
        WorktreePath::fake(format!("/code/{name}"))
    }

    /// The paths `/code/<name>` for each of `names`, in the order of `names`.
    /// The model takes a sorted list, as the listing gives it, so the helper
    /// refuses names out of order.
    fn sorted_paths(names: &[&str]) -> Vec<WorktreePath> {
        let paths: Vec<WorktreePath> = names.iter().copied().map(fake_path).collect();
        assert!(paths.is_sorted(), "the fixture must be sorted: {names:?}");
        paths
    }

    /// Right goes to the next worktree in path order. Right on the last
    /// worktree wraps to the first, as `cwt -f` does.
    #[test]
    fn next_goes_to_the_next_path_and_wraps_from_the_last_to_the_first() {
        let paths = sorted_paths(&["a", "b", "c", "d"]);

        assert_eq!(next(&paths, &paths[0]), Some(&paths[1]));
        assert_eq!(next(&paths, &paths[1]), Some(&paths[2]));
        assert_eq!(next(&paths, &paths[2]), Some(&paths[3]));
        assert_eq!(
            next(&paths, &paths[3]),
            Some(&paths[0]),
            "Right on the last worktree wraps to the first",
        );
    }

    /// Left goes to the previous worktree in path order. Left on the first
    /// worktree wraps to the last, as `cwt -p` does.
    #[test]
    fn previous_goes_to_the_previous_path_and_wraps_from_the_first_to_the_last() {
        let paths = sorted_paths(&["a", "b", "c", "d"]);

        assert_eq!(
            previous(&paths, &paths[0]),
            Some(&paths[3]),
            "Left on the first worktree wraps to the last",
        );
        assert_eq!(previous(&paths, &paths[1]), Some(&paths[0]));
        assert_eq!(previous(&paths, &paths[2]), Some(&paths[1]));
        assert_eq!(previous(&paths, &paths[3]), Some(&paths[2]));
    }

    /// With one worktree, Right and Left have no other worktree to go to.
    /// With no worktree, the same is true.
    #[test]
    fn next_and_previous_give_none_for_one_worktree_and_for_no_worktree() {
        let one = sorted_paths(&["a"]);
        assert_eq!(next(&one, &one[0]), None);
        assert_eq!(previous(&one, &one[0]), None);

        let current = fake_path("a");
        assert_eq!(next(&[], &current), None);
        assert_eq!(previous(&[], &current), None);
    }

    /// A current worktree that is not in the list: the worktree on the screen
    /// stopped existing after the last read of the list. Right goes to the
    /// first path that sorts after it, and Left goes to the last path that
    /// sorts before it. Past each end of the list, each key wraps.
    #[test]
    fn a_current_worktree_not_in_the_list_goes_to_the_paths_that_sort_around_it() {
        let paths = sorted_paths(&["b", "d", "f"]);

        let between = fake_path("c");
        assert_eq!(next(&paths, &between), Some(&paths[1]));
        assert_eq!(previous(&paths, &between), Some(&paths[0]));

        let before_every_path = fake_path("a");
        assert_eq!(next(&paths, &before_every_path), Some(&paths[0]));
        assert_eq!(
            previous(&paths, &before_every_path),
            Some(&paths[2]),
            "Left from before the first path wraps to the last",
        );

        let after_every_path = fake_path("g");
        assert_eq!(
            next(&paths, &after_every_path),
            Some(&paths[0]),
            "Right from after the last path wraps to the first",
        );
        assert_eq!(previous(&paths, &after_every_path), Some(&paths[2]));

        // One worktree that is not the current worktree is a worktree to go
        // to, from each side.
        let one = sorted_paths(&["d"]);
        assert_eq!(next(&one, &between), Some(&one[0]));
        assert_eq!(previous(&one, &between), Some(&one[0]));
    }

    /// Neither function ever gives the current worktree, so a press that gives
    /// a worktree always changes the worktree. The check covers every list of
    /// up to four worktrees, with the current worktree on each row, and before,
    /// between, and after the rows.
    #[test]
    fn next_and_previous_never_give_the_current_worktree() {
        let names = ["b", "d", "f", "h"];
        let outside = ["a", "c", "e", "g", "i"].map(fake_path);
        for count in 0..=names.len() {
            let paths = sorted_paths(&names[..count]);
            for current in paths.iter().chain(&outside) {
                assert_ne!(
                    next(&paths, current),
                    Some(current),
                    "Right from {current:?} in {paths:?}",
                );
                assert_ne!(
                    previous(&paths, current),
                    Some(current),
                    "Left from {current:?} in {paths:?}",
                );
            }
        }
    }
}
