//! The links from a new worktree to the child repositories of the main
//! worktree (issue #537).
//!
//! A container repository tracks the map of a workspace. The real repositories
//! sit one level below it, and its `.gitignore` keeps them out of its history.
//! `git worktree add` writes only tracked files, so a new worktree of the
//! container holds no child. [`link_children`] makes the symlink
//! `<worktree>/<name>` -> `<main worktree>/<name>` for each child.
//!
//! A symlink gives the worktree the one real checkout of each child. Thus no
//! clone, no branch, and no remote is duplicated, and a branch exists only in
//! a repository that the work changes.
//!
//! [`repowalker::child_repositories`] finds the children, so `nwt` and `cwt`
//! use one definition of a child: a directory one level below the main
//! worktree that holds a `.git` entry.
//!
//! First, this module reads the path `<worktree>/<name>`. When something is
//! already there, the module does not change it and asks git nothing. The new
//! branch can track that path, or a `post-checkout` hook can make it. The read
//! does not follow a symlink, so a broken symlink also counts. The read comes
//! before the question to git, because git does not ignore a directory that
//! holds a tracked file. Without the read, such a path gets a warning about
//! `.gitignore`, and `.gitignore` is correct.
//!
//! Before it makes a link, this module asks git in the **new** worktree
//! whether git ignores the path. The new worktree can hold a `.gitignore` that
//! differs from the main worktree, so the question goes to the new worktree. A
//! link that git does not ignore shows as untracked, and `git add -A` commits
//! it into the container. Git gives the same answer before the link exists, so
//! this module asks first and never makes a link that git does not ignore.
//!
//! The trap is a pattern with a trailing slash, such as `vial/`. It matches
//! only a directory, and git sees a symlink as a file, even when the symlink
//! points at a directory. So `vial/` does not ignore the link at `vial`, and
//! `git check-ignore -v -- vial` names no pattern, because no pattern matches.
//! Thus, when git does not ignore `<name>`, this module asks a second question
//! about `<name>/`, before a link exists. When a rule that ends with `/` matches
//! that form, the warning names the rule, its file, and its line, and it gives
//! the fix `/<name>`. Otherwise nothing ignores the child, and the warning gives
//! only the fix.
//!
//! `-q` removes the line for each link, the line for each path that is already
//! there, and the summary. It does not remove a warning, because a warning
//! names a defect in the repository.
//!
//! The target of each link is absolute: the main worktree that git names,
//! joined with the name of the child. Git keeps worktree paths absolute too, so
//! a relative link gives no more safety.
//!
//! The shell wrapper reads the worktree path from stdout, so each line of this
//! module goes to stderr, and the git child writes into a captured buffer.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Write};
use std::path::Path;
use std::process::Stdio;

use crate::production_git_command;

/// The exit status of `git check-ignore -q` for a path that git ignores.
const CHECK_IGNORE_IGNORED: i32 = 0;

/// The exit status of `git check-ignore -q` for a path that git does not
/// ignore.
const CHECK_IGNORE_NOT_IGNORED: i32 = 1;

/// The start of a pathspec that names an entry at the root of the worktree.
///
/// Git reads a leading `:` in a pathspec as magic. With this prefix, git reads
/// each name as a path.
const CURRENT_DIRECTORY_PREFIX: &str = "./";

/// The first word of each line that reports a link.
const LINKED_WORD: &str = "Linked";

/// The start of the warning for a child that has no link.
const NOT_LINKED_PREFIX: &str = "Warning: not linked:";

/// The start of the line for a child whose path the new worktree already
/// holds. The line is not a warning, because nothing is wrong.
const ALREADY_THERE_PREFIX: &str = "Not linked:";

/// Why a child whose path the new worktree already holds has no link.
const ALREADY_THERE_REASON: &str = "the new worktree already holds this path";

/// The byte that ends each field of `git check-ignore -z`, and each path of
/// its input.
const FIELD_END: u8 = 0;

/// The end of a path or a pattern that names only a directory.
const DIRECTORY_SUFFIX: &[u8] = b"/";

/// The start of a pattern that is a negation.
const NEGATION_PREFIX: &[u8] = b"!";

