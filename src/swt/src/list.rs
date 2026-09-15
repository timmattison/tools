//! list — `swt list`: the children of the branch that is checked out here.
//!
//! A child is a worktree that `swt create` made. Its branch is
//! `swt/<parent branch>/<name>-<token>`, so the branch of a child names its
//! parent. `list` shows each child of the branch that is checked out in the
//! worktree where it runs:
//!
//! ```text
//! <path>\t<branch>
//! <path>\t<branch>\tprunable
//! ```
//!
//! Each child gets one line, and the lines are sorted by path. The path is the
//! one that `swt create` printed, so it can go directly to `swt merge`. The
//! branch is the local branch without `refs/heads/`. Git marks a child
//! `prunable` when its directory is gone, for example after a person deletes it
//! by hand. Its line then gets the third field `prunable`, and the user can
//! remove the entry with `git worktree prune`.
//!
//! When the branch has no children, stdout stays empty and a one-line note that
//! names the branch goes to stderr. The status is still 0, because no children
//! is an answer and not a failure. `[ -n "$(swt list)" ]` is thus a complete
//! check.
//!
//! A detached HEAD has no branch, so it has no children to ask about. `list`
//! then prints nothing on stdout, says why on stderr, and exits 1. It reads
//! HEAD through [`head_branch`]. `create` uses the same read, and refuses a
//! detached HEAD for the same reason.
//!
//! The source is the worktree registry of git, `git worktree list --porcelain
//! -z`. `list` does not scan directories. A directory beside the parent is not
//! a worktree until git says so, and the directory name is for people. In the
//! `-z` form each field ends with NUL, so a path with a newline stays in one
//! field.
//!
//! A child is an entry whose branch names the current branch as its parent.
//! The parent is the text between `swt/` and the last `/` of the branch, and it
//! must be equal to the current branch. A prefix is not sufficient: the
//! children of `feat/foo` are not children of `feat`. A branch in the older
//! format `swt/<name>-<token>` names no parent, so `list` never shows it.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::create::parent_branch_of;
use crate::git::{git, head_branch, local_branch, BranchName, HeadBranch};

/// The query that reads the worktree registry of git. `--porcelain` gives the
/// format that git keeps stable for tools. `-z` ends each field with NUL
/// instead of a newline.
const WORKTREE_LIST_ARGS: [&str; 4] = ["worktree", "list", "--porcelain", "-z"];

/// The character that ends each field of the registry. An empty field ends a
/// record.
const FIELD_TERMINATOR: char = '\0';

/// Separates the label of a field from its value. A field with no value, such
/// as `detached`, holds only the label.
const LABEL_SEPARATOR: char = ' ';

/// The label of the field that starts a record. Its value is the path of the
/// worktree.
const WORKTREE_LABEL: &str = "worktree";

/// The label of the field that holds the full ref of the branch that the
/// worktree has checked out.
const BRANCH_LABEL: &str = "branch";

/// What `list` writes to stderr, before the name of the branch, when the branch
/// has no children. Stdout stays empty, so a caller that captures it reads no
/// children. A person reads this note and knows why. The wording lives here and
/// nowhere else.
const NO_CHILDREN_NOTE: &str = "No child worktrees of the branch";

/// What `list` writes to stderr when HEAD is detached. It states the fact, why
/// the fact stops the command, and what the user must do. The wording lives
/// here and nowhere else.
const DETACHED_HEAD_REFUSAL: &str =
    "HEAD is detached. A detached HEAD has no branch, so it has no children \
     to list. Check out a branch, then run swt list again.";

/// The label that git gives a worktree whose directory is gone, with or without
/// a reason after it. `list` prints the same word as the third field of the
/// line of such a child, so the user reads the word that git uses.
const PRUNABLE: &str = "prunable";

/// Separates the fields of a line that `list` prints. A branch name cannot hold
/// a tab.
const OUTPUT_FIELD_SEPARATOR: char = '\t';

