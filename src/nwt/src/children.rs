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
//! [`repowalker::child_repositories`] finds the candidates, so `nwt` and `cwt`
//! start from one definition: a directory one level below the main worktree
//! that holds a `.git` entry. `nwt` then removes each worktree of this
//! repository, because a worktree of this repository is not a child. The
//! setting `nwt.worktreesDir '.'` puts each worktree one level below the main
//! worktree, and each one holds a `.git` file. `git worktree add` makes the new
//! worktree before this module runs, so without that step the new worktree
//! gets a link to itself, a loop that a tool which follows links reads without
//! end. [`NotChildren`] holds the entries to remove, and `git worktree list`
//! names them. A linked worktree of another repository stays a child.
//!
//! This module asks git for the worktrees only when a candidate exists, so a
//! repository without candidates starts no git process here. When git gives
//! no list, this module cannot tell a child from a worktree, and it makes no
//! link. One warning then repeats the first line of the error of git.
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
//! about `<name>/`, before a link exists. The order matters, because git
//! refuses a question about `<name>/` when a symlink is at `<name>` ("beyond a
//! symbolic link"). When a rule that ends with `/` matches that form, the
//! warning names the rule, its file, and its line, and it gives the fix
//! `/<name>`. Otherwise nothing ignores the child, and the warning gives only
//! the fix.
//!
//! When git gives no answer to the first question, this module makes no link.
//! The warning then repeats the first line of the error of git, or the error
//! when git does not start.
//!
//! Thus a child without a link gets one of three warnings: the rule that
//! matches only a directory, no rule at all, or the error of git. A symlink
//! that fails gives a fourth, with the error of the operating system. `-q`
//! removes the line for each link, the line for each path that is already
//! there, and the summary. It does not remove a warning, because a warning
//! names a defect in the repository. The warning for a run without a list of
//! worktrees stays too.
//!
//! The target of each link is absolute: the main worktree that git names,
//! joined with the name of the child. Git keeps worktree paths absolute too, so
//! a relative link gives no more safety.
//!
//! The shell wrapper reads the worktree path from stdout, so each line of this
//! module goes to stderr, and the git child writes into a captured buffer.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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

/// The words of the subcommand of git that asks whether git ignores a path.
///
/// Git reads these words, and the warning for a git that gives no answer names
/// them, so the two cannot differ.
const CHECK_IGNORE: GitSubcommand = &["check-ignore"];

/// The words of the subcommand of git that names each worktree of the
/// repository.
const WORKTREE_LIST: GitSubcommand = &["worktree", "list"];

/// The options that make `git worktree list` write one field for each line of
/// its porcelain format, with a NUL byte after each field.
const WORKTREE_LIST_OPTIONS: [&str; 2] = ["--porcelain", "-z"];

/// The start of the field that names a worktree in the output of
/// `git worktree list --porcelain -z`. The path of the worktree follows it.
const WORKTREE_FIELD_PREFIX: &[u8] = b"worktree ";

/// The warning for a run that makes no link, because git gave no list of the
/// worktrees. The reason in parentheses follows it.
const NO_CHILD_LINKED: &str =
    "Warning: no child linked, because nwt cannot tell which directories are children";

