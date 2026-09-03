use ignore::WalkBuilder;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::{DirEntry, WalkDir};

/// Walks the file system upward from the current directory for a `.git` entry.
///
/// This stays a file system walk on purpose, and a caller that wants git's own
/// answer calls [`find_repo_context`] instead.
///
/// The destructive tools read this result and then walk it, deleting files:
/// `gitnuke`, `nodenuke`, `cdknuke`, `repotidy`, `polish`, `rr`, `reposize`,
/// `goup`, `glo` and `nodeup`. Blindness to a detached git directory is the
/// safe answer for them. In that layout the work tree is `$HOME`, so a walk
/// that found the repository would hand them `$HOME` to delete inside. Today
/// they find no repository and do nothing, which is what they must keep doing.
pub fn find_git_repo() -> Option<PathBuf> {
    let mut current_dir = env::current_dir().ok()?;

    loop {
        let git_dir = current_dir.join(".git");
        if git_dir.exists() {
            return Some(current_dir);
        }

        if !current_dir.pop() {
            break;
        }
    }

    None
}

pub fn is_git_worktree(dir: &Path) -> bool {
    let git_path = dir.join(".git");

    if git_path.is_file() {
        if let Ok(content) = fs::read_to_string(&git_path) {
            return content.trim().starts_with("gitdir:");
        }
    }

    false
}

/// Finds the root of the main git repository, even when called from a worktree.
///
/// Unlike `find_git_repo()` which returns the current worktree directory if inside one,
/// this function always returns the path to the main repository (where `.git` is a directory,
/// not a file).
///
/// Uses `git rev-parse --git-common-dir` to find the common git directory, which points
/// to the main repository's `.git` directory for both worktrees and the main repo.
pub fn find_main_repo() -> Option<PathBuf> {
    let git_dir = find_git_repo()?;
    let git_path = git_dir.join(".git");

    if git_path.is_dir() {
        // Already in the main repo
        return Some(git_dir);
    }

    // We're in a worktree - use git to find the common directory
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(&git_dir)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // Output is like: /path/to/main-repo/.git (usually absolute, but can be relative)
    let common_dir = String::from_utf8(output.stdout).ok()?.trim().to_string();
    let common_path = PathBuf::from(&common_dir);

    // Handle both absolute and relative paths from git rev-parse.
    // In practice, --git-common-dir returns absolute paths from worktrees because the
    // worktree's .git file contains an absolute gitdir: reference. However, git doesn't
    // guarantee this, so we handle relative paths defensively by joining with git_dir.
    let common_path = if common_path.is_absolute() {
        common_path
    } else {
        git_dir.join(&common_path)
    };

    // The parent of the .git directory is the repo root
    common_path.parent().map(|p| p.to_path_buf())
}

/// What git says about the repository a directory belongs to.
///
/// Every field comes from git, so the answer holds for a layout no file system
/// walk can read. The one that matters is a detached git directory, which is
/// how `yadm` keeps a directory of dotfiles: the git directory is
/// `~/.local/share/yadm/repo.git`, the work tree is `$HOME`, and no `.git`
/// entry exists anywhere for a walk to find.
///
/// The work tree is deliberately absent from this interface. In that layout it
/// is `$HOME`, and a caller that walks and deletes must never receive it.
#[derive(Debug, Clone)]
pub struct RepoContext {
    checkout: PathBuf,
    main_worktree: PathBuf,
}

impl RepoContext {
    /// The root of the checkout the directory is in.
    ///
    /// This is the linked worktree when the directory is in one. Otherwise it
    /// is the main worktree, which git names as the git directory itself when
    /// the git directory is detached.
    pub fn checkout(&self) -> &Path {
        &self.checkout
    }

    /// The main worktree, as `git worktree list --porcelain` names it.
    ///
    /// Git builds that name from the common git directory with a trailing
    /// `/.git` removed. A detached git directory carries no such suffix, so the
    /// main worktree of a `yadm` repository is the git directory itself.
    pub fn main_worktree(&self) -> &Path {
        &self.main_worktree
    }
}

/// Asks git which repository the process's current directory belongs to.
///
/// Returns `None` when the directory is in no repository, or when git cannot
/// answer.
pub fn find_repo_context() -> Option<RepoContext> {
    None
}

/// Asks git which repository `dir` belongs to.
///
/// This is the seam the tests use. A test cannot change the process directory
/// without racing every other test in the binary.
///
/// Returns `None` when `dir` is in no repository, or when git cannot answer.
pub fn find_repo_context_at(dir: &Path) -> Option<RepoContext> {
    let main_worktree = PathBuf::from(
        git_stdout(dir, &["worktree", "list", "--porcelain"])?
            .lines()
            .find_map(|line| line.strip_prefix(WORKTREE_LINE_PREFIX))?,
    );

    Some(RepoContext {
        checkout: main_worktree.clone(),
        main_worktree,
    })
}

/// The prefix of the line that names a worktree in `git worktree list
/// --porcelain`. The first such line names the main worktree.
const WORKTREE_LINE_PREFIX: &str = "worktree ";

/// Run git in `dir` and hand back its standard output, or `None` when git
/// cannot be spawned or exits non-zero.
fn git_stdout(dir: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    // This crate's whole job is to answer which repository a directory belongs
    // to, and an inherited `GIT_DIR` answers with a different repository. The
    // tests below prove it, so without this they fail under the pre-commit
    // hook, which exports `GIT_DIR` and `GIT_INDEX_FILE` to every command it
    // runs.
    gitscratch::shed_inherited_git_environment(&mut command);

    let output = command.args(args).current_dir(dir).output().ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout).ok()
}

