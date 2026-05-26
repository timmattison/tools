//! Selects which git files Cargo must watch (via `cargo:rerun-if-changed`) so
//! the commit hash captured at build time stays in sync with the checked-out
//! commit.
//!
//! This logic lives in its own module so it can be unit tested and shared
//! between the build script (`build.rs`) and the library's test suite.

use std::path::{Path, PathBuf};

/// Returns the set of paths that, when changed, should trigger a rebuild so the
/// embedded git hash matches the current commit.
///
/// - `git_dir` is the resolved git directory (`.git` in a normal repository, or
///   the per-worktree git dir in a linked worktree).
/// - `git_common_dir` is the shared git directory where branch refs live; it
///   equals `git_dir` outside of worktrees.
/// - `head_contents` is the raw text of `<git_dir>/HEAD`.
///
/// `HEAD` and `index` are always watched. When `HEAD` is a symbolic ref
/// (e.g. `ref: refs/heads/main`), the referenced branch ref and `packed-refs`
/// are also watched — because those, not `HEAD` (whose contents stay
/// `ref: refs/heads/main`), are what change when the branch advances to a new
/// commit. Omitting them lets `cargo install` / `cargo install-update` reuse a
/// cached build-script result and bake a stale commit hash into the binary.
pub fn rerun_if_changed_paths(
    git_dir: &Path,
    git_common_dir: &Path,
    head_contents: &str,
) -> Vec<PathBuf> {
    let mut paths = vec![git_dir.join("HEAD"), git_dir.join("index")];

    // When HEAD is a symbolic ref, the commit it points at changes via the
    // branch ref (loose under refs/, or rolled into packed-refs), never via
    // HEAD itself. Watch both so a moved branch forces a rebuild. These live in
    // the shared common dir, which differs from `git_dir` inside a worktree.
    if let Some(reference) = head_contents.strip_prefix("ref:") {
        let reference = reference.trim();
        if !reference.is_empty() {
            paths.push(git_common_dir.join(reference));
            paths.push(git_common_dir.join("packed-refs"));
        }
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbolic_ref_head_watches_branch_ref_and_packed_refs() {
        let git_dir = Path::new("/repo/.git");
        let paths = rerun_if_changed_paths(git_dir, git_dir, "ref: refs/heads/main\n");

        assert!(paths.contains(&git_dir.join("HEAD")));
        assert!(paths.contains(&git_dir.join("index")));
        assert!(
            paths.contains(&git_dir.join("refs/heads/main")),
            "the branch ref must be watched so a moved branch triggers a rebuild; got {paths:?}"
        );
        assert!(
            paths.contains(&git_dir.join("packed-refs")),
            "packed-refs must be watched in case the branch ref is packed; got {paths:?}"
        );
    }

    #[test]
    fn detached_head_watches_only_head_and_index() {
        let git_dir = Path::new("/repo/.git");
        let paths = rerun_if_changed_paths(
            git_dir,
            git_dir,
            "0064899bc1ae2c411701ed7f35ce8f8d00d21d31\n",
        );

        // A detached HEAD holds the commit directly, so its own contents change
        // per commit and there is no branch ref to watch.
        assert_eq!(paths, vec![git_dir.join("HEAD"), git_dir.join("index")]);
    }

    #[test]
    fn worktree_resolves_branch_ref_against_common_dir() {
        let git_dir = Path::new("/repo/.git/worktrees/feature");
        let common_dir = Path::new("/repo/.git");
        let paths = rerun_if_changed_paths(git_dir, common_dir, "ref: refs/heads/feature\n");

        // HEAD and index are per-worktree...
        assert!(paths.contains(&git_dir.join("HEAD")));
        // ...but the branch ref lives in the shared common dir, not the
        // worktree git dir.
        assert!(paths.contains(&common_dir.join("refs/heads/feature")));
        assert!(!paths.contains(&git_dir.join("refs/heads/feature")));
    }
}