/// The words of a subcommand of git, in the order that git reads them.
type GitSubcommand = &'static [&'static str];

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
    NoAnswer(GitFailure),
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
///
/// Each candidate that [`NotChildren`] holds gets no link and no line, because
/// it is not a child. The function asks git for [`NotChildren`] only when a
/// candidate exists. When git gives no answer, the function makes no link, and
/// it writes one warning that `quiet` does not remove.
pub(crate) fn link_children(main_worktree: &Path, worktree: &Path, quiet: bool) {
    let candidates = repowalker::child_repositories(main_worktree);
    if candidates.is_empty() {
        return;
    }
    let not_children = match NotChildren::of(main_worktree, worktree) {
        Ok(not_children) => not_children,
        Err(failure) => {
            eprintln!("{}", no_child_linked_line(&failure));
            return;
        }
    };

    let mut linked = 0_usize;

    for child in candidates {
        if not_children.holds(&child) {
            continue;
        }
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

    match check_ignore(worktree, name) {
        Ok(IgnoreAnswer::Ignored) => {}
        Ok(IgnoreAnswer::NotIgnored) => {
            return Outcome::NotIgnored(directory_rule_of(worktree, name));
        }
        Err(failure) => return Outcome::NoAnswer(failure),
    }

    link_outcome(make_directory_link(target, &link))
}

/// The answer of git to the question whether it ignores a path.
enum IgnoreAnswer {
    /// Git ignores the path, so a link there is safe.
    Ignored,
    /// Git does not ignore the path, so a link there shows as untracked.
    NotIgnored,
}

/// Why a subcommand of git gave no answer.
///
/// The text is the reason of a warning: `git <subcommand> failed: <error>`, or
/// `git <subcommand> did not start: <error>`.
struct GitFailure {
    /// The subcommand that gave no answer.
    subcommand: GitSubcommand,
    /// What went wrong.
    cause: FailureCause,
}

/// What went wrong when a subcommand of git gave no answer.
enum FailureCause {
    /// Git exited with a status that is not an answer, or a signal stopped it.
    /// The text is the first line of its stderr, or the exit status when its
    /// stderr holds no text.
    Failed(String),
    /// Git did not start, with this error.
    DidNotStart(io::Error),
}

impl GitFailure {
    /// The failure of `subcommand`, whose `output` holds a status that is not
    /// an answer.
    ///
    /// The text is the first line of the stderr of git, because git writes the
    /// error there. It is the exit status when that stderr holds no text.
    fn failed(subcommand: GitSubcommand, output: &Output) -> Self {
        let text = first_line_of(&output.stderr).unwrap_or_else(|| output.status.to_string());
        Self {
            subcommand,
            cause: FailureCause::Failed(text),
        }
    }
}

impl fmt::Display for GitFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let subcommand = self.subcommand.join(" ");
        match &self.cause {
            FailureCause::Failed(text) => write!(f, "git {subcommand} failed: {text}"),
            FailureCause::DidNotStart(error) => {
                write!(f, "git {subcommand} did not start: {error}")
            }
        }
    }
}

/// Run `command`, which runs the git `subcommand`, to its end with each stream
/// captured, and hand back its output.
///
/// # Errors
///
/// Returns a [`GitFailure`] with [`FailureCause::DidNotStart`] when git does not
/// start.
fn captured_output(mut command: Command, subcommand: GitSubcommand) -> Result<Output, GitFailure> {
    command.output().map_err(|error| GitFailure {
        subcommand,
        cause: FailureCause::DidNotStart(error),
    })
}

