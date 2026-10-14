//! The tally: counts the uses of each tool.
//!
//! [`tally`] reads the history rows, parses each distinct command once, and
//! counts every tool of the catalog. It counts a typed use, a use through an
//! alias, a use through a function, and a run of a development build. It also
//! classifies each tool by its age and by the date of its last use. The
//! function is pure: it starts no process and reads no file.

use std::collections::{BTreeMap, HashMap};

use chrono::NaiveDate;

use crate::catalog::{Catalog, Tool};
use crate::command_words::{command_words, CommandWord, Parsed};
use crate::history::HistoryEntry;
use crate::rules::{runs, Run};
use crate::shell::{Definitions, Resolver};
use crate::tool::{ToolName, ToolNames};

/// The days that the tally counts from, and the two age limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// The date of the run. Every age counts back from this date.
    pub today: NaiveDate,
    /// The first date that counts. A row on an earlier date does not count.
    pub since: Option<NaiveDate>,
    /// A tool that never ran and that is younger than this many days is new.
    pub new_days: u32,
    /// A tool whose last use is older than this many days is idle.
    pub idle_days: u32,
}

/// The state of one tool in the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// The tool ran in the last `idle_days` days.
    Used,
    /// The tool ran, but not in the last `idle_days` days.
    Idle,
    /// The tool never ran, and it is younger than `new_days` days.
    NeverUsedNew,
    /// The tool never ran, and it is not younger than `new_days` days.
    NeverUsedOld,
}

/// The uses of one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolUsage {
    /// The tool.
    pub tool: Tool,
    /// The typed uses plus every use through an alias or a function.
    pub uses: u64,
    /// The uses where the person typed the name of the tool.
    pub typed: u64,
    /// The uses through an alias or a function, by the name that the person
    /// typed. The most used name is first. Equal counts sort by name.
    pub through: Vec<(String, u64)>,
    /// The runs of a development build of the tool.
    pub development_runs: u64,
    /// The date of the first counted use.
    pub first_use: Option<NaiveDate>,
    /// The date of the last counted use.
    pub last_use: Option<NaiveDate>,
    /// The state of the tool.
    pub classification: Classification,
}

/// A command that the parser refused, with the rows that hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unparsed {
    /// The text of the command.
    pub command: String,
    /// The number of rows that hold this text.
    pub rows: u64,
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
    /// The distinct commands that did not parse. The command with the most
    /// rows is first. Equal counts sort by command.
    pub unparsed: Vec<Unparsed>,
    /// The number of aliases.
    pub aliases: usize,
    /// The number of functions, parsed and unparsed.
    pub functions: usize,
    /// The names of the functions whose body did not parse.
    pub unparsed_functions: Vec<String>,
    /// The number of alias lines that did not parse.
    pub unparsed_aliases: usize,
}

/// The result of the tally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tally {
    /// One entry for each tool of the catalog. The most used tool is first.
    pub tools: Vec<ToolUsage>,
    /// The counts about the history and about the shell.
    pub summary: Summary,
}

/// The counts of one tool, before they become a [`ToolUsage`].
#[derive(Debug, Default)]
struct Counts {
    typed: u64,
    through: BTreeMap<String, u64>,
    development_runs: u64,
    first_use: Option<NaiveDate>,
    last_use: Option<NaiveDate>,
}

impl Counts {
    /// Counts one use on `date`. `via` is the name that the person typed when
    /// the use went through an alias or a function.
    fn record_use(&mut self, via: Option<&str>, date: NaiveDate) {
        match via {
            Some(name) => *self.through.entry(name.to_string()).or_insert(0) += 1,
            None => self.typed += 1,
        }
        self.first_use = Some(self.first_use.map_or(date, |first| first.min(date)));
        self.last_use = Some(self.last_use.map_or(date, |last| last.max(date)));
    }

    /// Gives the uses through aliases and functions, most used first.
    fn sorted_through(&self) -> Vec<(String, u64)> {
        let mut through: Vec<(String, u64)> = self
            .through
            .iter()
            .map(|(name, count)| (name.clone(), *count))
            .collect();
        through.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        through
    }
}

/// Counts the runs of one command word on `date`.
fn count_word(
    word: &CommandWord,
    date: NaiveDate,
    names: &ToolNames,
    resolver: &Resolver,
    counts: &mut BTreeMap<ToolName, Counts>,
) {
    let mut counted: Vec<ToolName> = Vec::new();
    for run in runs(word, names) {
        match run {
            Run::Use(tool) => {
                if let Some(entry) = counts.get_mut(&tool) {
                    entry.record_use(word.alias.as_deref(), date);
                }
                counted.push(tool);
            }
            Run::Development(tool) => {
                if let Some(entry) = counts.get_mut(&tool) {
                    entry.development_runs += 1;
                }
            }
        }
    }
    if let Some(set) = resolver.tools_of(&word.word) {
        let via = word.alias.as_deref().unwrap_or(&word.word);
        for tool in set.iter().filter(|tool| !counted.contains(tool)) {
            if let Some(entry) = counts.get_mut(tool) {
                entry.record_use(Some(via), date);
            }
        }
    }
}

/// Counts every tool of `catalog` in `entries`. Pure.
///
/// The window drops the rows before `window.since`. A row with exit code 127
/// does not count, because the shell did not find the command. A row with
/// exit code -1 counts.
#[must_use]
pub fn tally(
    entries: &[HistoryEntry],
    definitions: &Definitions,
    resolver: &Resolver,
    catalog: &Catalog,
    window: &Window,
) -> Tally {
    let names = catalog.names();
    let mut counts: BTreeMap<ToolName, Counts> = catalog
        .tools
        .iter()
        .map(|tool| (tool.name.clone(), Counts::default()))
        .collect();
    let mut cache: HashMap<&str, Parsed> = HashMap::new();
    let mut before_since: u64 = 0;
    let mut not_found: u64 = 0;
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
            .or_insert_with(|| command_words(&entry.command, &definitions.aliases));
        if let Parsed::Words(words) = parsed {
            for word in words.iter() {
                count_word(word, entry.time.date(), &names, resolver, &mut counts);
            }
        }
    }
    let tools = catalog
        .tools
        .iter()
        .map(|tool| {
            let tool_counts = counts.remove(&tool.name).unwrap_or_default();
            let through = tool_counts.sorted_through();
            let through_total: u64 = through.iter().map(|(_, count)| count).sum();
            ToolUsage {
                tool: tool.clone(),
                uses: tool_counts.typed + through_total,
                typed: tool_counts.typed,
                through,
                development_runs: tool_counts.development_runs,
                first_use: tool_counts.first_use,
                last_use: tool_counts.last_use,
                classification: Classification::NeverUsedOld,
            }
        })
        .collect();
    Tally {
        tools,
        summary: Summary {
            rows: 0,
            before_since,
            not_found,
            unparsed_rows: 0,
            unparsed: Vec::new(),
            aliases: 0,
            functions: 0,
            unparsed_functions: Vec::new(),
            unparsed_aliases: 0,
        },
    }
}
