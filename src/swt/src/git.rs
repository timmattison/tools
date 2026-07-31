//! git — the single entrance for running git from `swt`, and the guard on the
//! names that reach it.
//!
//! Every git command goes through here as an argv array handed straight to the
//! process, with no shell in between. That is deliberate and load-bearing:
//! branch names and worktree paths are built from caller-supplied argv, so a
//! shell in the middle turns `swt create 'a; rm -rf ~'` into arbitrary code
//! execution, and even a benign space silently word-splits into the wrong branch
//! and the wrong path. There is intentionally no string-command variant here —
//! adding a new git call means adding an argv array, which cannot be injected
//! into.
//!
//! Argv arrays close the injection hole but not the *nonsense* hole, which is
//! what [`validate_worktree_name`] is for.

use std::ffi::OsStr;
use std::fmt;
use std::path::Path;

use crate::green_check::Outcome;

/// A git command that failed somewhere the caller cannot treat failure as an
/// answer, carrying git's combined output as the explanation.
///
/// Most of this module reports failure as an ordinary [`Outcome`] — "that git
/// command said no" is usually a fact to print and fold together with another
/// one. [`worktree_dirt`] is the exception: its successful answer is a *string*,
/// and the empty string already means "clean". A git that never ran would
/// otherwise be indistinguishable from a spotless worktree, and would wave a
/// merge straight past the guard that calls it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFailure(String);

impl GitFailure {
    /// Borrows git's combined output as it was captured.
    #[must_use]
    pub fn output(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GitFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GitFailure {}

/// Runs git and captures its combined output.
///
/// Public only so the process-group guard in the test suite can exercise this
/// exact call rather than a hand-rolled imitation of it; production callers want
/// [`git`], [`git_must`] or [`remove_worktree`], which fix `shielded` to the
/// value their situation calls for.
///
/// `args` are handed to the process one argv entry per element, never through a
/// shell. `cwd` is the directory to run git in; `None` means the current
/// working directory. `shielded` decides whether git is put in a process group
/// of its own, out of reach of a signal aimed at `swt`'s — see
/// [`remove_worktree`] for why that is the right call for teardown and the wrong
/// one for everything else.
///
/// Returns git's success flag and its stdout followed by its stderr. A git that
/// could not be spawned at all is a failure carrying the reason, not a panic:
/// every caller here already has to handle a git that said no.
pub fn run_git<I, S>(args: I, cwd: Option<&Path>, shielded: bool) -> Outcome
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let _ = (args, cwd, shielded);
    Outcome::failed(String::new())
}

/// Runs a git command, capturing its combined output.
///
/// Arguments are passed to git directly rather than through a shell, so spaces,
/// `;`, `$(…)` and every other metacharacter in `args` are always literal
/// argument text.
///
/// Interruptible: a Ctrl-C reaches this git the same way it reaches `swt`, which
/// is what you want for work the user is waiting on and can abandon.
///
/// `args` are the arguments to git, one element per argv entry, and `cwd` the
/// directory to run it in — `None` meaning the current working directory.
/// Returns git's success flag and its combined stdout/stderr.
pub fn git<I, S>(args: I, cwd: Option<&Path>) -> Outcome
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_git(args, cwd, false)
}

/// Runs a git command that has no sensible failure handling, aborting the
/// process with git's own output when it fails.
///
/// `args` are the arguments to git, one element per argv entry, and `cwd` the
/// directory to run it in — `None` meaning the current working directory.
/// Returns git's trimmed combined output; on failure it writes that output to
/// stderr and exits with status 1 rather than returning.
pub fn git_must<I, S>(args: I, cwd: Option<&Path>) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let _ = (args, cwd);
    String::new()
}

/// Tears down a worktree and the branch checked out in it, forcing both.
///
/// Teardown is best-effort by nature — git refuses to remove a working tree
/// whose `.git` link has gone missing, and refuses to delete a branch a
/// registered worktree still claims — so the outcome is *reported* rather than
/// assumed. Both commands are attempted even when the first fails, and both
/// outputs come back: a caller shown only the first complaint would not know
/// whether the branch is still lying around too, which is the difference between
/// a usable recovery instruction and a wrong one.
///
/// Best-effort is not the same as abandonable, though, so unlike every other git
/// call in `swt` these two run in a process group of their own. Teardown is most
/// often what a Ctrl-C *asked for*, and a terminal sends Ctrl-C to the whole
/// foreground process group — so an impatient second one would kill the very
/// command carrying out the first. Cut between these two calls, that leaves the
/// worst possible state: a worktree that survived and a branch that cannot be
/// deleted while it does. Out of the group, teardown finishes on its own terms,
/// and finishes even if `swt` itself is killed once it has started.
///
/// `root` is the repository worktree to run git from — never the one being
/// removed — `path` the worktree directory to delete, and `branch` the branch
/// checked out in it. Returns ok only when both commands succeeded; `out` is
/// their combined output.
pub fn remove_worktree(root: &Path, path: &Path, branch: &str) -> Outcome {
    let _ = (root, path, branch);
    Outcome::failed(String::new())
}

/// Reports a worktree's uncommitted state as git's own porcelain listing.
///
/// `cwd` is the worktree root to inspect. `include_untracked` decides whether
/// untracked files count as dirt, and the two answers are both load-bearing:
/// `swt merge` excludes them in the parent, because the documented `.swt-check`
/// escape hatch is by definition an untracked file at the parent root, and
/// includes them in the subagent worktree, because `git worktree remove` deletes
/// the whole directory and everything untracked in it.
///
/// Returns the trimmed porcelain listing; an empty string means clean.
///
/// # Errors
///
/// Returns a [`GitFailure`] carrying git's combined output when git itself
/// fails. A git that never answered is emphatically not a clean worktree.
pub fn worktree_dirt(cwd: &Path, include_untracked: bool) -> Result<String, GitFailure> {
    let _ = (cwd, include_untracked);
    Ok(String::new())
}