pub struct RepoWalker {
    root: PathBuf,
    skip_node_modules: bool,
    skip_worktrees: bool,
    respect_gitignore: bool,
    include_hidden: bool,
}

impl RepoWalker {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            skip_node_modules: true,
            skip_worktrees: true,
            respect_gitignore: true,
            include_hidden: false,
        }
    }

    pub fn skip_node_modules(mut self, skip: bool) -> Self {
        self.skip_node_modules = skip;
        self
    }

    pub fn skip_worktrees(mut self, skip: bool) -> Self {
        self.skip_worktrees = skip;
        self
    }

    pub fn respect_gitignore(mut self, respect: bool) -> Self {
        self.respect_gitignore = respect;
        self
    }

    pub fn include_hidden(mut self, include: bool) -> Self {
        self.include_hidden = include;
        self
    }

    pub fn walk_with_walkdir(&self) -> impl Iterator<Item = DirEntry> {
        let root = self.root.clone();
        let skip_node_modules = self.skip_node_modules;
        let skip_worktrees = self.skip_worktrees;

        WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(move |e| {
                if skip_node_modules && e.file_name() == "node_modules" {
                    return false;
                }

                if skip_worktrees
                    && e.file_type().is_dir()
                    && is_git_worktree(e.path())
                    && e.path() != root
                {
                    println!("Skipping git worktree directory: {}", e.path().display());
                    return false;
                }

                true
            })
            .filter_map(|e| e.ok())
    }

    pub fn walk_with_ignore(&self) -> impl Iterator<Item = ignore::DirEntry> + '_ {
        let mut builder = WalkBuilder::new(&self.root);

        builder
            .git_ignore(self.respect_gitignore)
            .git_global(self.respect_gitignore)
            .git_exclude(self.respect_gitignore)
            .hidden(!self.include_hidden);

        if self.skip_node_modules {
            builder.filter_entry(move |entry| entry.file_name() != "node_modules");
        }

        if self.skip_worktrees {
            let root = self.root.clone();
            builder.filter_entry(move |entry| {
                if entry.file_type().is_some_and(|ft| ft.is_dir())
                    && is_git_worktree(entry.path())
                    && entry.path() != root
                {
                    println!(
                        "Skipping git worktree directory: {}",
                        entry.path().display()
                    );
                    return false;
                }
                true
            });
        }

        builder.build().filter_map(|e| e.ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitscratch::testing::{DetachedGitDirRepo, TestRepo};

    /// Resolve a path before an assertion reads it. Git hands back a resolved
    /// path, and a fixture lives under a temporary directory that macOS reaches
    /// through a symbolic link: `/var` resolves to `/private/var`.
    fn canonical(path: &Path) -> PathBuf {
        fs::canonicalize(path).unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
    }

    /// Build a repository with one commit, so `git worktree list` has a HEAD to
    /// report and a later worktree has a commit to branch from.
    fn repo_with_one_commit() -> TestRepo {
        let repo = TestRepo::init();
        repo.commit_file("tracked.txt", "the file\n", "base");
        repo
    }

    #[test]
    fn a_nested_detached_git_directory_is_its_own_checkout() {
        let repo = DetachedGitDirRepo::nested();

        let context = find_repo_context_at(repo.git_dir()).expect("git knows this repository");

        assert_eq!(canonical(context.checkout()), canonical(repo.git_dir()));
        assert_eq!(
            canonical(context.main_worktree()),
            canonical(repo.git_dir())
        );
    }

    #[test]
    fn a_detached_git_directory_beside_its_work_tree_is_its_own_checkout() {
        let repo = DetachedGitDirRepo::beside();

        let context = find_repo_context_at(repo.git_dir()).expect("git knows this repository");

        assert_eq!(canonical(context.checkout()), canonical(repo.git_dir()));
        assert_eq!(
            canonical(context.main_worktree()),
            canonical(repo.git_dir())
        );
    }

    #[test]
    fn a_normal_repository_root_is_its_own_checkout() {
        let repo = repo_with_one_commit();

        let context = find_repo_context_at(repo.path()).expect("git knows this repository");

        assert_eq!(canonical(context.checkout()), canonical(repo.path()));
        assert_eq!(canonical(context.main_worktree()), canonical(repo.path()));
    }

    #[test]
    fn a_subdirectory_belongs_to_the_repository_above_it() {
        let repo = repo_with_one_commit();
        let deep = repo.path().join("sub").join("deeper");
        fs::create_dir_all(&deep).expect("create the subdirectory");

        let context = find_repo_context_at(&deep).expect("git knows this repository");

        assert_eq!(canonical(context.checkout()), canonical(repo.path()));
        assert_eq!(canonical(context.main_worktree()), canonical(repo.path()));
    }

    #[test]
    fn a_directory_that_is_no_repository_has_no_context() {
        let dir = tempfile::TempDir::new().expect("create temp dir");

        assert!(find_repo_context_at(dir.path()).is_none());
    }

    #[test]
    fn test_find_git_repo() {
        let path = find_git_repo().expect("Tests should run within a git repository");
        assert!(path.join(".git").exists());
    }

    #[test]
    fn test_find_main_repo() {
        let path = find_main_repo().expect("Tests should run within a git repository");
        assert!(
            path.join(".git").is_dir(),
            "Main repo should have .git directory, not file"
        );
    }
}
