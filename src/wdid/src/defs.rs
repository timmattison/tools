//! The count of the definitions: counts the uses of each alias and each
//! function of the shell.
//!
//! [`tally`] reads the history rows and the definitions of the shell. It gives
//! one [`Usage`] for each alias and for each function. It counts a typed use
//! and a use through another definition. It finds each definition that the
//! shell runs without a typed name, with its [`Reason`]. It classifies each
//! other definition by the date of its last use. It also gives each
//! [`Finding`]: the duplicates and the shadows among the definitions. The
//! function is pure: it starts no process and reads no file.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use chrono::NaiveDate;

use crate::added::AddedDates;
use crate::command_words::{parse_line, AliasKind, CommandWord, Line};
use crate::history::HistoryEntry;
use crate::origin::{Origin, Places};
use crate::shell::Definitions;
use crate::starts::Trigger;
use crate::tally::days_since;

mod findings;
mod reasons;
pub mod render;

pub use findings::{Finding, FindingKind, Member};

/// The label of a regular alias.
const LABEL_ALIAS: &str = "alias";
/// The label of a global alias.
const LABEL_GLOBAL_ALIAS: &str = "global alias";
/// The label of a suffix alias.
const LABEL_SUFFIX_ALIAS: &str = "suffix alias";
/// The label of a function.
const LABEL_FUNCTION: &str = "function";
/// The first character of the name of a private function. The report lists a
/// private function only with `all` (see [`Window::all`]).
const PRIVATE_PREFIX: char = '_';

/// The days that the count reads, and the idle limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// The date of the run. Every age counts back from this date.
    pub today: NaiveDate,
    /// The first date that counts. A row on an earlier date does not count.
    pub since: Option<NaiveDate>,
    /// A definition that never ran is new when its added date is younger than
    /// this many days.
    pub new_days: u32,
    /// A definition whose last use is older than this many days is idle.
    pub idle_days: u32,
    /// The report lists each definition: the functions whose name starts
    /// with `_` too, and in the text report the definitions that are not
    /// mine.
    pub all: bool,
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
    /// The definition never ran, and its added date is unknown or not
    /// younger than `new_days` days.
    NeverUsedOld,
    /// The definition never ran, and its added date is younger than
    /// `new_days` days.
    NeverUsedNew,
    /// The shell runs the definition without a typed name, for example at
    /// each prompt. The state does not depend on the uses.
    RunsWithoutTyping,
}

/// Why the shell runs a definition without a typed name, or why it does not
/// run a widget function.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    /// `trigger` runs the definition.
    Runs {
        /// The event of the shell.
        trigger: Trigger,
        /// The definition that `trigger` runs, when `trigger` reaches this
        /// definition through a call. `None` when `trigger` runs it directly.
        through: Option<String>,
    },
    /// The function implements a widget, and no key starts the widget. Only a
    /// typed `zle` call or `execute-named-cmd` starts it.
    WidgetWithoutKey,
}

impl Reason {
    /// Says if the shell runs the definition without a typed name for this
    /// reason. A widget that no key starts does not run.
    #[must_use]
    pub const fn runs_without_typing(&self) -> bool {
        matches!(self, Self::Runs { .. })
    }
}

impl fmt::Display for Reason {
    /// Writes the text of the reason that a report shows: the text of the
    /// trigger, then `, through` and the name when the trigger reaches the
    /// definition through a call. A widget that no key starts has the text
    /// `widget, no key`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runs {
                trigger,
                through: None,
            } => write!(formatter, "{trigger}"),
            Self::Runs {
                trigger,
                through: Some(name),
            } => write!(formatter, "{trigger}, through {name}"),
            Self::WidgetWithoutKey => formatter.write_str("widget, no key"),
        }
    }
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
    /// The file that defined the definition, or `None` when it is unknown.
    pub origin: Option<Origin>,
    /// True when the definition is mine (see [`Places::is_mine`]).
    pub mine: bool,
    /// The date of the oldest commit that adds a line that defines the name
    /// to the file of the origin (see [`crate::added`]). `None` when it is
    /// unknown, and for each definition that is not mine.
    pub added: Option<NaiveDate>,
    /// Why the shell runs the definition without a typed name, sorted. It is
    /// empty for most definitions.
    pub reasons: Vec<Reason>,
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
    /// The entries of the report that are mine.
    pub mine: usize,
    /// The entries of the report that are not mine.
    pub others: usize,
}

/// The result of the count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tally {
    /// One entry for each alias and each function. The most used is first.
    pub definitions: Vec<Usage>,
    /// The counts about the history and about the shell.
    pub summary: Summary,
    /// The directories that decide which definitions are mine. The text
    /// report shows the home directory as `~`.
    pub places: Places,
    /// The duplicates and the shadows among the entries, in report order. It
    /// holds each finding, also a finding with no definition of mine.
    pub findings: Vec<Finding>,
}

impl Tally {
    /// Gives the file of the origin of each of my definitions.
    #[must_use]
    pub fn my_origin_files(&self) -> BTreeSet<PathBuf> {
        BTreeSet::new()
    }

