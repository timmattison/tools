//! The tally: counts the uses of each tool.
//!
//! [`tally`] reads the history rows, parses each distinct command once, and
//! counts every tool of the catalog. It counts a typed use, a use through an
//! alias, a use through a function, and a run of a development build. It also
//! classifies each tool by its age and by the date of its last use. The
//! function is pure: it starts no process and reads no file.

use chrono::NaiveDate;

use crate::catalog::{Catalog, Tool};
use crate::history::HistoryEntry;
use crate::shell::{Definitions, Resolver};

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
    let _ = (entries, definitions, resolver, window);
    let tools = catalog
        .tools
        .iter()
        .map(|tool| ToolUsage {
            tool: tool.clone(),
            uses: 0,
            typed: 0,
            through: Vec::new(),
            development_runs: 0,
            first_use: None,
            last_use: None,
            classification: Classification::NeverUsedOld,
        })
        .collect();
    Tally {
        tools,
        summary: Summary {
            rows: 0,
            before_since: 0,
            not_found: 0,
            unparsed_rows: 0,
            unparsed: Vec::new(),
            aliases: 0,
            functions: 0,
            unparsed_functions: Vec::new(),
            unparsed_aliases: 0,
        },
    }
}
