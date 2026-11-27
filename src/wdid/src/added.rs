//! The added date of a definition: the date of the oldest commit that adds a
//! line that defines its name to its origin file.
//!
//! [`defined_names`] reads one line of a shell file and gives the names that
//! the line defines. [`added_dates`] reads the history of one file, with its full
//! diff, and gives the oldest date for each name that an added line defines.
//! Both functions are pure. [`read`] starts the processes: one `git log` for
//! each origin file, and for yadm one `yadm introspect repo` and one
//! `git ls-files`.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use chrono::NaiveDate;

use crate::catalog::{LOG_DATE_FORMAT, LOG_RECORD_START};
use crate::command_words::{split_words, Argument, SUFFIX_MARKER};

/// The family of a name that a line of a shell file defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Family {
    /// An alias of one of the three kinds.
    Alias,
    /// A function.
    Function,
}

/// A name that one line of a shell file defines.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Defined {
    /// The family of the definition.
    pub family: Family,
    /// The name. A suffix alias has its marker, `*.` and the suffix (`*.md`),
    /// as in [`crate::command_words::AliasDefinition::name`].
    pub name: String,
}

impl Defined {
    /// Makes the alias `name`.
    #[must_use]
    pub fn alias(name: impl Into<String>) -> Self {
        Self {
            family: Family::Alias,
            name: name.into(),
        }
    }

    /// Makes the function `name`.
    #[must_use]
    pub fn function(name: impl Into<String>) -> Self {
        Self {
            family: Family::Function,
            name: name.into(),
        }
    }
}

/// The programs that [`read`] starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Programs {
    /// The git program: a name that the PATH resolves, or a path.
    pub git: OsString,
    /// The yadm program: a name that the PATH resolves, or a path.
    pub yadm: OsString,
}

impl Default for Programs {
    fn default() -> Self {
        Self {
            git: OsString::from("git"),
            yadm: OsString::from("yadm"),
        }
    }
}

/// The added dates of the names that each origin file defines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddedDates {
    /// The dates of each file, by name.
    files: BTreeMap<PathBuf, BTreeMap<Defined, NaiveDate>>,
}

impl AddedDates {
    /// Gives the added date of `defined` in `file`, or `None` when it is
    /// unknown.
    #[must_use]
    pub fn get(&self, file: &Path, defined: &Defined) -> Option<NaiveDate> {
        self.files.get(file)?.get(defined).copied()
    }

    /// Records `date` as the added date of `defined` in `file`. When a date
    /// is already known, the older date stays.
    pub fn record(&mut self, file: impl Into<PathBuf>, defined: Defined, date: NaiveDate) {
        self.files
            .entry(file.into())
            .or_default()
            .entry(defined)
            .and_modify(|known| *known = (*known).min(date))
            .or_insert(date);
    }
}

/// The arguments of the `git log` that gives the history of one file, before
/// `--` and the path.
///
/// `--literal-pathspecs` makes a `*` or a `:` in the name a plain character.
/// `--follow` reads the history through each rename of the file. The other
/// flags make the output the same for each configuration of git: no color, no
/// external diff, no text conversion, and no signature lines.
const LOG_ARGUMENTS: [&str; 10] = [
    "--literal-pathspecs",
    "log",
    "--follow",
    "--format=%x00%as",
    "--patch",
    "--unified=0",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--no-show-signature",
];

/// The argument that ends the options of git, before the path.
const END_OF_GIT_OPTIONS: &str = "--";

/// The arguments of `yadm` that print the git directory of yadm.
const YADM_REPO_ARGUMENTS: [&str; 2] = ["introspect", "repo"];

/// The arguments of the `git ls-files` that lists the files that yadm tracks,
/// each one ended by a NUL.
const LIST_ARGUMENTS: [&str; 2] = ["ls-files", "-z"];

/// The byte that ends each path of [`LIST_ARGUMENTS`].
const LIST_SEPARATOR: u8 = b'\0';

/// The option of git that names the git directory.
const GIT_DIR_OPTION: &str = "--git-dir";

/// The option of git that names the work tree.
const WORK_TREE_OPTION: &str = "--work-tree";

