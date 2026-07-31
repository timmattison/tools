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

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::git::{git_must, validate_worktree_name, WorktreeName, WORKTREE_NAME_RULE};
use crate::green_check::{is_green, shell_quote};
use crate::teardown::{hold_unverified_worktree, remove_unverified_worktree};

/// The git query that names the root of the worktree `swt` was invoked in.
const TOPLEVEL_ARGS: [&str; 2] = ["rev-parse", "--show-toplevel"];

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
fn base36(mut value: u128) -> String {
    let radix = u128::from(BRANCH_SUFFIX_RADIX);
    let mut digits = String::new();
    // Division peels the digits off least-significant first, so they are pushed
    // in reverse and the string is flipped once at the end — cheaper and clearer
    // than repeatedly inserting at the front.
    loop {
        let remainder = value % radix;
        value /= radix;
        // Both conversions are total: a remainder below the radix always fits in
        // a `u32`, and `char::from_digit` covers every digit below 36. The
        // fallback exists only so an impossible case cannot panic.
        digits.push(
            u32::try_from(remainder)
                .ok()
                .and_then(|digit| char::from_digit(digit, BRANCH_SUFFIX_RADIX))
                .unwrap_or('0'),
        );
        if value == 0 {
            break;
        }
    }
    digits.chars().rev().collect()
}

/// Milliseconds since the UNIX epoch, the quantity the branch suffix spells.
///
/// A clock somehow set before the epoch yields `0` rather than killing the
/// command: the suffix exists to distinguish two runs, and even a useless one is
/// a better outcome than refusing to create a worktree over a clock reading.
fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_millis())
}

/// Names the directory a worktree called `name` belongs in: a sibling of the
/// repository root, so worktrees sit beside the repo rather than inside it,
/// where git would have to be told to ignore them.
///
/// `root` is the repository root and `name` the validated worktree name.
fn worktree_path(root: &Path, name: &WorktreeName) -> PathBuf {
    // The lexical parent, which is what resolving `<root>/../<name>.swt` comes
    // to: git answers `--show-toplevel` with an absolute, already-normalized
    // path, so there is no `..` component left for a lexical step to get wrong.
    // A root with nothing above it stands in for itself, exactly as path
    // resolution treats `/..`.
    root.parent()
        .unwrap_or(root)
        .join(format!("{name}{WORKTREE_SUFFIX}"))
}

/// Names the branch a fresh worktree is created on.
///
/// The timestamp suffix is what keeps two worktrees of the same name from
/// naming the same branch. `name` is the validated worktree name.
fn branch_name(name: &WorktreeName) -> String {
    format!("{BRANCH_PREFIX}/{name}-{}", base36(now_millis()))
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

    let root = PathBuf::from(git_must(TOPLEVEL_ARGS, None));
    let branch = branch_name(&name);
    let path = worktree_path(&root, &name);

    // The worktree comes first and the check runs inside it. `git worktree add`
    // populates it from HEAD, so what gets verified is the commit a subagent
    // would actually branch from rather than the parent's working tree.
    git_must(
        [
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("-b"),
            OsStr::new(&branch),
            path.as_os_str(),
            OsStr::new("HEAD"),
        ],
        Some(&root),
    );
    // Nothing has verified this worktree yet, and the check about to run can take
    // minutes. Until it passes, `swt` owns removing it — including when the user
    // gives up and hits Ctrl-C, and including when the check panics: the hold is
    // a guard, so an unwind past this point takes the worktree with it.
    let hold = hold_unverified_worktree(&root, &path, &branch);

    // Checked in the new worktree, configured from the parent: the `.swt-check`
    // override is an uncommitted per-developer file, so it exists only in `root`,
    // while the tree worth verifying is the fresh one. Swapping these two
    // directories is the difference between verifying HEAD and verifying
    // whatever the user happens to have half-written.
    let green = is_green(&path, Some(&root));
    if !green.ok {
        // The verdict before the cleanup: why the worktree is going away matters
        // more than the fact that it did. The check's output already ends in a
        // newline of its own.
        eprint!("HEAD not green: {}", green.out);
        // Asked for explicitly rather than left to `hold`'s destructor, because
        // this is the one path with something to *say* about the teardown. The
        // drop that follows finds nothing left to do — the removal is latched.
        report_teardown(&path, &branch);
        return ExitCode::FAILURE;
    }

    // Verified: it is the caller's worktree now, not `swt`'s to tear down.
    hold.keep();

    // Only the path, and only on stdout — a caller captures this.
    println!("{}", path.display());
    ExitCode::SUCCESS
}

/// Tears the unverified worktree down and says what actually happened to it.
///
/// `path` is the worktree directory and `branch` the branch checked out in it,
/// both named in the message so a failed teardown leaves a copy-pasteable
/// recovery command behind.
fn report_teardown(path: &Path, branch: &str) {
    // A teardown nobody had left to do is reported as a cleanup, because the
    // state it claims is the state that holds: somebody else already got there.
    let failure = remove_unverified_worktree().filter(|torn| !torn.ok);
    let Some(failed) = failure else {
        eprintln!(
            "Cleaned up worktree {} and branch {branch}.",
            path.display()
        );
        return;
    };

    // Teardown is best-effort, so claiming it worked would strand the user with
    // an orphaned worktree *and* branch they were told did not exist. Report
    // git's own account, then the command that finishes the job by hand.
    eprint!("{}", failed.out);
    eprintln!(
        "Could not clean up {}. Remove it by hand:\n  git worktree remove --force {} && git branch -D {branch}",
        path.display(),
        shell_quote(&path.to_string_lossy())
    );
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