    /// Gives each of my definitions its added date from `dates`.
    pub fn apply_added_dates(&mut self, _dates: &AddedDates, _window: &Window) {}
}

/// Gives the first line of `text`, with its other characters unchanged.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

/// Makes one entry with no use for each alias and each function that the
/// report lists, with its origin. A function whose name starts with `_` is an
/// entry only when `all` is true.
fn empty_usages(definitions: &Definitions, all: bool) -> Vec<Usage> {
    let aliases = definitions.aliases.definitions().into_iter().map(|alias| {
        let origin = definitions.alias_origin(&alias).cloned();
        let definition = first_line(&alias.value).to_string();
        (alias.name, Kind::from(alias.kind), definition, origin)
    });
    let functions = definitions
        .functions
        .keys()
        .chain(definitions.unparsed_functions.iter())
        .filter(|name| all || !name.starts_with(PRIVATE_PREFIX))
        .map(|name| {
            let line = definitions.function_lines.get(name).cloned();
            let origin = definitions.function_origin(name).cloned();
            (
                name.clone(),
                Kind::Function,
                line.unwrap_or_default(),
                origin,
            )
        });
    aliases
        .chain(functions)
        .map(|(name, kind, definition, origin)| Usage {
            name,
            kind,
            definition,
            uses: 0,
            typed: 0,
            through: Vec::new(),
            first_use: None,
            last_use: None,
            classification: Classification::NeverUsedOld,
            mine: definitions.places.is_mine(origin.as_ref()),
            added: None,
            origin,
            reasons: Vec::new(),
        })
        .collect()
}

/// The counts of one definition, before they become a [`Usage`].
#[derive(Debug, Default)]
struct Counts {
    typed: u64,
    through: BTreeMap<String, u64>,
    first_use: Option<NaiveDate>,
    last_use: Option<NaiveDate>,
}

impl Counts {
    /// Counts one use on `date`. `via` is the name that the person typed when
    /// the use went through another definition.
    fn record_use(&mut self, via: Option<&str>, date: NaiveDate) {
        match via {
            Some(name) => *self.through.entry(name.to_string()).or_insert(0) += 1,
            None => self.typed += 1,
        }
        self.first_use = Some(self.first_use.map_or(date, |first| first.min(date)));
        self.last_use = Some(self.last_use.map_or(date, |last| last.max(date)));
    }
}

/// The names of the definitions of the shell, which each walk needs.
struct Names<'a> {
    /// The kind of each alias, by name.
    alias_kinds: HashMap<String, Kind>,
    /// The names of all functions, also the private ones and the unparsed ones.
    functions: HashSet<&'a str>,
}

impl<'a> Names<'a> {
    /// Gives the names of `definitions`.
    fn new(definitions: &'a Definitions) -> Self {
        let alias_kinds = definitions
            .aliases
            .definitions()
            .into_iter()
            .map(|alias| (alias.name, Kind::from(alias.kind)))
            .collect();
        let functions = definitions
            .functions
            .keys()
            .chain(definitions.unparsed_functions.iter())
            .map(String::as_str)
            .collect();
        Self {
            alias_kinds,
            functions,
        }
    }
}

/// Gives each function that the body of the function `start` calls, to any
/// depth, one time each, in walk order. The walk visits each function one
/// time, thus a cycle ends. `start` is not in the list, also when a cycle
/// calls it.
fn callees<'a>(definitions: &'a Definitions, names: &Names<'_>, start: &str) -> Vec<&'a str> {
    let mut visited: HashSet<&str> = HashSet::from([start]);
    let mut pending: Vec<&str> = vec![start];
    let mut found = Vec::new();
    while let Some(current) = pending.pop() {
        let Some(body) = definitions.functions.get(current) else {
            continue;
        };
        for callee in body.iter().map(|call| call.word.as_str()) {
            if names.functions.contains(callee) && visited.insert(callee) {
                found.push(callee);
                pending.push(callee);
            }
        }
    }
    found
}

/// The counts of every entry, by name and kind, with the names that the walk
/// needs.
struct Counter<'a> {
    definitions: &'a Definitions,
    names: Names<'a>,
    counts: HashMap<(String, Kind), Counts>,
}

impl<'a> Counter<'a> {
    /// Makes a counter with one empty count for each entry of `usages`.
    fn new(definitions: &'a Definitions, usages: &[Usage]) -> Self {
        let counts = usages
            .iter()
            .map(|usage| ((usage.name.clone(), usage.kind), Counts::default()))
            .collect();
        Self {
            definitions,
            names: Names::new(definitions),
            counts,
        }
    }

    /// Counts one use of the function `name`. A private function has no entry,
    /// thus its use is dropped.
    fn use_function(&mut self, name: &str, via: Option<&str>, date: NaiveDate) {
        if let Some(entry) = self.counts.get_mut(&(name.to_string(), Kind::Function)) {
            entry.record_use(via, date);
        }
    }