/// One entry of the worktree registry, with the fields that `list` reads.
///
/// Git writes more fields than these: `HEAD`, `detached`, `bare` and `locked`.
/// `list` ignores them. A child is known by its branch alone, and `prunable`
/// only adds a field to its line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RegisteredWorktree {
    /// The directory of the worktree, as git recorded it.
    path: PathBuf,
    /// The local branch that the worktree has checked out. `None` for a
    /// detached HEAD, for a bare entry, and for a ref outside `refs/heads/`.
    branch: Option<BranchName>,
    /// Whether git marks the worktree prunable, because its directory is gone.
    prunable: bool,
}

impl RegisteredWorktree {
    /// An entry for the worktree at `path`, before any other field is read.
    fn at(path: &str) -> Self {
        Self {
            path: PathBuf::from(path),
            branch: None,
            prunable: false,
        }
    }

    /// Records one field of this entry. `label` names the field and `value` is
    /// the text after the label, empty when the field has none. A label that
    /// `list` does not read changes nothing. A `prunable` field marks the entry
    /// whatever its reason says.
    fn read(&mut self, label: &str, value: &str) {
        match label {
            BRANCH_LABEL => self.branch = local_branch(value),
            PRUNABLE => self.prunable = true,
            _ => {}
        }
    }
}

/// Reads the entries out of the output of `git worktree list --porcelain -z`.
///
/// The pure half of the registry read. It takes the text as an argument, so
/// the unit tests pin each form of entry without a repository.
///
/// `listing` is the output that [`git`] captured. That is stdout, followed by
/// stderr. A field that comes before the first `worktree` field, or after the
/// empty field that ends the last record, belongs to no entry. `list` ignores
/// it, so a warning on stderr cannot become an entry.
///
/// Returns the entries in the order of the registry.
fn parse_registry(listing: &str) -> Vec<RegisteredWorktree> {
    let mut entries = Vec::new();
    // The entry whose fields are being read, if one is open.
    let mut open: Option<RegisteredWorktree> = None;
    for field in listing.split(FIELD_TERMINATOR) {
        let (label, value) = field.split_once(LABEL_SEPARATOR).unwrap_or((field, ""));
        if label == WORKTREE_LABEL {
            // A new record. It also closes an open record whose empty field is
            // missing, so no record is lost.
            entries.extend(open.replace(RegisteredWorktree::at(value)));
        } else if field.is_empty() {
            // The empty field that ends a record.
            entries.extend(open.take());
        } else if let Some(entry) = open.as_mut() {
            entry.read(label, value);
        }
    }
    entries.extend(open);
    entries
}

/// A child of the current branch: a worktree that `swt create` made there.
///
/// A child always has a branch, because its branch is what makes it a child.
struct Child {
    /// The directory of the child, as git recorded it.
    path: PathBuf,
    /// The branch that the child has checked out.
    branch: BranchName,
    /// Whether git marks the child prunable, because its directory is gone.
    prunable: bool,
}

impl Child {
    /// The line that `list` prints for this child: the path, a tab, and the
    /// branch. A prunable child gets a tab and [`PRUNABLE`] after the branch.
    /// A newline ends the line.
    fn line(&self) -> String {
        let mut line = format!(
            "{}{OUTPUT_FIELD_SEPARATOR}{}",
            self.path.display(),
            self.branch
        );
        if self.prunable {
            line.push(OUTPUT_FIELD_SEPARATOR);
            line.push_str(PRUNABLE);
        }
        line.push('\n');
        line
    }
}

/// Keeps the entries of `registry` that are children of `parent`, and sorts
/// them by path.
///
/// An entry is a child when the parent in its branch, as [`parent_branch_of`]
/// reads it, is equal to `parent`. The order of the registry is the order in
/// which git lists its worktrees, and a caller cannot control it. A sort by
/// path gives the same output for the same children.
fn children_of(registry: Vec<RegisteredWorktree>, parent: &BranchName) -> Vec<Child> {
    let mut children: Vec<Child> = registry
        .into_iter()
        .filter_map(|entry| {
            let branch = entry.branch?;
            // Equality, not a prefix: the parent in `swt/feat/foo/x-1` is
            // `feat/foo`, and that is not `feat`.
            (parent_branch_of(branch.as_str()) == Some(parent.as_str())).then_some(Child {
                path: entry.path,
                branch,
                prunable: entry.prunable,
            })
        })
        .collect();
    children.sort_by(|left, right| left.path.cmp(&right.path));
    children
}