/// The text between a file and the condition of a yadm alternate, as in
/// `.zshrc##template.default`.
const ALTERNATE_SEPARATOR: &str = "##";

/// Reads the added date of each name that each file of `files` defines.
///
/// Each file gets the history of the first source that applies:
///
/// 1. **yadm.** The file is under `home`, and yadm tracks it. `yadm introspect
///    repo` gives the git directory, and git runs with that git directory and
///    with `home` as the work tree.
/// 2. **A yadm alternate.** The file is under `home`, yadm does not track it,
///    and yadm tracks one or more files `<file>##<condition>`, for example
///    `.zshrc##template.default` for the rendered `.zshrc`. Each such file
///    gives its history, and the oldest date of a name wins. The text of each
///    line counts, not its number, thus a template with other line numbers
///    gives the dates of its rendered file.
/// 3. **A git work tree.** git runs in the directory of the file, or of the
///    file that a symbolic link names, and finds its repository there.
///
/// yadm is the first source because its list of files says without one more
/// process if yadm tracks the file. A file that no source tracks has no dates.
///
/// The read starts one `git log` for each file, or one for each tracked
/// alternate of it, not one for each definition. When a file is under `home`,
/// it also starts one `yadm introspect repo` and one `git ls-files` for all
/// files. When yadm does not start or fails, the read skips step 1 and step
/// 2. Each child process sheds the inherited git environment before git gets
/// its own git directory, thus a `GIT_DIR` of a hook never redirects the read.
/// A process that fails gives no dates. It is not an error, because the dates
/// are optional.
#[must_use]
pub fn read(files: &BTreeSet<PathBuf>, home: Option<&Path>, programs: &Programs) -> AddedDates {
    let yadm = home
        .filter(|home| files.iter().any(|file| below(file, home).is_some()))
        .and_then(|home| Yadm::find(home, programs));
    let mut dates = AddedDates::default();
    for file in files {
        for source in sources(file, yadm.as_ref()) {
            for (defined, date) in added_dates(&source.history(programs)) {
                dates.record(file.clone(), defined, date);
            }
        }
    }
    dates
}

/// Gives the path of `file` relative to `home`, or `None` when `file` is not
/// under `home`. "Under" compares whole path components.
fn below(file: &Path, home: &Path) -> Option<PathBuf> {
    file.strip_prefix(home).ok().map(Path::to_path_buf)
}

/// The repository of yadm.
struct Yadm<'a> {
    /// The git directory that `yadm introspect repo` gives.
    git_dir: PathBuf,
    /// The work tree: the home directory.
    home: &'a Path,
    /// Each path that yadm tracks, relative to the home directory. The paths
    /// keep their bytes, also when a name is not UTF-8.
    tracked: BTreeSet<OsString>,
}

impl<'a> Yadm<'a> {
    /// Asks yadm for its git directory, and git for the files that yadm
    /// tracks. Gives `None` when yadm does not start, when it fails, when it
    /// gives no directory, or when git cannot list the files.
    fn find(home: &'a Path, programs: &Programs) -> Option<Self> {
        let mut command = childenv::command(&programs.yadm);
        command.args(YADM_REPO_ARGUMENTS);
        let output = succeeded(&mut command)?;
        let mut line = output;
        while line
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            line.pop();
        }
        let git_dir = PathBuf::from(os_string_from_bytes(line)?);
        if git_dir.as_os_str().is_empty() {
            return None;
        }
        let mut yadm = Self {
            git_dir,
            home,
            tracked: BTreeSet::new(),
        };
        let mut list = yadm.git(programs);
        list.args(LIST_ARGUMENTS);
        yadm.tracked = succeeded(&mut list)?
            .split(|byte| *byte == LIST_SEPARATOR)
            .filter(|path| !path.is_empty())
            .filter_map(|path| os_string_from_bytes(path.to_vec()))
            .collect();
        Some(yadm)
    }

    /// Gives a git command with the git directory of yadm and the home
    /// directory as the work tree. It runs in the home directory, thus a
    /// relative path is relative to the work tree.
    fn git(&self, programs: &Programs) -> Command {
        let mut command = childenv::command(&programs.git);
        command
            .current_dir(self.home)
            .arg(GIT_DIR_OPTION)
            .arg(&self.git_dir)
            .arg(WORK_TREE_OPTION)
            .arg(self.home);
        command
    }

    /// Gives the tracked files that hold the history of `path`: `path` itself
    /// when yadm tracks it, else each tracked alternate `<path>##<condition>`.
    /// The list is empty when yadm tracks neither.
    fn tracked_files(&self, path: &Path) -> Vec<PathBuf> {
        if self.tracked.contains(path.as_os_str()) {
            return vec![path.to_path_buf()];
        }
        let mut prefix = path.as_os_str().as_encoded_bytes().to_vec();
        prefix.extend_from_slice(ALTERNATE_SEPARATOR.as_bytes());
        self.tracked
            .iter()
            .filter(|tracked| tracked.as_encoded_bytes().starts_with(&prefix))
            .map(|tracked| PathBuf::from(tracked.clone()))
            .collect()
    }
}