/// The start of a pattern that matches only at the root of the worktree. Such
/// a pattern without a trailing slash also matches a symlink.
const ROOT_ANCHOR: &str = "/";

/// The ignore file that the warning tells the user to change.
const IGNORE_FILE: &str = ".gitignore";

/// What happened to one child of the main worktree.
///
/// Each variant other than [`Outcome::Linked`] makes no link.
enum Outcome {
    /// The link exists now.
    Linked,
    /// Something was already at the path of the link, and it stays as it was.
    AlreadyThere,
    /// Git does not ignore the path in the new worktree, so a link there shows
    /// as untracked. The rule is there when a rule matches only the directory
    /// form of the path, which is the trap of a trailing slash.
    NotIgnored(Option<DirectoryOnlyRule>),
    /// Git gave no answer. It did not start, or it exited with a status that
    /// is not 0 or 1.
    NoAnswer,
    /// Git ignores the path, but the symlink could not be made, with this
    /// error.
    LinkFailed(io::Error),
}

/// Link each child repository of `main_worktree` into `worktree`.
///
/// `main_worktree` is the main worktree that git names, and `worktree` is the
/// new worktree. The function writes one line to stderr for each link, one line
/// for each child whose path the new worktree already holds, and a summary when
/// it made at least one link. `quiet` removes those lines, and the links still
/// exist. A main worktree without children gives no link and no line.
pub(crate) fn link_children(main_worktree: &Path, worktree: &Path, quiet: bool) {
    let mut linked = 0_usize;

    for child in repowalker::child_repositories(main_worktree) {
        let Some(name) = child.file_name() else {
            continue;
        };
        let target = main_worktree.join(name);

        let outcome = link_child(worktree, name, &target);
        if matches!(outcome, Outcome::Linked) {
            linked += 1;
        }
        report(name, &target, &outcome, quiet);
    }

    if linked > 0 && !quiet {
        eprintln!("{}", summary_line(linked));
    }
}

/// Make the symlink `<worktree>/<name>` -> `target` when nothing is at that
/// path and git ignores it.
///
/// The read of the path comes first, and it does not follow a symlink. A path
/// that is already there stays as it was, and git gets no question.
fn link_child(worktree: &Path, name: &OsStr, target: &Path) -> Outcome {
    let link = worktree.join(name);
    if link.symlink_metadata().is_ok() {
        return Outcome::AlreadyThere;
    }

    match check_ignore_status(worktree, name) {
        Some(CHECK_IGNORE_IGNORED) => {}
        Some(CHECK_IGNORE_NOT_IGNORED) => {
            return Outcome::NotIgnored(directory_rule_of(worktree, name));
        }
        _ => return Outcome::NoAnswer,
    }

    link_outcome(make_directory_link(target, &link))
}

/// Why git gave no answer to the question whether it ignores a path.
enum GitFailure {
    /// Git exited with a status that is not 0 or 1, or a signal stopped it.
    /// The text is the first line of its stderr.
    Failed(String),
    /// Git did not start, with this error.
    DidNotStart(io::Error),
}

impl fmt::Display for GitFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = (self, f);
        Ok(())
    }
}

/// The first line of `stderr` that holds text, without the space around it.
fn first_line_of(stderr: &[u8]) -> Option<String> {
    let _ = stderr;
    None
}

/// The outcome of the attempt to make a symlink, from its `result`.
///
/// A path can appear after the read of [`link_child`] and before the symlink.
/// The error of kind `AlreadyExists` then gives [`Outcome::AlreadyThere`], as
/// the read does. Each other error gives [`Outcome::LinkFailed`].
fn link_outcome(result: io::Result<()>) -> Outcome {
    match result {
        Ok(()) => Outcome::Linked,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Outcome::AlreadyThere,
        Err(error) => Outcome::LinkFailed(error),
    }
}

/// The exit status of `git check-ignore -q -- ./<name>` in `worktree`.
///
/// Returns `None` when git does not start, or when a signal stops it.
///
/// The command goes through [`production_git_command`], which sheds the
/// inherited `GIT_` environment. An inherited `GIT_DIR` or `GIT_INDEX_FILE`
/// otherwise aims the question at another repository. The output is captured,
/// so nothing that git writes reaches the stdout of `nwt`.
fn check_ignore_status(worktree: &Path, name: &OsStr) -> Option<i32> {
    let mut command = production_git_command(worktree);
    command
        .args(["check-ignore", "-q", "--"])
        .arg(pathspec_of(name));
    command.output().ok()?.status.code()
}