/// Prints the children of the branch that is checked out in the current
/// worktree, one line each, sorted by path.
///
/// The current branch comes from [`head_branch`], the same read that `create`
/// uses to name the parent of a child. The children come from the worktree
/// registry of git. When git fails in either read, its own output goes to
/// stderr and the command fails. A detached HEAD has no branch, so the command
/// writes [`DETACHED_HEAD_REFUSAL`] to stderr, prints nothing on stdout, and
/// fails.
///
/// When the branch has no children, nothing goes to stdout.
/// [`NO_CHILDREN_NOTE`] and the branch go to stderr, and the command succeeds.
///
/// Returns the status that `swt` exits with.
pub fn list() -> ExitCode {
    let current = match head_branch(None) {
        Ok(HeadBranch::Branch(branch)) => branch,
        // A detached HEAD has no branch, so it has no children to show. Stdout
        // stays empty, so a caller cannot take the refusal for a child.
        Ok(HeadBranch::Detached) => {
            eprintln!("{DETACHED_HEAD_REFUSAL}");
            return ExitCode::FAILURE;
        }
        // The account of git itself, which already ends in a newline.
        Err(failure) => {
            eprint!("{failure}");
            return ExitCode::FAILURE;
        }
    };

    let registry = git(WORKTREE_LIST_ARGS, None);
    if !registry.ok {
        eprint!("{}", registry.out);
        return ExitCode::FAILURE;
    }

    let children = children_of(parse_registry(&registry.out), &current);
    if children.is_empty() {
        // No children is an answer, not a failure. Stdout stays empty for a
        // caller, and a person reads why on stderr.
        eprintln!("{NO_CHILDREN_NOTE} {current}.");
        return ExitCode::SUCCESS;
    }

    let listing: String = children.iter().map(Child::line).collect();
    // One write for the whole listing, so a reader that stops after the first
    // line does not cut a later write in half.
    print!("{listing}");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{parse_registry, RegisteredWorktree};
    use crate::git::local_branch;
    use std::path::PathBuf;

    /// The entry that a test expects for the worktree at `path`, on the local
    /// branch `branch` or on no branch. Git does not mark it prunable.
    fn entry(path: &str, branch: Option<&str>) -> RegisteredWorktree {
        RegisteredWorktree {
            path: PathBuf::from(path),
            branch: branch.map(|name| {
                local_branch(&format!("refs/heads/{name}"))
                    .unwrap_or_else(|| panic!("fixture branch {name:?} must be a local branch"))
            }),
            prunable: false,
        }
    }

    /// The entry that a test expects for a worktree that git marks prunable.
    fn prunable_entry(path: &str, branch: Option<&str>) -> RegisteredWorktree {
        RegisteredWorktree {
            prunable: true,
            ..entry(path, branch)
        }
    }

    // The common case, in the form that git 2.55 writes: a main checkout and
    // one child, each on a branch.
    #[test]
    fn a_record_holds_its_path_and_its_branch() {
        let listing = "worktree /repos/tools\0HEAD 0123abcd\0branch refs/heads/main\0\0\
                       worktree /repos/tools--fix-abc.swt\0HEAD 0123abcd\0\
                       branch refs/heads/swt/main/fix-abc\0\0";
        assert_eq!(
            parse_registry(listing),
            vec![
                entry("/repos/tools", Some("main")),
                entry("/repos/tools--fix-abc.swt", Some("swt/main/fix-abc")),
            ]
        );
    }

    // A worktree on a detached HEAD has a `detached` field and no `branch`
    // field. It must still be an entry, so it cannot take the fields of the
    // next record.
    #[test]
    fn a_detached_record_has_no_branch() {
        let listing = "worktree /repos/detached\0HEAD 0123abcd\0detached\0\0\
                       worktree /repos/on-main\0HEAD 0123abcd\0branch refs/heads/main\0\0";
        assert_eq!(
            parse_registry(listing),
            vec![
                entry("/repos/detached", None),
                entry("/repos/on-main", Some("main")),
            ]
        );
    }

    // A bare repository lists its main entry with a `bare` field and nothing
    // else. It has no branch, so it can never be a child.
    #[test]
    fn a_bare_main_entry_has_no_branch() {
        let listing = "worktree /repos/tools.git\0bare\0\0\
                       worktree /repos/linked\0HEAD 0123abcd\0branch refs/heads/feat/foo\0\0";
        assert_eq!(
            parse_registry(listing),
            vec![
                entry("/repos/tools.git", None),
                entry("/repos/linked", Some("feat/foo")),
            ]
        );
    }

    // `locked` comes with a reason or without one. Neither form changes the
    // path, the branch or the mark. A reason is not a label, even when it
    // starts with the word of another label.
    #[test]
    fn a_locked_record_keeps_its_path_and_its_branch() {
        let listing = "worktree /repos/locked\0HEAD 0123abcd\0branch refs/heads/a\0locked\0\0\
                       worktree /repos/held\0HEAD 0123abcd\0branch refs/heads/b\0\
                       locked prunable branch refs/heads/decoy\0\0";
        assert_eq!(
            parse_registry(listing),
            vec![
                entry("/repos/locked", Some("a")),
                entry("/repos/held", Some("b")),
            ]
        );
    }

    // Git marks a worktree whose directory is gone `prunable`, with a reason or
    // without one. The registry still knows its branch, and the mark stays on
    // the entry that carries it.
    #[test]
    fn a_prunable_record_keeps_its_branch_and_carries_the_mark() {
        let listing = "worktree /repos/gone\0HEAD 0123abcd\0branch refs/heads/swt/main/gone-abc\0\
                       prunable gitdir file points to non-existent location\0\0\
                       worktree /repos/no-reason\0HEAD 0123abcd\0branch refs/heads/b\0prunable\0\0\
                       worktree /repos/kept\0HEAD 0123abcd\0branch refs/heads/c\0\0";
        assert_eq!(
            parse_registry(listing),
            vec![
                prunable_entry("/repos/gone", Some("swt/main/gone-abc")),
                prunable_entry("/repos/no-reason", Some("b")),
                entry("/repos/kept", Some("c")),
            ]
        );
    }

    // The reason to read the `-z` form. A newline is part of the path there,
    // so it cannot split one record into two. A space is part of the path as
    // well, because only the first space separates the label.
    #[test]
    fn a_path_with_a_newline_or_a_space_stays_in_one_record() {
        let listing = "worktree /repos/two\nlines\0HEAD 0123abcd\0branch refs/heads/main\0\0\
                       worktree /repos/with space\0HEAD 0123abcd\0branch refs/heads/b\0\0";
        assert_eq!(
            parse_registry(listing),
            vec![
                entry("/repos/two\nlines", Some("main")),
                entry("/repos/with space", Some("b")),
            ]
        );
    }

    // `git` captures stderr after stdout. A warning after the last record, or
    // text before the first one, belongs to no entry.
    #[test]
    fn text_outside_every_record_is_not_an_entry() {
        let listing = "noise before\0worktree /repos/tools\0HEAD 0123abcd\0\
                       branch refs/heads/main\0\0warning: branch refs/heads/decoy\n";
        assert_eq!(
            parse_registry(listing),
            vec![entry("/repos/tools", Some("main"))]
        );
    }

    // A record whose empty field is missing still counts, whether the next
    // record or the end of the text closes it.
    #[test]
    fn a_record_with_no_closing_field_is_still_an_entry() {
        let listing =
            "worktree /repos/a\0branch refs/heads/a\0worktree /repos/b\0branch refs/heads/b";
        assert_eq!(
            parse_registry(listing),
            vec![entry("/repos/a", Some("a")), entry("/repos/b", Some("b"))]
        );
    }

    #[test]
    fn an_empty_listing_has_no_entries() {
        assert_eq!(parse_registry(""), Vec::new());
    }
}