/// Makes a name from the bytes that a program wrote. On Unix a name is any
/// sequence of bytes, so each sequence gives a name.
#[cfg(unix)]
fn os_string_from_bytes(bytes: Vec<u8>) -> Option<OsString> {
    use std::os::unix::ffi::OsStringExt;
    Some(OsString::from_vec(bytes))
}

/// Makes a name from the bytes that a program wrote. git on Windows writes
/// UTF-8, so bytes that are not UTF-8 give no name.
#[cfg(not(unix))]
fn os_string_from_bytes(bytes: Vec<u8>) -> Option<OsString> {
    String::from_utf8(bytes).ok().map(OsString::from)
}

/// A place that holds the history of one tracked file.
enum Source<'a> {
    /// A file that yadm tracks, by its path relative to the home directory.
    Yadm {
        /// The repository of yadm.
        yadm: &'a Yadm<'a>,
        /// The path, relative to the home directory.
        path: PathBuf,
    },
    /// A file in a git work tree. git finds the repository from the directory.
    WorkTree {
        /// The directory of the file.
        directory: PathBuf,
        /// The name of the file in the directory.
        name: OsString,
    },
}

impl Source<'_> {
    /// Gives the output of the `git log` of [`LOG_ARGUMENTS`] for this source,
    /// or an empty text when git does not start or fails.
    fn history(&self, programs: &Programs) -> String {
        let (mut command, path): (Command, &OsStr) = match self {
            Self::Yadm { yadm, path } => (yadm.git(programs), path.as_os_str()),
            Self::WorkTree { directory, name } => {
                let mut command = childenv::command(&programs.git);
                command.current_dir(directory);
                (command, name.as_os_str())
            }
        };
        command
            .args(LOG_ARGUMENTS)
            .arg(END_OF_GIT_OPTIONS)
            .arg(path);
        succeeded(&mut command)
            .map(|stdout| String::from_utf8_lossy(&stdout).into_owned())
            .unwrap_or_default()
    }
}

/// Gives the sources of the history of `file`, in the order of [`read`].
fn sources<'a>(file: &Path, yadm: Option<&'a Yadm<'a>>) -> Vec<Source<'a>> {
    if let Some(yadm) = yadm {
        let tracked = below(file, yadm.home)
            .map(|path| yadm.tracked_files(&path))
            .unwrap_or_default();
        if !tracked.is_empty() {
            return tracked
                .into_iter()
                .map(|path| Source::Yadm { yadm, path })
                .collect();
        }
    }
    let target = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    match (target.parent(), target.file_name()) {
        (Some(directory), Some(name)) if directory.is_dir() => vec![Source::WorkTree {
            directory: directory.to_path_buf(),
            name: name.to_os_string(),
        }],
        _ => Vec::new(),
    }
}