    /// Counts the uses that one parsed line makes on `date`.
    fn count_line(&mut self, line: &Line, date: NaiveDate) {
        for alias_use in &line.aliases {
            let Some(kind) = self.names.alias_kinds.get(&alias_use.name).copied() else {
                continue;
            };
            if let Some(entry) = self.counts.get_mut(&(alias_use.name.clone(), kind)) {
                entry.record_use(alias_use.through.as_deref(), date);
            }
        }
        for word in &line.words {
            self.count_word(word, date);
        }
    }

    /// Counts the use of the function that one command word names, and the use
    /// of each function that its body calls, to any depth. A function counts
    /// once for each word, and the start of the walk never counts itself.
    fn count_word(&mut self, word: &CommandWord, date: NaiveDate) {
        let name = word.word.as_str();
        if !self.names.functions.contains(name) {
            return;
        }
        self.use_function(name, word.alias.as_deref(), date);
        let via = word.alias.as_deref().unwrap_or(name);
        for callee in callees(self.definitions, &self.names, name) {
            self.use_function(callee, Some(via), date);
        }
    }
}

/// Classifies one definition. A definition that the shell runs without a
/// typed name has that state, whatever its uses. A definition with no use
/// never ran. A definition whose last use is older than `idle_days` days is
/// idle.
fn classify(
    reasons: &[Reason],
    uses: u64,
    last_use: Option<NaiveDate>,
    window: &Window,
) -> Classification {
    if reasons.iter().any(Reason::runs_without_typing) {
        return Classification::RunsWithoutTyping;
    }
    match last_use {
        Some(last) if uses > 0 => {
            if days_since(window.today, last) > i64::from(window.idle_days) {
                Classification::Idle
            } else {
                Classification::Used
            }
        }
        _ => Classification::NeverUsedOld,
    }
}

/// Counts the uses of each alias and each function in `entries`. Pure.
///
/// Each entry gets the origin of its definition, and says if it is mine. A
/// function whose name starts with `_` is an entry only with `window.all`. A
/// call through such a function counts in each case.
///
/// The window drops the rows before `window.since`. A row with exit code 127
/// does not count, because the shell did not find the command. An alias in the
/// body of a function does not count, because the shell expanded it when it
/// read the function.
///
/// Each entry also gets the reasons that the shell runs it without a typed
/// name (see [`Definitions::starts`]). A reason goes to each function that a
/// hook, a trap, or a widget with a key runs, to each function that such a
/// function calls, to any depth, and to each alias that the code of a bash
/// start names. Such an entry has the state
/// [`Classification::RunsWithoutTyping`].
#[must_use]
pub fn tally(entries: &[HistoryEntry], definitions: &Definitions, window: &Window) -> Tally {
    let mut usages = empty_usages(definitions, window.all);
    let mut counter = Counter::new(definitions, &usages);
    let mut reasons = reasons::find(definitions, &counter.names);
    let mut cache: HashMap<&str, Option<Line>> = HashMap::new();
    let mut before_since: u64 = 0;
    let mut not_found: u64 = 0;
    let mut unparsed_rows: u64 = 0;
    for entry in entries {
        if window.since.is_some_and(|since| entry.time.date() < since) {
            before_since += 1;
            continue;
        }
        if entry.exit.is_command_not_found() {
            not_found += 1;
            continue;
        }
        let parsed = cache
            .entry(entry.command.as_str())
            .or_insert_with(|| parse_line(&entry.command, &definitions.aliases));
        match parsed {
            Some(line) => counter.count_line(line, entry.time.date()),
            None => unparsed_rows += 1,
        }
    }
    for usage in &mut usages {
        let counts = counter
            .counts
            .remove(&(usage.name.clone(), usage.kind))
            .unwrap_or_default();
        let mut through: Vec<(String, u64)> = counts.through.into_iter().collect();
        through.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        let through_total: u64 = through.iter().map(|(_, count)| count).sum();
        usage.typed = counts.typed;
        usage.uses = counts.typed + through_total;
        usage.through = through;
        usage.first_use = counts.first_use;
        usage.last_use = counts.last_use;
        usage.reasons = reasons
            .remove(&(usage.name.clone(), usage.kind))
            .map(Vec::from_iter)
            .unwrap_or_default();
        usage.classification = classify(&usage.reasons, usage.uses, counts.last_use, window);
    }
    usages.sort_by(|left, right| {
        right
            .uses
            .cmp(&left.uses)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    let aliases = usages.iter().filter(|u| u.kind != Kind::Function).count();
    let functions = usages.iter().filter(|u| u.kind == Kind::Function).count();
    let unparsed_functions = usages
        .iter()
        .filter(|u| u.kind == Kind::Function && definitions.unparsed_functions.contains(&u.name))
        .count();
    let mine = usages.iter().filter(|u| u.mine).count();
    let usages_count = usages.len();
    let findings = findings::find(definitions, &usages);
    Tally {
        definitions: usages,
        summary: Summary {
            rows: u64::try_from(entries.len()).unwrap_or(u64::MAX),
            before_since,
            not_found,
            unparsed_rows,
            aliases,
            functions,
            unparsed_functions,
            unparsed_aliases: definitions.unparsed_aliases.len(),
            mine,
            others: usages_count.saturating_sub(mine),
        },
        places: definitions.places.clone(),
        findings,
    }
}
