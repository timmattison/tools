//! create — `swt create <name>`: a worktree that only exists if HEAD was green.
//!
//! The order of operations is the whole design, and it is not the obvious one.
//! The worktree is built *first* and the check runs *inside* it, because a fresh
//! worktree is a clean checkout of HEAD: uncommitted work in the parent — the
//! half-finished edit that is the normal state of the tree a subagent is being
//! branched from — cannot reach it, and so cannot fake a green. Checking the
//! parent instead would verify a commit nobody is about to branch from.
//!
//! Building first is what creates the window this module then has to close: for
//! as long as the check runs, a worktree and a branch exist that nobody has
//! agreed to keep. [`crate::teardown`] owns that window; `create` hands the
//! worktree over the moment `git worktree add` returns and takes it back only
//! once the check has passed.
//!
//! What survives a failed check is reported, never assumed. Teardown is
//! best-effort — git refuses to remove a working tree whose `.git` link has gone
//! missing — and claiming a cleanup that did not happen would strand the user
//! with an orphaned worktree *and* branch they were told did not exist.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::git::{validate_worktree_name, WorktreeName, WORKTREE_NAME_RULE};

/// Suffix every subagent worktree directory carries, so a stray directory beside
/// a repository is recognizable as `swt`'s at a glance.
const WORKTREE_SUFFIX: &str = ".swt";

/// Namespace every branch `swt` creates lives under.
const BRANCH_PREFIX: &str = "swt";

/// Radix the branch's timestamp suffix is spelled in — the Rust spelling of the
/// original's `Date.now().toString(36)`. Base 36 is the largest radix `char`
/// digits cover, and keeps a millisecond timestamp to eight compact characters
/// that are all legal in both a branch name and a path.
const BRANCH_SUFFIX_RADIX: u32 = 36;

/// Spells a number in lowercase base 36.
///
/// `value` is the number to spell. Returns its digits, most significant first;
/// zero is `"0"` rather than the empty string.
fn base36(value: u128) -> String {
    let _ = value;
    String::new()
}

/// Names the directory a worktree called `name` belongs in: a sibling of the
/// repository root, so worktrees sit beside the repo rather than inside it,
/// where git would have to be told to ignore them.
///
/// `root` is the repository root and `name` the validated worktree name.
fn worktree_path(root: &Path, name: &WorktreeName) -> PathBuf {
    let _ = (root, name);
    PathBuf::new()
}

/// Names the branch a fresh worktree is created on.
///
/// The timestamp suffix is what keeps two worktrees of the same name from
/// naming the same branch. `name` is the validated worktree name.
fn branch_name(name: &WorktreeName) -> String {
    let _ = name;
    String::new()
}

/// Creates a subagent worktree named `raw_name`, branched from a green HEAD.
///
/// The name is checked before any git runs, because it becomes both a branch and
/// a directory: `..` or a leading `-` would otherwise have git create the wrong
/// thing somewhere else entirely, and the cheapest place to stop that is before
/// anything exists to clean up. A rejected name is reported on stderr together
/// with the rule it broke, and the command fails.
///
/// On success the worktree's path — and nothing else — goes to stdout, so a
/// caller can capture it cleanly. On a red check the worktree and its branch are
/// torn down again, what that teardown actually did is reported on stderr, and
/// the command fails.
pub fn create(raw_name: &str) -> ExitCode {
    let Some(name): Option<WorktreeName> = validate_worktree_name(raw_name) else {
        eprintln!("Invalid worktree name {raw_name:?} — {WORKTREE_NAME_RULE}.");
        return ExitCode::FAILURE;
    };

    let _ = name;
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{base36, branch_name, worktree_path};
    use crate::git::validate_worktree_name;
    use std::path::{Path, PathBuf};

    /// A validated name for the helpers under test.
    fn name(raw: &str) -> crate::git::WorktreeName {
        validate_worktree_name(raw).expect("fixture name should validate")
    }

    // The branch suffix has to be spelled exactly the way the TypeScript
    // original spelled it, digit for digit: `Date.now().toString(36)`. These are
    // that function's answers.
    #[test]
    fn base36_spells_the_same_digits_as_the_original() {
        let cases: [(u128, &str); 8] = [
            (0, "0"),
            (1, "1"),
            (35, "z"),
            (36, "10"),
            (37, "11"),
            (1295, "zz"),
            (1296, "100"),
            (1_706_651_234_567, "ls0w2whz"),
        ];
        for (value, expected) in cases {
            assert_eq!(base36(value), expected, "base36({value})");
        }
    }

    #[test]
    fn the_worktree_is_a_sibling_of_the_repository_root() {
        assert_eq!(
            worktree_path(Path::new("/repos/tools"), &name("fix-parser")),
            PathBuf::from("/repos/fix-parser.swt"),
            "the worktree belongs beside the repository, not inside it"
        );
    }

    #[test]
    fn a_root_with_no_parent_still_names_a_worktree() {
        assert_eq!(
            worktree_path(Path::new("/"), &name("x")),
            PathBuf::from("/x.swt"),
            "a root with nothing above it resolves '..' to itself, as path resolution does"
        );
    }

    // The branch is namespaced under `swt/` so a repository's own branches are
    // never confused for a subagent's, and suffixed so two runs cannot name the
    // same branch. Every character in the suffix has to stay inside the rule the
    // name itself was validated against.
    #[test]
    fn a_branch_is_the_name_under_the_swt_namespace_with_a_timestamp_suffix() {
        let branch = branch_name(&name("fix-parser"));
        let suffix = branch
            .strip_prefix("swt/fix-parser-")
            .unwrap_or_else(|| panic!("branch should be namespaced and suffixed, got {branch:?}"));
        assert!(
            !suffix.is_empty(),
            "the suffix is what stops two runs naming one branch: {branch}"
        );
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "a branch suffix must stay inside the worktree name rule: {branch}"
        );
    }
}