/// Runs `command` with no standard input, and gives its standard output when
/// it ends with a zero status. Gives `None` when it does not start or fails.
fn succeeded(command: &mut Command) -> Option<Vec<u8>> {
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// The word that starts an alias line.
const ALIAS_WORD: &str = "alias";

/// The word that starts a function line of the keyword form.
const FUNCTION_WORD: &str = "function";

/// The word that ends the options of `alias`.
const END_OF_OPTIONS: &str = "--";

/// The character that starts an option word of `alias`.
const OPTION_START: char = '-';

/// The option letter of `alias` that defines a suffix alias.
const SUFFIX_OPTION: char = 's';

/// The character between the name and the value of an alias.
const ASSIGNMENT: char = '=';

/// The texts that the read puts at the end of an alias line, in order:
/// nothing, a closing single quote, and a closing double quote. A line that
/// opens a quote and does not close it goes on to the next line, and the
/// closing quote at the end of the line stands for that.
const LINE_ENDINGS: [&str; 3] = ["", "'", "\""];

/// The characters that a function name of a definition line does not hold,
/// other than the blanks. Each one is a quote, an expansion, or an operator
/// of the shell.
const NOT_IN_A_FUNCTION_NAME: [char; 16] = [
    '"', '\'', '`', '$', '|', '&', ';', '<', '>', '(', ')', '{', '}', '=', '#', '\\',
];

/// The character that opens the parentheses of a function line.
const OPEN: char = '(';

/// The character that closes the parentheses of a function line.
const CLOSE: char = ')';

/// The character that opens the body of a function.
const BODY_START: char = '{';

/// The character that ends a command.
const COMMAND_END: char = ';';

/// What starts the diff of one file in the history.
const FILE_HEADER: &str = "diff --git ";

/// What starts a hunk of a diff.
const HUNK_HEADER: &str = "@@";

/// The character that starts an added line in a hunk.
const ADDED_MARK: char = '+';

/// Gives the names that `line` defines, in the order of the line. Pure.
///
/// Blanks at the start of the line do not count. An alias line starts with
/// the word `alias`, then option words such as `-g` and `-s`, then one or
/// more `name=value` words. Each such word defines a name, thus
/// `alias gs='git status' gd='git diff'` defines `gs` and `gd`. A word with no
/// `=` defines nothing. The words end at the first operator or comment, thus
/// `alias s1=x; s2=y` defines only `s1`. The shell expands each unquoted brace
/// list of a word before `alias` runs, thus `alias -s {md,txt}=glow` defines
/// `*.md` and `*.txt`. A name that holds an expansion, a glob, or a brace
/// sequence such as `{1..3}` defines nothing. The option `-s` makes each name a
/// suffix alias, `*.` and the suffix. A word that starts with `-` and holds no
/// `=` is an option word, and `--` ends the options. The quotes and the
/// backslashes in a name do not count, thus `alias 'll=ls -l'` defines `ll`.
/// The value of a line that opens a quote and does not close it goes on to
/// the next line. The read closes the quote at the end of the line, thus
/// `alias a=1 b='x` defines `a` and `b`, and a word of the open value defines
/// nothing. When a different construct stays open, for example `$(`, only the
/// text up to the first `=` counts, thus `alias x="$(date` defines `x`.
///
/// A function line is `name()`, `name ()`, or `function name`. A function
/// name holds no blank, no quote, no `$`, and no operator of the shell. A
/// function line defines one name. A line that starts with `alias` and
/// defines no alias can still be a function line, thus `alias () {` defines
/// the function `alias`. A comment line defines nothing, because `#` is not in
/// a name.
#[must_use]
pub fn defined_names(line: &str) -> Vec<Defined> {
    let text = line.trim_start();
    if let Some(names) = after_word(text, ALIAS_WORD)
        .map(alias_names)
        .filter(|names| !names.is_empty())
    {
        return names;
    }
    let single = if let Some(rest) = after_word(text, FUNCTION_WORD) {
        keyword_function_name(rest)
    } else {
        parenthesized_function_name(text)
    };
    single.into_iter().collect()
}

/// Gives the text after `word` when `text` starts with `word` and a blank
/// follows it.
fn after_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(word)?;
    rest.starts_with(char::is_whitespace).then_some(rest)
}