/// The pathspec that names the entry `name` at the root of the worktree.
///
/// Git reads a leading `:` in a pathspec as magic, so a bare `:vial` asks
/// about `vial`. [`CURRENT_DIRECTORY_PREFIX`] stops that. The name stays an
/// [`OsStr`], so a name that is not UTF-8 reaches git as it is.
fn pathspec_of(name: &OsStr) -> OsString {
    let mut pathspec = OsString::from(CURRENT_DIRECTORY_PREFIX);
    pathspec.push(name);
    pathspec
}

/// A rule of an ignore file that matches only a directory, as git names it.
#[derive(Debug, PartialEq, Eq)]
struct DirectoryOnlyRule {
    /// The file that holds the rule, as git names it.
    source: String,
    /// The number of the line of the rule in `source`.
    line: String,
    /// The pattern, as `source` holds it.
    pattern: String,
}

/// Ask git in `worktree` which rule matches `./<name>/`, the directory form of
/// the child, and hand back that rule when it matches only a directory.
///
/// The caller asks only after git said that it does not ignore `./<name>`, and
/// before a link exists. With a symlink at `<name>`, git refuses the question
/// ("beyond a symbolic link"). The question goes through
/// `git check-ignore -v -z --stdin`, because `-z` works only with `--stdin`.
/// With `-z`, git reads the path as it is, and it writes each field of the
/// answer with a NUL byte after it.
///
/// Returns `None` when no rule matches, when the rule does not match only a
/// directory, or when git fails. Each of those cases gets the same warning. The
/// command goes through [`production_git_command`], as the first question
/// does, and each stream is captured.
fn directory_rule_of(worktree: &Path, name: &OsStr) -> Option<DirectoryOnlyRule> {
    let mut command = production_git_command(worktree);
    command
        .args(["check-ignore", "-v", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().ok()?;

    // The closure takes stdin and drops it, which closes the pipe. Git then
    // reads the end of its input, and the wait below cannot stop on it.
    let written = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(&directory_query_of(name)).is_ok());
    let output = child.wait_with_output().ok()?;

    if !written || !output.status.success() {
        return None;
    }
    directory_only_rule(&output.stdout)
}

/// The input of `git check-ignore -z --stdin` that asks about the directory
/// form of `name`: `./<name>/`, and the NUL byte that ends a path.
///
/// The bytes of the name go to git as they are, so a name that is not UTF-8
/// also reaches git unchanged.
fn directory_query_of(name: &OsStr) -> Vec<u8> {
    let mut query = CURRENT_DIRECTORY_PREFIX.as_bytes().to_vec();
    query.extend_from_slice(name.as_encoded_bytes());
    query.extend_from_slice(DIRECTORY_SUFFIX);
    query.push(FIELD_END);
    query
}

/// The rule that `stdout` names, when that rule matches only a directory.
///
/// `stdout` is the output of `git check-ignore -v -z --stdin` for one path. A
/// match is one record of four fields: the source, the line, the pattern, and
/// the path. A NUL byte ends each field, so a `:` in the source stays in the
/// source. A pattern that matches only a directory ends with `/`. A pattern
/// that starts with `!` is a negation, and it ignores nothing.
///
/// Returns `None` for no output, for a record that is not complete, and for a
/// pattern that does not match only a directory.
fn directory_only_rule(stdout: &[u8]) -> Option<DirectoryOnlyRule> {
    let record = stdout.strip_suffix(&[FIELD_END])?;
    let fields: Vec<&[u8]> = record.split(|byte| *byte == FIELD_END).collect();
    let [source, line, pattern, _path] = fields.as_slice() else {
        return None;
    };
    if !pattern.ends_with(DIRECTORY_SUFFIX) || pattern.starts_with(NEGATION_PREFIX) {
        return None;
    }

    Some(DirectoryOnlyRule {
        source: String::from_utf8_lossy(source).into_owned(),
        line: String::from_utf8_lossy(line).into_owned(),
        pattern: String::from_utf8_lossy(pattern).into_owned(),
    })
}

/// Write the line for one child to stderr.
///
/// `quiet` removes the line that reports a link, and the line for a path that
/// is already there. It does not remove a warning, because a missing link is a
/// defect that the user must see.
fn report(name: &OsStr, target: &Path, outcome: &Outcome, quiet: bool) {
    match outcome {
        Outcome::Linked => {
            if !quiet {
                eprintln!("{}", linked_line(name, target));
            }
        }
        Outcome::AlreadyThere => {
            if !quiet {
                eprintln!("{}", already_there_line(name));
            }
        }
        Outcome::NotIgnored(rule) => {
            let reason = not_ignored_reason(name, rule.as_ref());
            eprintln!("{}", not_linked_line(name, &reason));
        }
        Outcome::LinkFailed(error) => eprintln!("{}", not_linked_line(name, error)),
        Outcome::NoAnswer => {}
    }
}

/// Why the child `name`, which git does not ignore, has no link.
///
/// `rule` is the rule that matches only the directory form of the path. The
/// reason then names that rule, because the user thinks it ignores the child.
/// Without such a rule, nothing ignores the child, and the reason gives only
/// the fix.
fn not_ignored_reason(name: &OsStr, rule: Option<&DirectoryOnlyRule>) -> String {
    let name = name.to_string_lossy();
    match rule {
        Some(DirectoryOnlyRule {
            source,
            line,
            pattern,
        }) => format!(
            "{source}:{line} has '{pattern}', which matches only a directory, and git sees \
             a symlink as a file. Write '{ROOT_ANCHOR}{name}' to link it"
        ),
        None => format!(
            "git does not ignore this path. Add '{ROOT_ANCHOR}{name}' to {IGNORE_FILE} to link it"
        ),
    }
}

/// The line for the child `name`, whose path the new worktree already holds.
fn already_there_line(name: &OsStr) -> String {
    format!(
        "{ALREADY_THERE_PREFIX} {} ({ALREADY_THERE_REASON})",
        name.to_string_lossy()
    )
}

/// The warning for the child `name`, which has no link because of `reason`.
fn not_linked_line(name: &OsStr, reason: &dyn fmt::Display) -> String {
    format!("{NOT_LINKED_PREFIX} {} ({reason})", name.to_string_lossy())
}

/// The line that reports the link of the child `name` to `target`.
fn linked_line(name: &OsStr, target: &Path) -> String {
    format!(
        "{LINKED_WORD} {} -> {}",
        name.to_string_lossy(),
        target.display()
    )
}

/// The line that counts the links of one run.
fn summary_line(linked: usize) -> String {
    let noun = if linked == 1 {
        "child repository"
    } else {
        "child repositories"
    };
    format!("{LINKED_WORD} {linked} {noun} from the main worktree")
}

/// Make the symlink `link` that points at the directory `target`.
#[cfg(unix)]
fn make_directory_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Make the symlink `link` that points at the directory `target`.
///
/// Windows keeps a link to a directory apart from a link to a file, and a
/// child repository is a directory.
#[cfg(windows)]
fn make_directory_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path that appears after the read, and before the symlink, is a path
    /// that the new worktree already holds. The operating system gives the
    /// error of kind `AlreadyExists` for it, and the outcome keeps that path.
    #[cfg(unix)]
    #[test]
    fn a_link_onto_a_path_that_appeared_is_already_there() {
        let temp = tempfile::TempDir::new().expect("create a temporary directory");
        let link = temp.path().join("vial");
        std::fs::write(&link, "made by a race\n").expect("write the file at the link path");

        let outcome = link_outcome(make_directory_link(temp.path(), &link));

        assert!(
            matches!(outcome, Outcome::AlreadyThere),
            "a symlink onto a path that is there must give the outcome AlreadyThere"
        );
    }

    /// An error of another kind stays a failure, with the same error.
    #[test]
    fn a_link_error_of_another_kind_stays_a_failure() {
        let outcome = link_outcome(Err(io::Error::from(io::ErrorKind::PermissionDenied)));

        assert!(
            matches!(
                &outcome,
                Outcome::LinkFailed(error) if error.kind() == io::ErrorKind::PermissionDenied
            ),
            "an error that is not AlreadyExists must give the outcome LinkFailed"
        );
    }

    /// The rule that [`directory_only_rule`] gives for `source`, `line`, and
    /// `pattern`.
    fn rule(source: &str, line: &str, pattern: &str) -> Option<DirectoryOnlyRule> {
        Some(DirectoryOnlyRule {
            source: source.to_string(),
            line: line.to_string(),
            pattern: pattern.to_string(),
        })
    }

    /// One record of four fields, each one followed by a NUL byte, names the
    /// source, the line, and the pattern of the rule.
    #[test]
    fn one_match_names_the_rule() {
        assert_eq!(
            directory_only_rule(b".gitignore\x001\x00vial/\x00./vial/\x00"),
            rule(".gitignore", "1", "vial/")
        );
    }

    /// Git writes nothing when no rule matches.
    #[test]
    fn no_output_names_no_rule() {
        assert_eq!(directory_only_rule(b""), None);
    }

    /// A record that stops before its fourth field names no rule.
    #[test]
    fn a_truncated_record_names_no_rule() {
        assert_eq!(directory_only_rule(b".gitignore\x001\x00vial/\x00"), None);
        assert_eq!(
            directory_only_rule(b".gitignore\x001\x00vial/\x00./vial/"),
            None
        );
    }

    /// A pattern without a trailing slash also matches a symlink, so it is not
    /// the trap.
    #[test]
    fn a_pattern_without_a_trailing_slash_names_no_rule() {
        assert_eq!(
            directory_only_rule(b".gitignore\x001\x00*\x00./vial/\x00"),
            None
        );
    }

    /// A negation does not ignore the directory, so it is not the trap.
    #[test]
    fn a_negation_names_no_rule() {
        assert_eq!(
            directory_only_rule(b".gitignore\x001\x00!vial/\x00./vial/\x00"),
            None
        );
    }

    /// The fields are separated by NUL bytes, so a `:` in the source stays in
    /// the source.
    #[test]
    fn a_source_that_holds_a_colon_stays_whole() {
        assert_eq!(
            directory_only_rule(b"/tmp/a:b/.gitignore\x0012\x00**/vial/\x00./vial/\x00"),
            rule("/tmp/a:b/.gitignore", "12", "**/vial/")
        );
    }

    /// The first line of the stderr of git is the error, and a hint can follow
    /// it.
    #[test]
    fn the_first_line_of_stderr_is_the_error() {
        assert_eq!(
            first_line_of(b"fatal: not a git repository\nhint: run git init\n"),
            Some("fatal: not a git repository".to_string())
        );
    }

    /// The line loses the space around it, and the line break of Windows.
    #[test]
    fn the_first_line_of_stderr_loses_the_space_around_it() {
        assert_eq!(
            first_line_of(b"  fatal: padded \t\r\n"),
            Some("fatal: padded".to_string())
        );
    }

    /// A blank line before the error does not hide the error.
    #[test]
    fn a_blank_line_before_the_error_is_skipped() {
        assert_eq!(
            first_line_of(b"\n  \nfatal: after blank lines\n"),
            Some("fatal: after blank lines".to_string())
        );
    }

    /// A stderr that holds no text gives no line.
    #[test]
    fn a_stderr_without_text_gives_no_line() {
        assert_eq!(first_line_of(b""), None);
        assert_eq!(first_line_of(b" \n\t\n"), None);
    }

    /// The warning for a git that failed repeats the error of git.
    #[test]
    fn the_warning_for_a_git_that_failed_repeats_its_error() {
        let failure = GitFailure::Failed("fatal: not a git repository".to_string());

        assert_eq!(
            not_linked_line(OsStr::new("vial"), &failure),
            "Warning: not linked: vial (git check-ignore failed: fatal: not a git repository)"
        );
    }

    /// The warning for a git that did not start names the error of the
    /// operating system.
    #[test]
    fn the_warning_for_a_git_that_did_not_start_names_the_error() {
        let error = io::Error::from(io::ErrorKind::NotFound);
        let expected =
            format!("Warning: not linked: vial (git check-ignore did not start: {error})");

        assert_eq!(
            not_linked_line(OsStr::new("vial"), &GitFailure::DidNotStart(error)),
            expected
        );
    }
}
