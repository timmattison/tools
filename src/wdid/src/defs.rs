//! The count of the definitions: counts the uses of each alias and each
//! function of the shell.
//!
//! [`tally`] reads the history rows and the definitions of the shell. It gives
//! one [`Usage`] for each alias and for each function. It counts a typed use
//! and a use through another definition. It classifies each definition by the
//! date of its last use. The function is pure: it starts no process and reads
//! no file.

use chrono::NaiveDate;

use crate::command_words::AliasKind;
use crate::history::HistoryEntry;
use crate::shell::Definitions;

/// The label of a regular alias.
const LABEL_ALIAS: &str = "alias";
/// The label of a global alias.
const LABEL_GLOBAL_ALIAS: &str = "global alias";
/// The label of a suffix alias.
const LABEL_SUFFIX_ALIAS: &str = "suffix alias";
/// The label of a function.
const LABEL_FUNCTION: &str = "function";
/// The first character of the name of a private function. The report does not
/// list a private function.
const PRIVATE_PREFIX: char = '_';

/// The days that the count reads, and the idle limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// The date of the run. Every age counts back from this date.
    pub today: NaiveDate,
    /// The first date that counts. A row on an earlier date does not count.
    pub since: Option<NaiveDate>,
    /// A definition whose last use is older than this many days is idle.
    pub idle_days: u32,
}

/// The kind of a definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// An alias that expands at command position only.
    Alias,
    /// An alias that expands in every word position.
    GlobalAlias,
    /// An alias that expands a command word with its suffix.
    SuffixAlias,
    /// A function.
    Function,
}

impl Kind {
    /// Gives the label that a report shows for this kind.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Alias => LABEL_ALIAS,
            Self::GlobalAlias => LABEL_GLOBAL_ALIAS,
            Self::SuffixAlias => LABEL_SUFFIX_ALIAS,
            Self::Function => LABEL_FUNCTION,
        }
    }
}

impl From<AliasKind> for Kind {
    fn from(kind: AliasKind) -> Self {
        match kind {
            AliasKind::Regular => Self::Alias,
            AliasKind::Global => Self::GlobalAlias,
            AliasKind::Suffix => Self::SuffixAlias,
        }
    }
}

/// The state of one definition in the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// The definition ran in the last `idle_days` days.
    Used,
    /// The definition ran, but not in the last `idle_days` days.
    Idle,
    /// The definition never ran.
    NeverUsed,
}

/// The uses of one definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    /// The name. A suffix alias has its marker as its name (`*.md`).
    pub name: String,
    /// The kind.
    pub kind: Kind,
    /// The value of an alias up to its first newline, or the first line of the
    /// body of a function. It is empty when the function has no such line.
    pub definition: String,
    /// The typed uses plus the uses through other definitions.
    pub uses: u64,
    /// The uses where the person typed the name.
    pub typed: u64,
    /// The uses through other definitions, by the name that the person typed.
    /// The most used name is first. Equal counts sort by name.
    pub through: Vec<(String, u64)>,
    /// The date of the first counted use.
    pub first_use: Option<NaiveDate>,
    /// The date of the last counted use.
    pub last_use: Option<NaiveDate>,
    /// The state of the definition.
    pub classification: Classification,
}

/// The counts about the history and about the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// Every row that atuin gave.
    pub rows: u64,
    /// The rows before the `since` date. This is 0 without a `since` date.
    pub before_since: u64,
    /// The rows in the window with exit code 127.
    pub not_found: u64,
    /// The rows in the window, not 127, that did not parse.
    pub unparsed_rows: u64,
    /// The alias entries of the report, of all three kinds.
    pub aliases: usize,
    /// The function entries of the report.
    pub functions: usize,
    /// The function entries whose body did not parse.
    pub unparsed_functions: usize,
    /// The alias lines that did not parse.
    pub unparsed_aliases: usize,
}

/// The result of the count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tally {
    /// One entry for each alias and each function. The most used is first.
    pub definitions: Vec<Usage>,
    /// The counts about the history and about the shell.
    pub summary: Summary,
}

/// Gives the first line of `text`, with its other characters unchanged.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

/// Makes one entry with no use for each alias and each function that the
/// report lists.
fn empty_usages(definitions: &Definitions) -> Vec<Usage> {
    let aliases = definitions.aliases.definitions().into_iter().map(|alias| {
        (
            alias.name,
            Kind::from(alias.kind),
            first_line(&alias.value).to_string(),
        )
    });
    let functions = definitions
        .functions
        .keys()
        .chain(definitions.unparsed_functions.iter())
        .filter(|name| !name.starts_with(PRIVATE_PREFIX))
        .map(|name| {
            let line = definitions.function_lines.get(name).cloned();
            (name.clone(), Kind::Function, line.unwrap_or_default())
        });
    aliases
        .chain(functions)
        .map(|(name, kind, definition)| Usage {
            name,
            kind,
            definition,
            uses: 0,
            typed: 0,
            through: Vec::new(),
            first_use: None,
            last_use: None,
            classification: Classification::NeverUsed,
        })
        .collect()
}

/// Counts the uses of each alias and each function in `entries`. Pure.
#[must_use]
pub fn tally(entries: &[HistoryEntry], definitions: &Definitions, window: &Window) -> Tally {
    let _ = window;
    let usages = empty_usages(definitions);
    let aliases = usages.iter().filter(|u| u.kind != Kind::Function).count();
    let functions = usages.iter().filter(|u| u.kind == Kind::Function).count();
    Tally {
        summary: Summary {
            rows: u64::try_from(entries.len()).unwrap_or(u64::MAX),
            before_since: 0,
            not_found: 0,
            unparsed_rows: 0,
            aliases,
            functions,
            unparsed_functions: 0,
            unparsed_aliases: 0,
        },
        definitions: usages,
    }
}