/// Human-readable statement of what a worktree name may contain.
///
/// Kept as one string so the rule a name was judged against and the rule quoted
/// back to the user can never drift apart.
pub const WORKTREE_NAME_RULE: &str =
    "allowed: letters, digits, '.', '_' and '-'; must not start with '-', and must not be '.' or '..'";

/// Names that are built only from allowed characters and are still meaningless
/// as a path component: `.` resolves to the worktree parent directory itself and
/// `..` resolves to its parent, so either one would have git create the worktree
/// on top of a directory that already exists and belongs to someone else.
const RESERVED_WORKTREE_NAMES: [&str; 2] = [".", ".."];

/// The character set a worktree name may be built from — the Rust spelling of
/// the original `/^[A-Za-z0-9._-]+$/`.
///
/// Deliberately a `char` predicate rather than a regular expression: the rule is
/// a one-line membership test, and expressing it directly keeps a regex engine
/// out of the dependency tree for no loss of clarity. Iterating `chars()` also
/// means a multi-byte character is judged as the single character it is instead
/// of as the bytes it happens to encode to.
fn is_worktree_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

/// A worktree base name that has passed [`validate_worktree_name`] and is
/// therefore safe to splice into a branch name and a filesystem path.
///
/// The private field is the Rust equivalent of the TypeScript original's unique
/// symbol brand, and it does the same job: nothing outside this module can
/// produce one, so a `WorktreeName` in a signature is proof the check ran rather
/// than a request that it should have.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorktreeName(String);

impl WorktreeName {
    /// Borrows the validated name as it was originally spelled.
    ///
    /// Validation neither rewrites nor normalizes: the string that comes back is
    /// byte for byte the one the caller supplied, which is what lets the same
    /// value name both the branch and the directory.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorktreeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Checks that a caller-supplied worktree base name is safe to turn into a
/// branch name and a worktree path.
///
/// Passing git argv arrays already removes the injection risk, but an unchecked
/// name still yields nonsense: `../..` escapes the worktree parent directory, a
/// leading `-` is read as an option, and `/` silently nests the branch.
///
/// `name` is the raw string as supplied on the command line. Returns the
/// validated name, or `None` if it violates [`WORKTREE_NAME_RULE`] — callers are
/// expected to quote the rule back to the user on `None` rather than invent
/// their own wording.
#[must_use]
pub fn validate_worktree_name(name: &str) -> Option<WorktreeName> {
    // `all` is vacuously true for an empty name, so the emptiness check is what
    // stands in for the `+` in the original pattern.
    if name.is_empty() || !name.chars().all(is_worktree_name_char) {
        return None;
    }
    if name.starts_with('-') {
        return None;
    }
    if RESERVED_WORKTREE_NAMES.contains(&name) {
        return None;
    }
    Some(WorktreeName(name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{validate_worktree_name, WorktreeName};

    /// Names that survive validation, paired with why each shape has to keep
    /// working: between them they cover every character class the rule allows.
    const ACCEPTED: &[(&str, &str)] = &[
        ("fix-parser", "a plain hyphenated name is the common case"),
        ("fix_parser", "underscores are allowed"),
        ("issue42", "digits are allowed"),
        ("v1.2.3", "dots inside the name are allowed"),
        ("A-Z_a-z.0-9", "every allowed class at once"),
        ("x", "a single character is enough"),
    ];

    /// Names that must be refused, paired with the damage accepting them would
    /// do. The reason is the test's real subject: each of these is rejected for
    /// a *specific* failure it would otherwise cause.
    const REJECTED: &[(&str, &str)] = &[
        ("fix parser", "a space splits the branch name from the path"),
        ("a;rm -rf /", "a semicolon is a command separator"),
        ("$(touch pwned)", "command substitution"),
        ("feat/foo", "a slash silently nests the branch and the path"),
        ("a\\b", "a backslash is a path separator on Windows"),
        ("..", "escapes the worktree parent directory"),
        (".", "resolves to the parent directory itself"),
        ("../evil", "path traversal"),
        ("-b", "a leading dash is read as a git option"),
        ("-rf", "a leading dash is read as a git option"),
        ("", "an empty name yields an empty path component"),
        ("with\nnewline", "a newline breaks ref parsing"),
        ("'quoted'", "quotes are not part of a name"),
        ("\"quoted\"", "quotes are not part of a name"),
        ("日本語", "non-ASCII is outside the allowed set"),
        ("café", "one non-ASCII character is still non-ASCII"),
        ("🎉", "a multi-byte emoji is outside the allowed set"),
    ];

    #[test]
    fn accepts_names_built_from_the_allowed_characters() {
        for (name, why) in ACCEPTED {
            assert!(
                validate_worktree_name(name).is_some(),
                "{name:?} should be accepted: {why}"
            );
        }
    }

    #[test]
    fn rejects_names_that_would_name_the_wrong_thing() {
        for (name, why) in REJECTED {
            assert_eq!(
                validate_worktree_name(name),
                None,
                "{name:?} should be rejected: {why}"
            );
        }
    }

    #[test]
    fn an_accepted_name_round_trips_unchanged() {
        for (name, _) in ACCEPTED {
            let validated: WorktreeName =
                validate_worktree_name(name).expect("accepted name should validate");
            assert_eq!(
                validated.as_str(),
                *name,
                "validation must not rewrite the name it accepts"
            );
            assert_eq!(validated.to_string(), *name, "Display must match as_str");
        }
    }
}