/// The first line of `stderr` that holds text, without the space around it.
///
/// Git writes the error on its first line, and a hint can follow it. Returns
/// `None` when `stderr` holds no text.
fn first_line_of(stderr: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
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

/// The answer of `git check-ignore -q -- ./<name>` in `worktree`.
///
/// The status 0 says that git ignores the path, and the status 1 says that it
/// does not.
///
/// # Errors
///
/// Returns a [`GitFailure`] with [`FailureCause::DidNotStart`] when git does
/// not start, and with [`FailureCause::Failed`] for each other status or for a
/// signal.
///
/// The command goes through [`production_git_command`], which sheds the
/// inherited `GIT_` environment. An inherited `GIT_DIR` or `GIT_INDEX_FILE`
/// otherwise aims the question at another repository. The output is captured,
/// so nothing that git writes reaches the stdout of `nwt`, and the error of git
/// can go into the warning.
fn check_ignore(worktree: &Path, name: &OsStr) -> Result<IgnoreAnswer, GitFailure> {
    let mut command = production_git_command(worktree);
    command
        .args(CHECK_IGNORE)
        .args(["-q", "--"])
        .arg(pathspec_of(name));
    let output = captured_output(command, CHECK_IGNORE)?;

    match output.status.code() {
        Some(CHECK_IGNORE_IGNORED) => Ok(IgnoreAnswer::Ignored),
        Some(CHECK_IGNORE_NOT_IGNORED) => Ok(IgnoreAnswer::NotIgnored),
        _ => Err(GitFailure::failed(CHECK_IGNORE, &output)),
    }
}

/// The entries one level below the main worktree that hold a `.git` entry, and
/// that are not children.
///
/// [`repowalker::child_repositories`] finds each directory one level below the
/// main worktree that holds a `.git` entry. Some of those directories are not
/// children, and this set holds them. [`link_children`] skips each candidate
/// that this set holds, with no line.
///
/// Each path is canonical, so a path that reaches a directory through a
/// symlink matches the path that reaches it directly. On macOS, `/var` is a
/// symlink to `/private/var`. A path that cannot be made canonical stays as it
/// is.
struct NotChildren(HashSet<PathBuf>);

impl NotChildren {
    /// The entries of `main_worktree` that are not children, when `worktree` is
    /// the new worktree.
    ///
    /// A worktree of this repository is not a child. `git worktree list` in
    /// `main_worktree` names each worktree, and the new worktree comes from
    /// `worktree` too, with no question to git. A linked worktree of another
    /// repository holds a `.git` file too, but git does not name it here, so it
    /// stays a child.
    ///
    /// Each kind of entry that is not a child goes into the one chain below, so
    /// [`link_children`] reads one set and makes one pass.
    ///
    /// # Errors
    ///
    /// Returns the [`GitFailure`] of `git worktree list`. Without the list, a
    /// child and a worktree look the same.
    fn of(main_worktree: &Path, worktree: &Path) -> Result<Self, GitFailure> {
        let worktrees = worktree_paths(main_worktree)?;
        Ok(Self::from_paths(
            std::iter::once(worktree.to_path_buf()).chain(worktrees),
        ))
    }

    /// The set of `paths`, each one canonical.
    fn from_paths(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self(
            paths
                .into_iter()
                .map(|path| canonical_or_raw(&path))
                .collect(),
        )
    }

    /// Whether the candidate `path` is an entry that is not a child.
    fn holds(&self, path: &Path) -> bool {
        self.0.contains(&canonical_or_raw(path))
    }
}

/// The canonical form of `path`, or `path` as it is when it cannot be made
/// canonical.
///
/// `git worktree list` can name a worktree whose directory is gone. Such a path
/// cannot be made canonical, and no candidate reaches it.
fn canonical_or_raw(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The path of each worktree of the repository at `main_worktree`, as
/// `git worktree list --porcelain -z` names it.
///
/// # Errors
///
/// Returns a [`GitFailure`] when git does not start, or when it exits with a
/// status that is not 0. The command goes through [`production_git_command`],
/// as the question about an ignored path does, and each stream is captured.
fn worktree_paths(main_worktree: &Path) -> Result<Vec<PathBuf>, GitFailure> {
    let mut command = production_git_command(main_worktree);
    command.args(WORKTREE_LIST).args(WORKTREE_LIST_OPTIONS);
    let output = captured_output(command, WORKTREE_LIST)?;

    if !output.status.success() {
        return Err(GitFailure::failed(WORKTREE_LIST, &output));
    }
    Ok(worktree_paths_of(&output.stdout))
}

/// The path of each worktree that `stdout` names, in order.
///
/// `stdout` is the output of `git worktree list --porcelain -z`. Each field of
/// a record ends with a NUL byte, and an empty field ends the record. The first
/// field of each record is `worktree <path>`, and no other field starts with
/// that label. A NUL byte cannot occur in a path, so a space or a line break in
/// a path stays in the path.
fn worktree_paths_of(stdout: &[u8]) -> Vec<PathBuf> {
    stdout
        .split(|byte| *byte == FIELD_END)
        .filter_map(|field| field.strip_prefix(WORKTREE_FIELD_PREFIX))
        .map(path_of_bytes)
        .collect()
}

/// The path that git wrote as `bytes`.
///
/// Git writes a path as the bytes that the operating system gives it, so a
/// path that is not UTF-8 stays as it is.
#[cfg(unix)]
fn path_of_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;

    PathBuf::from(OsStr::from_bytes(bytes))
}

/// The path that git wrote as `bytes`.
///
/// Git for Windows writes each path as UTF-8.
#[cfg(not(unix))]
fn path_of_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
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
        .args(CHECK_IGNORE)
        .args(["-v", "-z", "--stdin"])
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
        Outcome::NoAnswer(failure) => eprintln!("{}", not_linked_line(name, failure)),
        Outcome::LinkFailed(error) => eprintln!("{}", not_linked_line(name, error)),
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

/// The warning for a run that makes no link, because git gave no list of the
/// worktrees, with `failure`.
fn no_child_linked_line(failure: &GitFailure) -> String {
    format!("{NO_CHILD_LINKED} ({failure})")
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

/// The lines of this module that the documents of `nwt` show as samples, for
/// children of `main_worktree`.
///
/// The CHILD REPOSITORY LINKS section of `--help` and the `### Child Repository
/// Links` section of the README hold these samples. Each line comes from the
/// function that prints it, so a test can hold each document to the code. The
/// samples are: two links, their summary, the line for a path that is already
/// there, the two warnings for a child that git does not ignore, and the
/// warning for a run without a list of worktrees.
#[cfg(test)]
pub(crate) fn sample_lines(main_worktree: &Path) -> Vec<String> {
    let vial = OsStr::new("vial");
    let rule = DirectoryOnlyRule {
        source: IGNORE_FILE.to_string(),
        line: "1".to_string(),
        pattern: "vial/".to_string(),
    };

    let mut lines: Vec<String> = ["vial-qmk", "zmk-config-corne"]
        .iter()
        .map(|name| linked_line(OsStr::new(name), &main_worktree.join(name)))
        .collect();
    lines.push(summary_line(2));
    lines.push(already_there_line(vial));
    lines.push(not_linked_line(
        vial,
        &not_ignored_reason(vial, Some(&rule)),
    ));
    lines.push(not_linked_line(vial, &not_ignored_reason(vial, None)));
    lines.push(no_child_linked_line(&GitFailure {
        subcommand: WORKTREE_LIST,
        cause: FailureCause::Failed(SAMPLE_GIT_ERROR.to_string()),
    }));
    lines
}

/// The error of git in the samples of the documents.
#[cfg(test)]
const SAMPLE_GIT_ERROR: &str = "fatal: not a git repository";

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

    /// The failure of the git `subcommand` that exited with a status that is
    /// not an answer, and wrote `text` as its error.
    fn failed(subcommand: GitSubcommand, text: &str) -> GitFailure {
        GitFailure {
            subcommand,
            cause: FailureCause::Failed(text.to_string()),
        }
    }

    /// The failure of the git `subcommand` that did not start, with `error`.
    fn did_not_start(subcommand: GitSubcommand, error: io::Error) -> GitFailure {
        GitFailure {
            subcommand,
            cause: FailureCause::DidNotStart(error),
        }
    }

    /// The warning for a git that failed repeats the error of git.
    #[test]
    fn the_warning_for_a_git_that_failed_repeats_its_error() {
        let failure = failed(CHECK_IGNORE, "fatal: not a git repository");

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
            not_linked_line(OsStr::new("vial"), &did_not_start(CHECK_IGNORE, error)),
            expected
        );
    }

    /// A run without a list of worktrees makes no link, and its one warning
    /// repeats the error of `git worktree list`.
    #[test]
    fn the_warning_for_a_worktree_list_that_failed_repeats_its_error() {
        let failure = failed(WORKTREE_LIST, "fatal: not a git repository");

        assert_eq!(
            no_child_linked_line(&failure),
            "Warning: no child linked, because nwt cannot tell which directories are children \
             (git worktree list failed: fatal: not a git repository)"
        );
    }

    /// A run whose `git worktree list` did not start makes no link, and its one
    /// warning names the error of the operating system.
    #[test]
    fn the_warning_for_a_worktree_list_that_did_not_start_names_the_error() {
        let error = io::Error::from(io::ErrorKind::NotFound);
        let expected = format!(
            "Warning: no child linked, because nwt cannot tell which directories are children \
             (git worktree list did not start: {error})"
        );

        assert_eq!(
            no_child_linked_line(&did_not_start(WORKTREE_LIST, error)),
            expected
        );
    }

    /// A git that fails with no text on its stderr gives its exit status as
    /// the error.
    #[cfg(unix)]
    #[test]
    fn a_failure_without_stderr_names_the_exit_status() {
        use std::os::unix::process::ExitStatusExt;

        let output = Output {
            status: std::process::ExitStatus::from_raw(128 << 8),
            stdout: Vec::new(),
            stderr: b" \n".to_vec(),
        };

        assert_eq!(
            GitFailure::failed(WORKTREE_LIST, &output).to_string(),
            format!("git worktree list failed: {}", output.status)
        );
    }

    /// Each record of `git worktree list --porcelain -z` names one worktree,
    /// in order. The fields after the path are not paths, and the record of a
    /// bare repository names its directory too.
    #[test]
    fn each_record_of_the_worktree_list_names_one_worktree() {
        let stdout = b"worktree /srv/keyboards.git\0bare\0\0\
            worktree /srv/keyboards/first\0HEAD 1111111111111111111111111111111111111111\0\
            branch refs/heads/first\0\0\
            worktree /srv/keyboards/second\0HEAD 2222222222222222222222222222222222222222\0\
            detached\0locked worktree /srv/elsewhere\0\0";

        assert_eq!(
            worktree_paths_of(stdout),
            vec![
                PathBuf::from("/srv/keyboards.git"),
                PathBuf::from("/srv/keyboards/first"),
                PathBuf::from("/srv/keyboards/second"),
            ]
        );
    }

    /// A NUL byte ends each field, so a space and a line break stay in the
    /// path.
    #[test]
    fn a_worktree_path_with_a_space_stays_whole() {
        assert_eq!(
            worktree_paths_of(b"worktree /srv/my keyboards/new\nline\0HEAD 1\0detached\0\0"),
            vec![PathBuf::from("/srv/my keyboards/new\nline")]
        );
    }

    /// No output names no worktree.
    #[test]
    fn no_output_names_no_worktree() {
        assert_eq!(worktree_paths_of(b""), Vec::<PathBuf>::new());
    }

    /// A path that is not UTF-8 reaches the set as the bytes that git wrote.
    #[cfg(unix)]
    #[test]
    fn a_worktree_path_that_is_not_utf8_stays_as_git_wrote_it() {
        use std::os::unix::ffi::OsStrExt;

        assert_eq!(
            worktree_paths_of(b"worktree /srv/caf\xe9\0detached\0\0"),
            vec![PathBuf::from(OsStr::from_bytes(b"/srv/caf\xe9"))]
        );
    }

    /// A candidate that reaches an entry through a symlink matches the entry,
    /// because each path is canonical. On macOS, the temporary directory itself
    /// sits below the symlink `/var`.
    #[cfg(unix)]
    #[test]
    fn an_entry_reached_through_a_symlink_is_held() {
        let temp = tempfile::TempDir::new().expect("create a temporary directory");
        let worktree = temp.path().join("first");
        fs::create_dir(&worktree).expect("create the worktree directory");
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(temp.path(), &alias).expect("make the symlink");

        let not_children = NotChildren::from_paths([worktree]);

        assert!(not_children.holds(&alias.join("first")));
        assert!(!not_children.holds(&temp.path().join("vial")));
    }

    /// A path that cannot be made canonical, such as a worktree whose
    /// directory is gone, stays in the set as it is.
    #[test]
    fn a_path_that_is_gone_is_held_as_it_is() {
        let temp = tempfile::TempDir::new().expect("create a temporary directory");
        let gone = temp.path().join("gone");

        let not_children = NotChildren::from_paths([gone.clone()]);

        assert!(not_children.holds(&gone));
    }
}