/// Gives the aliases that the words after `alias` define.
///
/// The shell splitter gives the words. A word after an operator belongs to
/// another command, thus the read stops there. The read has two steps:
///
/// 1. The splitter reads `rest` closed by the first of [`LINE_ENDINGS`] that
///    it accepts.
/// 2. If no ending works, the splitter reads the text of `rest` up to the
///    first `=`, closed the same way. A different construct stays open, for
///    example `$(`, and only that name counts.
///
/// If both steps fail, the line defines nothing.
fn alias_names(rest: &str) -> Vec<Defined> {
    let words = closed_words(rest)
        .or_else(|| {
            let (name, _) = rest.split_once(ASSIGNMENT)?;
            closed_words(&format!("{name}{ASSIGNMENT}"))
        })
        .unwrap_or_default()
        .into_iter()
        .take_while(|argument| !matches!(argument, Argument::Operator(_)))
        .collect::<Vec<Argument>>();
    let mut suffix = false;
    let mut in_options = true;
    let mut names = Vec::new();
    for word in &words {
        if in_options {
            let text = word.text();
            if text == END_OF_OPTIONS {
                in_options = false;
                continue;
            }
            if text.starts_with(OPTION_START) && !text.contains(ASSIGNMENT) {
                suffix |= text.contains(SUFFIX_OPTION);
                continue;
            }
            in_options = false;
        }
        names.extend(word.assignment_names().into_iter().map(|name| {
            if suffix {
                Defined::alias(format!("{SUFFIX_MARKER}{name}"))
            } else {
                Defined::alias(name)
            }
        }));
    }
    names
}

/// Gives the words of `text` closed by the first of [`LINE_ENDINGS`] that the
/// splitter accepts, or `None` when no ending works.
fn closed_words(text: &str) -> Option<Vec<Argument>> {
    LINE_ENDINGS
        .iter()
        .find_map(|ending| split_words(&format!("{text}{ending}")))
}

/// Gives the function that the words after `function` define.
fn keyword_function_name(rest: &str) -> Option<Defined> {
    let rest = rest.trim_start();
    let end = rest
        .find(|character: char| {
            character.is_whitespace() || [OPEN, BODY_START, COMMAND_END].contains(&character)
        })
        .unwrap_or(rest.len());
    let name = rest.get(..end)?;
    is_function_name(name).then(|| Defined::function(name))
}

/// Gives the function of a line of the form `name()` or `name ()`.
fn parenthesized_function_name(text: &str) -> Option<Defined> {
    let end = text.find(|character: char| character.is_whitespace() || character == OPEN)?;
    let (name, rest) = text.split_at_checked(end)?;
    let inside = rest.trim_start().strip_prefix(OPEN)?.trim_start();
    (inside.starts_with(CLOSE) && is_function_name(name)).then(|| Defined::function(name))
}

/// Says if `name` can be the name of a function line: not empty, with no
/// blank and no character of [`NOT_IN_A_FUNCTION_NAME`].
fn is_function_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(|character: char| {
            character.is_whitespace() || NOT_IN_A_FUNCTION_NAME.contains(&character)
        })
}

/// Gives the oldest date of the commits that add a line that defines each
/// name. Pure.
///
/// `log` is the history of one file: for each commit, a NUL, the date in the
/// form `YYYY-MM-DD`, then the diff of the commit. Only a line that starts
/// with `+` inside a hunk is an added line. Thus the file header `+++ b/…`
/// and a removed line define nothing. A record whose first line is not a date
/// adds nothing. A name that no added line defines has no entry.
#[must_use]
pub fn added_dates(log: &str) -> BTreeMap<Defined, NaiveDate> {
    let mut oldest: BTreeMap<Defined, NaiveDate> = BTreeMap::new();
    for record in log.split(LOG_RECORD_START) {
        let mut lines = record.lines();
        let Some(date) = lines
            .next()
            .and_then(|line| NaiveDate::parse_from_str(line.trim(), LOG_DATE_FORMAT).ok())
        else {
            continue;
        };
        let mut in_hunk = false;
        for line in lines {
            if line.starts_with(FILE_HEADER) {
                in_hunk = false;
            } else if line.starts_with(HUNK_HEADER) {
                in_hunk = true;
            } else if in_hunk {
                let added = line.strip_prefix(ADDED_MARK).unwrap_or_default();
                for defined in defined_names(added) {
                    oldest
                        .entry(defined)
                        .and_modify(|known| *known = (*known).min(date))
                        .or_insert(date);
                }
            }
        }
    }
    oldest
}
