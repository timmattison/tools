//! The added date of a definition: the date of the oldest commit that adds a
//! line that defines its name to its origin file.
//!
//! [`defined_name`] reads one line of a shell file and gives the name that the
//! line defines. [`added_dates`] reads the history of one file, with its full
//! diff, and gives the oldest date for each name that an added line defines.
//! Both functions are pure.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::catalog::{LOG_DATE_FORMAT, LOG_RECORD_START};
use crate::command_words::SUFFIX_MARKER;

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

/// The characters that the shell removes from the name of an alias: the two
/// quotes and the backslash.
const QUOTE_CHARACTERS: [char; 3] = ['\'', '"', '\\'];

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

/// Gives the name that `line` defines, or `None`. Pure.
///
/// Blanks at the start of the line do not count. An alias line starts with
/// the word `alias`, then option words such as `-g` and `-s`, then `name=`.
/// The option `-s` makes the name a suffix alias, `*.` and the suffix. A word
/// that starts with `-` and holds no `=` is an option word, and `--` ends the
/// options. The quotes and the backslashes in the name do not count, thus
/// `alias 'll=ls -l'` defines `ll`. Only the first name of the line counts.
///
/// A function line is `name()`, `name ()`, or `function name`. A function
/// name holds no blank, no quote, no `$`, and no operator of the shell. A
/// comment line defines nothing, because `#` is not in a name.
#[must_use]
pub fn defined_name(line: &str) -> Option<Defined> {
    let text = line.trim_start();
    if let Some(rest) = after_word(text, ALIAS_WORD) {
        return alias_name(rest);
    }
    if let Some(rest) = after_word(text, FUNCTION_WORD) {
        return keyword_function_name(rest);
    }
    parenthesized_function_name(text)
}

/// Gives the text after `word` when `text` starts with `word` and a blank
/// follows it.
fn after_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(word)?;
    rest.starts_with(char::is_whitespace).then_some(rest)
}

/// Gives the alias that the words after `alias` define.
fn alias_name(rest: &str) -> Option<Defined> {
    let mut suffix = false;
    let mut words = rest.split_whitespace();
    let word = loop {
        let word = words.next()?;
        if word == END_OF_OPTIONS {
            break words.next()?;
        }
        if word.starts_with(OPTION_START) && !word.contains(ASSIGNMENT) {
            suffix |= word.contains(SUFFIX_OPTION);
            continue;
        }
        break word;
    };
    let (name, _) = word.split_once(ASSIGNMENT)?;
    let name: String = name
        .chars()
        .filter(|character| !QUOTE_CHARACTERS.contains(character))
        .collect();
    if name.is_empty() {
        return None;
    }
    Some(if suffix {
        Defined::alias(format!("{SUFFIX_MARKER}{name}"))
    } else {
        Defined::alias(name)
    })
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
                if let Some(defined) = line.strip_prefix(ADDED_MARK).and_then(defined_name) {
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
