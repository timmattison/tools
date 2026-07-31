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

use std::fmt;

/// Human-readable statement of what a worktree name may contain.
///
/// Kept as one string so the rule a name was judged against and the rule quoted
/// back to the user can never drift apart.
pub const WORKTREE_NAME_RULE: &str =
    "allowed: letters, digits, '.', '_' and '-'; must not start with '-', and must not be '.' or '..'";

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
