//! Reading a plan of parallel work out of a page of text.
//!
//! A chain says one order: do these issues, one after the other. A plan says
//! several orders side by side, one for each part of the repository that a
//! reader can work in without walking into the work of a neighbor. Such a plan
//! is written as a record for each stream, or as one table row for each stream.
//! This module reads both, and it gives back the same streams either way.
//!
//! # Why only two fields hold numbers
//!
//! A stream carries prose as well as a chain. The prose is about code, and
//! prose about code is full of numbers: `main.rs:1566-1650` names two lines of
//! a file, and `265 lines apart in a 5113-line file` names a distance and a
//! length. None of them is an issue.
//!
//! So this module reads the `Order` field and the `Waits for` field, and it
//! reads nothing else. `Stream`, `Zone`, and `Notes` never give a number to a
//! chain. A reader who writes a note about line 5113 gets the issues of the
//! plan, and not issue 5113.
//!
//! The `Waits for` field is as safe to read as the `Order` field, because a
//! reader writes steps in it and nothing else: it names the work of other
//! streams that comes before the first step of this one. The reason a stream
//! waits is prose, and the prose of a stream belongs in `Notes`.
//!
//! The two fields are read the same way and they mean different things.
//! `Order` is a chain, so the order of its steps is the order of the work.
//! `Waits for` is a set, so `#96 → #91` and `#96, #91` say the same thing:
//! both pieces of work come first, and the stream starts when both are
//! finished.
//!
//! # The pair
//!
//! A step of a plan is one piece of work, and one piece of work is sometimes
//! two numbers: `PR#344 (#341)` is a pull request that closes an issue. The
//! step holds both, because the state of the work is the state of the pull
//! request and the reader still wants to see which issue it finishes.
//!
//! A plan writes the same pair the other way round as well: `#4 (in flight,
//! PR #15)` is the issue `#4`, whose work is the pull request `#15`. So a
//! group in parentheses annotates the step to its left, and it never opens
//! one. Inside a group, only a word that carries the `#` is a number, and the
//! `PR` in front of one marks that number as the work. Every other word is
//! prose the reader drops, which is what lets `#12 (human)` and `#4 (30-line
//! window)` each hold one number.
//!
//! The prose of a group holds a parenthesis as readily as a word does, so a
//! group counts its depth: `#4 (a note (see the docs))` is one group and not
//! two, and the parenthesis that brings the depth to zero is the one that
//! closes it.

use thiserror::Error;

use crate::chain::{read_number, IssueNumber, Snippet, SEPARATORS};

/// The character that marks a number as an issue number.
///
/// The same mark as the one the chain reader uses. It stands here as well
/// because a step ends where the next `#` starts, so `#1#2` is two steps
/// written with no separator at all.
const HASH: char = '#';

/// The character that stands between the key of a field and its text.
const FIELD_COLON: char = ':';

/// The characters that stand between two cells of a table.
///
/// The ASCII bar, and the two the box-drawing block writes. A row is split on
/// these and never on a column position: a split by position needs the display
/// width of every character in front of the column, and an em dash or a
/// Japanese character inside one cell would then shift every cell after it.
const TABLE_BARS: &[char] = &[
    '|',        // the ASCII spelling
    '\u{2502}', // │ BOX DRAWINGS LIGHT VERTICAL
    '\u{2503}', // ┃ BOX DRAWINGS HEAVY VERTICAL
];

/// The character that opens the group of a pair.
const GROUP_OPEN: char = '(';

/// The character that closes the group of a pair.
const GROUP_CLOSE: char = ')';

/// The prefix a plan writes before the number of a pull request.
///
/// It marks which number of a pair is the work. GitHub numbers a pull request
/// out of the same series as an issue, so the mark is the only thing that says
/// `PR#344 (#341)` and `#4 (in flight, PR #15)` name the work in opposite
/// places.
const PULL_REQUEST_PREFIX: &str = "pr";

/// The word a stream with no `Stream` field takes as the first half of its
/// label. The second half is the place of the stream in the plan.
const UNNAMED_LABEL: &str = "Stream";

/// The lowest number of marks a line of rule holds.
///
/// Two marks are an arrow (`--`) or the start of a word. Three are a rule. The
/// bar of a cell and the space around it are no marks, so they add nothing to
/// this count.
const RULE_CHARS: usize = 3;

/// The marks a Markdown divider is drawn with.
///
/// The hyphen draws the line, and the colon names the alignment of a column.
/// A divider carries these two and no other mark of [`is_rule_mark`], which is
/// what parts it from the `├─────┼─────┤` of a box table and the `+-----+`
/// of an ASCII one.
const MARKDOWN_DIVIDER_MARKS: &[char] = &['-', ':'];

/// One piece of work of a stream.
///
/// A step is one number, and sometimes two: a pull request and the issue that
/// pull request closes. The two travel together because a reader who reads
/// `PR#344 (#341)` wants one row and not two, and because the state of the row
/// is the state of the pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// The number of the work itself: the pull request of a pair.
    number: IssueNumber,
    /// The issue the work closes, when the plan names one.
    closes: Option<IssueNumber>,
}

impl Step {
    /// The step that does the work `number` names and closes `closes`.
    #[must_use]
    pub fn new(number: IssueNumber, closes: Option<IssueNumber>) -> Self {
        Self { number, closes }
    }

    /// The number of the work: the pull request of a pair, and the issue of a
    /// step that stands alone.
    #[must_use]
    pub fn number(&self) -> IssueNumber {
        self.number
    }

    /// The issue the work closes, or `None` when the plan names one number
    /// only.
    #[must_use]
    pub fn closes(&self) -> Option<IssueNumber> {
        self.closes
    }
}

/// One line of work of a plan, from its first step to its last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    /// The name the plan gives the stream, or the place of the stream in the
    /// plan when it gives none.
    label: String,
    /// The steps of the stream, in the order the plan writes them.
    steps: Vec<Step>,
    /// The steps of other streams that come before the first step of this
    /// one, in the order the plan writes them.
    ///
    /// A set and not a chain. The plan names the work that finishes first, and
    /// the order it names that work in says nothing: a stream that waits for
    /// two pieces of work waits for both of them.
    waits_for: Vec<Step>,
}

impl Stream {
    /// The name of the stream, as the plan writes it.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The steps of the stream, in the order the plan writes them.
    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// The steps that come before the first step of the stream, in the order
    /// the plan writes them.
    ///
    /// [`crate::graph::of_plan`] reads them, because a plan that names what a
    /// stream waits for is a graph.
    #[must_use]
    pub fn waits_for(&self) -> &[Step] {
        &self.waits_for
    }
}

/// Several streams of work, to walk side by side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The streams, in the order the plan writes them.
    streams: Vec<Stream>,
}

impl Plan {
    /// The streams of the plan, in the order the plan writes them.
    #[must_use]
    pub fn streams(&self) -> &[Stream] {
        &self.streams
    }

    /// Every number of every stream, in the order of its first appearance.
    ///
    /// The number of a step comes before the number the step closes, because
    /// the pull request is the work and the issue is what the work finishes.
    /// A number that stands in two streams arrives once, so one query to
    /// GitHub answers the whole plan.
    ///
    /// The chain of a stream comes first and the work it waits for after it.
    /// The work a stream waits for is sometimes the work of no stream of the
    /// plan, and it stands here all the same: a reader who is told to wait is
    /// owed the state of the work they wait for.
    #[must_use]
    pub fn numbers(&self) -> Vec<IssueNumber> {
        let mut numbers: Vec<IssueNumber> = Vec::new();
        for step in self
            .streams
            .iter()
            .flat_map(|stream| stream.steps.iter().chain(&stream.waits_for))
        {
            for number in [Some(step.number), step.closes].into_iter().flatten() {
                if !numbers.contains(&number) {
                    numbers.push(number);
                }
            }
        }
        numbers
    }
}

/// Why a page of text is not a plan.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanError {
    /// No stream of the text names a chain.
    #[error("the plan has no Order field. Each stream names its issues in one")]
    NoOrder,
    /// One stream of the plan names no chain.
    #[error("stream {0:?} has no Order field")]
    StreamWithoutOrder(Snippet),
    /// The `Order` field of a stream holds a token that names no issue.
    #[error("stream {stream:?}: {token:?} is not an issue number")]
    NotAnIssue {
        /// The label of the stream that holds the token.
        stream: Snippet,
        /// The token itself.
        token: Snippet,
    },
    /// The `Order` field of a stream holds no number at all.
    #[error("stream {0:?}: the Order field holds no issue number")]
    NoIssues(Snippet),
    /// A group stands before the first step of a stream, so it attaches to
    /// nothing.
    #[error("stream {stream:?}: {token:?} stands before any issue number")]
    UnattachedPair {
        /// The label of the stream that holds the group.
        stream: Snippet,
        /// The group itself.
        token: Snippet,
    },
    /// A row of a table splits into a cell count the header does not have.
    #[error("row has {cells} cells, the header has {header}: {line:?}")]
    RowWidth {
        /// The number of cells the row splits into.
        cells: usize,
        /// The number of columns the header names.
        header: usize,
        /// The row itself, as the table writes it.
        line: Snippet,
    },
    /// A group opens and never closes, so where it ends is a guess. A
    /// parenthesis of the prose inside it closes the group it opened and no
    /// other, so `#4 (a (b) c` opens a group that never closes.
    #[error("stream {stream:?}: {token:?} has no closing parenthesis")]
    UnclosedGroup {
        /// The label of the stream that holds the group.
        stream: Snippet,
        /// The group, from its opening parenthesis to the end of the field,
        /// with every parenthesis of its prose.
        token: Snippet,
    },
    /// A second group stands on one step, and a step closes one issue.
    #[error("stream {stream:?}: {token:?} is a second issue for one step")]
    SecondPair {
        /// The label of the stream that holds the group.
        stream: Snippet,
        /// The group itself.
        token: Snippet,
    },
}

/// The five fields a stream of a plan is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    /// The name of the stream.
    Stream,
    /// The chain of the stream. One of the two fields this module reads for
    /// numbers.
    Order,
    /// The work of other streams that comes before the first step of this one.
    /// The other field this module reads for numbers.
    WaitsFor,
    /// The part of the repository the stream works in.
    Zone,
    /// The prose of the stream.
    Notes,
}

impl Key {
    /// The five keys, to read a line against.
    const ALL: [Self; 5] = [
        Self::Stream,
        Self::Order,
        Self::WaitsFor,
        Self::Zone,
        Self::Notes,
    ];

    /// The word the key is written with.
    fn word(self) -> &'static str {
        match self {
            Self::Stream => "Stream",
            Self::Order => "Order",
            Self::WaitsFor => "Waits for",
            Self::Zone => "Zone",
            Self::Notes => "Notes",
        }
    }
}

/// Is `text` a plan of several streams, and not one chain?
///
/// True when a line of `text` opens a `Stream` field or an `Order` field, and
/// true when a row of a table names a `Stream` column or an `Order` column.
///
/// `Stream` counts on its own so that a plan with no `Order` field reaches
/// [`parse`] and earns an error that says which field is missing. The chain
/// reader would answer such a text with a complaint about the token `Stream:`,
/// which tells the reader nothing about what to write instead.
#[must_use]
pub fn looks_like_a_plan(text: &str) -> bool {
    if find_header(text).is_some() {
        return true;
    }
    text.lines()
        .any(|line| matches!(key_of(line), Some((Key::Stream | Key::Order, _))))
}

/// Read the streams of `text`, in the order it writes them.
///
/// # Errors
///
/// Gives [`PlanError::NoOrder`] for a text where no stream names a chain,
/// [`PlanError::StreamWithoutOrder`] for one stream that names none while
/// another one does,
/// [`PlanError::NoIssues`] for an `Order` field with no number in it,
/// [`PlanError::NotAnIssue`] for a token of an `Order` field or a `Waits for`
/// field that names no issue, and [`PlanError::UnattachedPair`] or
/// [`PlanError::SecondPair`] for a group that attaches to no step or to a step
/// that already holds one.
pub fn parse(text: &str) -> Result<Plan, PlanError> {
    let streams = match find_header(text) {
        Some((body, header)) => table_streams(text, body, &header)?,
        None => record_streams(text)?,
    };
    // A text that names a column or a key and then names no chain at all is a
    // plan of nothing. The reader wrote a header and stopped, and an empty
    // answer would print nothing and say why nowhere.
    if streams.is_empty() {
        return Err(PlanError::NoOrder);
    }
    Ok(Plan { streams })
}

/// One stream of a plan, as the record form writes it.
///
/// The three fields this module reads. `Zone` and `Notes` open a field and
/// close the field before it, and the text of them goes nowhere.
#[derive(Default)]
struct Record {
    /// The text of the `Stream` field, when the record holds one.
    label: Option<String>,
    /// The text of the `Order` field, when the record holds one.
    order: Option<String>,
    /// The text of the `Waits for` field, when the record holds one.
    ///
    /// A record that holds none waits for nothing, which is what a record with
    /// an empty field says as well.
    waits: Option<String>,
}

/// The streams the record form of `text` writes.
///
/// # Errors
///
/// Gives [`PlanError::NoOrder`] when no record names a chain. That question is
/// asked of every record first, because a text where each record is missing
/// the same field is a text written in a form this module does not read, and
/// a complaint about the first record alone points the reader at one line of
/// it. Gives the errors of [`stream_of`] for one record that names a chain
/// this module cannot read.
fn record_streams(text: &str) -> Result<Vec<Stream>, PlanError> {
    let records = records_of(text);
    if records.iter().all(|record| record.order.is_none()) {
        return Err(PlanError::NoOrder);
    }
    records
        .iter()
        .enumerate()
        .map(|(place, record)| {
            stream_of(
                record.label.as_deref(),
                place,
                record.order.as_deref(),
                record.waits.as_deref(),
            )
        })
        .collect()
}

/// Cut `text` into one record for each stream.
///
/// A `Stream` field starts a record, and so does an `Order` field that has no
/// record to go into or that meets a record which already holds one. A plan
/// written with no `Stream` field at all is therefore still a plan of several
/// streams.
fn records_of(text: &str) -> Vec<Record> {
    let mut records: Vec<Record> = Vec::new();
    let mut open: Option<Record> = None;
    let mut field: Option<(Key, String)> = None;
    for line in text.lines() {
        if line.trim().is_empty() {
            close_field(&mut field, &mut open);
            continue;
        }
        if is_rule(line) {
            continue;
        }
        let Some((key, value)) = key_of(line) else {
            // A line that opens no field continues the one that is open. The
            // notes of a stream run over three lines as readily as one.
            if let Some((_, text)) = field.as_mut() {
                text.push(' ');
                text.push_str(line.trim());
            }
            continue;
        };
        close_field(&mut field, &mut open);
        let starts_a_record = match key {
            Key::Stream => true,
            Key::Order => open.as_ref().is_none_or(|record| record.order.is_some()),
            Key::WaitsFor | Key::Zone | Key::Notes => false,
        };
        if starts_a_record {
            records.extend(open.take());
            open = Some(Record::default());
        }
        field = Some((key, value.trim().to_string()));
    }
    close_field(&mut field, &mut open);
    records.extend(open);
    records
}

/// Put the field that is open into the record that is open, and close it.
fn close_field(field: &mut Option<(Key, String)>, open: &mut Option<Record>) {
    let Some((key, text)) = field.take() else {
        return;
    };
    match key {
        Key::Stream => open.get_or_insert_with(Record::default).label = Some(text),
        Key::Order => open.get_or_insert_with(Record::default).order = Some(text),
        Key::WaitsFor => open.get_or_insert_with(Record::default).waits = Some(text),
        Key::Zone | Key::Notes => {}
    }
}

/// The field `line` opens, and the text after the colon of it.
///
/// A key stands first or it is not a key: `Finish-what-we-started:` opens no
/// field, because the word before its first colon is none of the four. This is
/// what keeps a sentence of the notes out of the parser, and a sentence of the
/// notes is where the numbers that are not issues live.
fn key_of(line: &str) -> Option<(Key, &str)> {
    let (head, rest) = line.trim_start().split_once(FIELD_COLON)?;
    let key = Key::ALL
        .into_iter()
        .find(|key| head.eq_ignore_ascii_case(key.word()))?;
    Some((key, rest))
}

/// Is `line` a rule a reader draws between two streams or two rows?
///
/// One rule for both forms, and one rule for every drawing of a table. It
/// deletes `┌─┬─┐`, `├─┼─┤`, and `└─┴─┘`, the `+---+` of an ASCII table, the
/// `| --- |` divider of a Markdown table, and the `|:--- | ---:|` divider that
/// carries an alignment colon. A divider that stayed would be a stream whose
/// `Order` field is `---:`.
///
/// A rule carries no field and no prose: every character of it is a bar, a
/// space, or a mark of [`is_rule_mark`]. It carries three marks at the least,
/// because `--` is the tail of an arrow and a rule is a line.
///
/// The count leaves the bar and the space out, and that is what parts a rule
/// from the empty row `|   |   |`. An empty row carries no mark, so it is no
/// rule. [`table_body`] drops it, and a rule there would end the row above it
/// and cut a row that wraps in two.
fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().all(is_rule_char)
        && trimmed.chars().filter(|&c| is_rule_mark(c)).count() >= RULE_CHARS
}

/// Is `c` a character a line of rule carries?
///
/// A mark, the bar of a cell, or a space of any kind — the space a Markdown
/// divider writes around its cells. The bar and the space draw nothing, so a
/// line of nothing but those two is no rule.
fn is_rule_char(c: char) -> bool {
    is_rule_mark(c) || c.is_whitespace() || TABLE_BARS.contains(&c)
}

/// Is `c` a mark a reader draws a rule with?
///
/// The box-drawing block holds every line and every corner. Beside it stand
/// the marks a keyboard writes: the hyphen, the equals sign, the underscore,
/// the two dashes a word processor writes for a hyphen, the `+` of an ASCII
/// table, and the `:` of an alignment.
fn is_rule_mark(c: char) -> bool {
    matches!(
        c,
        '\u{2500}'..='\u{257f}' | '-' | '=' | '_' | '+' | ':' | '\u{2013}' | '\u{2014}'
    )
}

/// The row of `text` that names the columns of a table.
///
/// Gives the line the body of the table starts on, and the cells of the header
/// itself. A row that names a `Stream` column or an `Order` column is the
/// header, because a plan names its streams or the work in them. A `Waits for`
/// column stands beside those two and never alone: a table of blockers and no
/// work of its own is a table of nothing to start. The `┌─┬─┐` a box table
/// opens with is a rule and never a header.
fn find_header(text: &str) -> Option<(usize, Vec<&str>)> {
    text.lines().enumerate().find_map(|(place, line)| {
        if is_rule(line) {
            return None;
        }
        let cells = table_cells(line)?;
        let names_a_column = cells
            .iter()
            .any(|cell| is_key_cell(cell, Key::Stream) || is_key_cell(cell, Key::Order));
        names_a_column.then_some((place + 1, cells))
    })
}

/// The streams the body of a table writes.
///
/// # Errors
///
/// Gives [`PlanError::NoOrder`] for a table with no `Order` column, the error
/// of [`table_body`] for the lines under the header, and the errors of
/// [`stream_of`] for one row of it.
///
/// A table with no `Waits for` column names no blocker, and that is a plan of
/// streams that stand apart. So the column is optional, and its absence reads
/// as an empty cell in every row.
fn table_streams(text: &str, body: usize, header: &[&str]) -> Result<Vec<Stream>, PlanError> {
    let order_at = column_of(header, Key::Order).ok_or(PlanError::NoOrder)?;
    let stream_at = column_of(header, Key::Stream);
    let waits_at = column_of(header, Key::WaitsFor);
    let rows = rows_of(&table_body(text, body, header)?, order_at);
    rows.iter()
        .enumerate()
        .map(|(place, row)| {
            let named = stream_at.and_then(|at| row.get(at)).map(String::as_str);
            let waits = waits_at.and_then(|at| row.get(at)).map(String::as_str);
            stream_of(named, place, row.get(order_at).map(String::as_str), waits)
        })
        .collect()
}

/// One line of the body of a table.
///
/// The rules stay, rather than being stepped over: a rule that stands between
/// two row lines is where one row ends and the next one starts, and it is the
/// only mark a table carries that says so for certain. A cell says nothing
/// about the row it belongs to.
///
/// A rule carries the line it was drawn on, because how a rule is drawn says
/// which table this is. `| --- |` opens a Markdown table and `├─┼─┤` opens a
/// box one, and [`row_reading`] reads the two apart.
enum BodyLine<'a> {
    /// A line a reader draws between two rows, or around the table.
    Rule(&'a str),
    /// The cells of one line of a row.
    Cells(Vec<&'a str>),
}

/// The lines the body of a table holds, from `body` to the end of the table.
///
/// The table ends at the first line that is no row of it, because a report of
/// parallel work holds more than the stream table. A Housekeeping table and a
/// table of the work already in flight stand under it, and neither one is more
/// streams to start: a reader that ran to the end of the page named the rows
/// of those tables as work, and a row whose cell under the `Order` column
/// holds a word rather than a number took the whole plan down with it.
///
/// A row of nothing but empty cells is dropped, and it ends the table no more
/// than a rule does.
///
/// # Errors
///
/// Gives [`PlanError::RowWidth`] for a row whose cell count `header` does not
/// have.
fn table_body<'a>(
    text: &'a str,
    body: usize,
    header: &[&str],
) -> Result<Vec<BodyLine<'a>>, PlanError> {
    let mut lines: Vec<BodyLine<'a>> = Vec::new();
    for line in text.lines().skip(body) {
        if is_rule(line) {
            lines.push(BodyLine::Rule(line));
            continue;
        }
        let Some(cells) = table_cells(line) else {
            break;
        };
        if cells.iter().all(|cell| cell.is_empty()) {
            continue;
        }
        // The header names the column count, and a row that splits into
        // another one puts every cell after its stray bar under the wrong
        // column. Such a cell is rare, and guessing at it is worse than
        // refusing it, because the guess reads a note as a chain.
        if cells.len() != header.len() {
            return Err(PlanError::RowWidth {
                cells: cells.len(),
                header: header.len(),
                line: Snippet::new(line),
            });
        }
        lines.push(BodyLine::Cells(cells));
    }
    Ok(lines)
}

/// The rows `lines` writes, each one the cells of every line it wraps onto.
///
/// [`row_reading`] says which of the three readings tells the rows of this
/// table apart, and each line of the body is then asked under that one.
fn rows_of(lines: &[BodyLine], order_at: usize) -> Vec<Vec<String>> {
    let reading = row_reading(lines);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row_is_open = false;
    for line in lines {
        let cells = match line {
            BodyLine::Rule(_) => {
                row_is_open = false;
                continue;
            }
            BodyLine::Cells(cells) => cells,
        };
        let continues = match reading {
            RowReading::Rules => row_is_open,
            RowReading::Lines => false,
            RowReading::Cells => continues_a_row(cells, order_at),
        };
        if continues {
            // A continuation with no row above it continues nothing. The plan
            // then holds one row fewer, and a plan of no rows at all is the
            // error [`parse`] gives for a header with nothing under it.
            if let Some(row) = rows.last_mut() {
                join_row(row, cells);
            }
        } else {
            rows.push(cells.iter().copied().map(str::to_string).collect());
        }
        row_is_open = true;
    }
    rows
}

/// Which of the three readings says where one row of a table ends.
///
/// A table says so with a mark of its own wherever it can, because a mark
/// answers for every row and a cell answers for one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowReading {
    /// The rules of the table stand between its rows, so a row opens under a
    /// rule and takes every line up to the next one.
    Rules,
    /// The table is a Markdown one, so one line writes one row and no line
    /// continues another.
    Lines,
    /// The table carries neither mark, so each line is asked for itself with
    /// [`continues_a_row`].
    Cells,
}

/// Which reading tells the rows of `lines` apart.
///
/// The two marks come first, because each one answers for the whole table.
/// [`continues_a_row`] answers for one line and it is the last reading asked.
fn row_reading(lines: &[BodyLine]) -> RowReading {
    if rules_divide_the_rows(lines) {
        RowReading::Rules
    } else if opens_with_a_markdown_divider(lines) {
        RowReading::Lines
    } else {
        RowReading::Cells
    }
}

/// Do the rules of `lines` say where each row of the table ends?
///
/// A rule with a row line above it and a row line under it stands between two
/// rows, and a renderer that draws one draws them between every pair. So one
/// such rule answers for the whole table.
///
/// The two other written forms answer `false` here. A Markdown table carries
/// one rule and it stands over the first row. A box table drawn with its outer
/// border alone carries two, and the one under the rows closes the table.
fn rules_divide_the_rows(lines: &[BodyLine]) -> bool {
    let is_row = |line: &BodyLine| matches!(line, BodyLine::Cells(_));
    let first = lines.iter().position(is_row);
    let last = lines.iter().rposition(is_row);
    match (first, last) {
        (Some(first), Some(last)) => lines[first..last]
            .iter()
            .any(|line| matches!(line, BodyLine::Rule(_))),
        _ => false,
    }
}

/// Is `lines` the body of a Markdown table?
///
/// A Markdown table opens its body with the divider that names the alignment
/// of each column, so the first line of the body answers.
fn opens_with_a_markdown_divider(lines: &[BodyLine]) -> bool {
    matches!(lines.first(), Some(BodyLine::Rule(line)) if is_markdown_divider(line))
}

/// Is `line` the divider of a Markdown table?
///
/// A divider carries a bar and it is drawn with [`MARKDOWN_DIVIDER_MARKS`]
/// alone: `|---|---|`, `| --- | --- |`, and `| :---: | :---: |`. Every other
/// mark of [`is_rule_mark`] belongs to another drawing, so `├─────┼─────┤`
/// and `+-----+-----+` answer `false` and their tables keep the reading they
/// have.
///
/// Ask this of a line [`is_rule`] answers for. A row of prose carries no mark
/// at all, and a line of no marks is a divider of nothing.
fn is_markdown_divider(line: &str) -> bool {
    line.contains(TABLE_BARS)
        && line
            .chars()
            .filter(|&c| is_rule_mark(c))
            .all(|c| MARKDOWN_DIVIDER_MARKS.contains(&c))
}

/// Does `cells` continue the row above it, rather than open one?
///
/// The third reading, for a table that carries neither mark of its own and
/// where the cells are all a reader has. The `Order` cell is the one that
/// answers: a step of a chain never opens with an arrow, and a row that
/// carries no step carries no chain. "The first cell is empty" reads the same
/// way and is wrong, because a label wraps as readily as a chain does — the
/// row of stream B of a real report carries the word `engine` in its first
/// cell and nothing in its `Order` cell.
///
/// It answers for one line, so it cannot hold a row together through a wrap
/// that falls in the middle of a chain. `(#329)` opens no step and `#330`
/// opens one, and a table with rules between its rows is what says that both
/// of them continue the row above. [`rules_divide_the_rows`] finds that table.
///
/// It reads an empty `Order` cell as a wrap, and a Markdown table writes no
/// wrap at all: one row of one is one line, by definition. So an empty cell
/// there is a stream that names no chain, and the message that says which
/// stream is worth more than a row this reading would drop.
/// [`opens_with_a_markdown_divider`] finds that table.
fn continues_a_row(cells: &[&str], order_at: usize) -> bool {
    cells.get(order_at).is_some_and(|order| {
        order.is_empty()
            || order
                .chars()
                .next()
                .is_some_and(|c| SEPARATORS.contains(&c))
    })
}

/// Join each cell of `cells` to the cell of `row` above it, with one space.
///
/// An empty cell adds nothing and takes nothing, so the label of a row that
/// wraps in its `Order` cell alone keeps the space it was written with.
fn join_row(row: &mut [String], cells: &[&str]) {
    for (held, cell) in row.iter_mut().zip(cells) {
        if cell.is_empty() {
            continue;
        }
        if !held.is_empty() {
            held.push(' ');
        }
        held.push_str(cell);
    }
}

/// The cells of `line`, or `None` when `line` is no row of a table.
///
/// The bar of a table is the bar of a chain as well, so a row is read by its
/// cells and never by its shape: `#1 || #2` holds two bars and names no
/// column, and it is a chain.
fn table_cells(line: &str) -> Option<Vec<&str>> {
    if !line.contains(TABLE_BARS) {
        return None;
    }
    let mut cells: Vec<&str> = line.split(TABLE_BARS).map(str::trim).collect();
    // The bar at each end of a row gives an empty cell that is no cell. The
    // two bars are optional, so each end is dropped only when it is empty.
    if cells.first().is_some_and(|cell| cell.is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|cell| cell.is_empty()) {
        cells.truncate(cells.len().saturating_sub(1));
    }
    Some(cells)
}

/// Does `cell` name the column `key` names?
fn is_key_cell(cell: &str, key: Key) -> bool {
    cell.eq_ignore_ascii_case(key.word())
}

/// The place of the column `key` names among `header`.
fn column_of(header: &[&str], key: Key) -> Option<usize> {
    header.iter().position(|cell| is_key_cell(cell, key))
}

/// The stream one record or one row writes.
///
/// `named` is the text of the `Stream` field or cell, `place` is the place of
/// the stream in the plan, `order` is the text of the `Order` field or cell,
/// and `waits` is the text of the `Waits for` field or cell.
///
/// # Errors
///
/// Gives [`PlanError::StreamWithoutOrder`] when the stream names no `Order`
/// field at all, the errors of [`read_order`] for the chain in one, and the
/// errors of [`read_waits`] for the blockers of the stream.
fn stream_of(
    named: Option<&str>,
    place: usize,
    order: Option<&str>,
    waits: Option<&str>,
) -> Result<Stream, PlanError> {
    let label = label_of(named, place);
    let order = order.ok_or_else(|| PlanError::StreamWithoutOrder(Snippet::new(&label)))?;
    let steps = read_order(order, &label)?;
    let waits_for = read_waits(waits, &label)?;
    Ok(Stream {
        label,
        steps,
        waits_for,
    })
}

/// The name of the stream at `place`.
///
/// A stream the plan does not name takes its place as a name, counted from
/// one. Every message about a stream names it, so a stream with no name of its
/// own still gets a message the reader can follow back to a line of the plan.
fn label_of(named: Option<&str>, place: usize) -> String {
    match named.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None => format!("{UNNAMED_LABEL} {}", place + 1),
    }
}

/// Read the chain of one `Order` field into the steps of a stream.
///
/// `label` names the stream every message of this function repeats back,
/// because a plan holds several chains and a message about one of them says
/// nothing until it says which.
///
/// # Errors
///
/// Gives [`PlanError::NotAnIssue`] for a token that names no issue,
/// [`PlanError::UnattachedPair`] for a group that stands before every step,
/// [`PlanError::SecondPair`] for a second group on one step,
/// [`PlanError::NoIssues`] for a field with no number in it, and
/// [`PlanError::UnclosedGroup`] for a group that opens and never closes.
fn read_order(order: &str, label: &str) -> Result<Vec<Step>, PlanError> {
    let mut readings: Vec<Reading> = Vec::new();
    let pieces = pieces(order).map_err(|open| PlanError::UnclosedGroup {
        stream: Snippet::new(label),
        token: Snippet::new(&format!("{GROUP_OPEN}{open}")),
    })?;
    for piece in pieces {
        match piece {
            Piece::Step(token) => {
                let (number, marked) =
                    marked_number(&token).ok_or_else(|| PlanError::NotAnIssue {
                        stream: Snippet::new(label),
                        token: Snippet::new(&token),
                    })?;
                // Through the constructor, so one place builds a step. A
                // group that follows attaches to it below.
                readings.push(Reading {
                    step: Step::new(number, None),
                    marked,
                });
            }
            Piece::Group(text) => {
                // The group is repeated back with its parentheses, because
                // that is how the reader wrote it and how they find it again.
                let written = group_text(&text);
                // A group annotates the step to its left, and it never opens
                // one. So a group that stands first attaches to nothing,
                // whatever it holds.
                if readings.is_empty() {
                    return Err(PlanError::UnattachedPair {
                        stream: Snippet::new(label),
                        token: Snippet::new(&written),
                    });
                }
                let Some(annotation) = annotation_of(&text, label)? else {
                    continue;
                };
                let reading = readings.last_mut().expect("the list holds a step");
                if reading.step.closes.is_some() {
                    return Err(PlanError::SecondPair {
                        stream: Snippet::new(label),
                        token: Snippet::new(&written),
                    });
                }
                reading.annotate(annotation);
            }
        }
    }
    if readings.is_empty() {
        return Err(PlanError::NoIssues(Snippet::new(label)));
    }
    Ok(readings.into_iter().map(|reading| reading.step).collect())
}

/// Read one `Waits for` field into the steps the stream waits for.
///
/// `waits` is the text of the field or the cell, and `label` names the stream
/// every message repeats back.
///
/// A stream that waits for nothing writes an empty cell, and a plan whose
/// streams all stand apart writes no column at all. Both are the common case
/// and neither is an error, so a text of spaces alone and an absent text each
/// give no steps. A drawn cell is padded to the width of its column, which is
/// why the spaces are asked about.
///
/// Every other text is steps, and [`read_order`] is what reads a step. The
/// order the steps arrive in says nothing: `Order` is a chain and `Waits for`
/// is a set, so `#96 → #91` and `#96, #91` name the same two blockers and the
/// stream starts when both are finished.
///
/// # Errors
///
/// Gives the errors of [`read_order`], so a cell of prose earns
/// [`PlanError::NotAnIssue`] with the name of the stream and the word that
/// names no issue.
fn read_waits(waits: Option<&str>, label: &str) -> Result<Vec<Step>, PlanError> {
    match waits.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => read_order(text, label),
        None => Ok(Vec::new()),
    }
}

/// The one step `text` writes, or `None` when it writes none or more than one.
///
/// The reader of a picture asks this of the text beside a wire. A port of a
/// net is one step and not a bare number, so `#249  (gallery)` is the step
/// `#249` and `#4 (in flight, PR #15)` is one step that holds two numbers.
/// The two forms read here exactly as they read in a table, because one
/// function reads both.
///
/// It is built on [`read_order`], whose messages name the stream that holds
/// the text. A picture has no stream to name, so the label is empty and the
/// messages go nowhere: a port that names no step is not an error of this
/// function, it is text that is not a port.
pub(crate) fn one_step(text: &str) -> Option<Step> {
    let steps = read_order(text, "").ok()?;
    match steps.as_slice() {
        [step] => Some(*step),
        _ => None,
    }
}

/// The report of the `plan-parallel-work` skill, as it arrives on the
/// clipboard.
///
/// The paste of issue #416, character for character. Every rule the table form
/// needs stands in it: the `│` bar, the `┌─┬─┐` rules, a row that wraps onto a
/// second line in its `Order` cell, a row that wraps in its `Stream` cell
/// alone, and an `Order` field that annotates a step in parentheses.
///
/// It stands beside the module rather than inside its tests because the reader
/// of a picture must refuse this same text, and the border of this table is a
/// net that touches every cell of it. One text asked of both readers is what
/// keeps the two answers together; a second copy would drift.
#[cfg(test)]
pub(crate) const BOX_TABLE: &str = include_str!("../fixtures/plan-parallel-work.txt");

/// One step of a stream, with the mark the plan wrote on its number.
///
/// The mark stands here and not on [`Step`], because it says nothing to a
/// reader of the answer. It settles one question inside this module: which of
/// the two numbers of a pair is the work.
struct Reading {
    /// The step itself.
    step: Step,
    /// The number of the step carries a `PR` in front of it.
    marked: bool,
}

impl Reading {
    /// Give the step the number its annotation names.
    ///
    /// A step holds the work and the issue the work closes, in that order. The
    /// plan marks the work with `PR`, so a marked number inside the group and
    /// an unmarked number outside it means the two arrived the other way round
    /// and swap here. A group that marks a step which is marked already names
    /// a second pull request for one piece of work, and the number that opened
    /// the step stands as the work, because it opened it.
    fn annotate(&mut self, annotation: Annotation) {
        if annotation.marked && !self.marked {
            self.step.closes = Some(self.step.number);
            self.step.number = annotation.number;
            self.marked = true;
            return;
        }
        self.step.closes = Some(annotation.number);
    }
}

/// The number one group gives the step before it.
struct Annotation {
    /// The number itself.
    number: IssueNumber,
    /// The plan wrote `PR` in front of it, so this number is the work.
    marked: bool,
}

/// One piece of an `Order` field.
enum Piece {
    /// A step of the stream, as the field writes it.
    Step(String),
    /// The text of a group, without the parentheses around it.
    Group(String),
}

/// A group of an `Order` field, while it is open.
///
/// The depth is what parts the parenthesis that closes the group from a
/// parenthesis of its prose. A group holds prose, and prose holds parentheses.
struct OpenGroup {
    /// The text of the group so far, without the parenthesis that opened it.
    text: String,
    /// How many parentheses of the group stand open, the one that opened it
    /// counted. The parenthesis that brings this to zero closes the group.
    depth: usize,
}

/// Cut `order` into its steps and its groups.
///
/// A parenthesis opens and closes a group, and every character between the two
/// belongs to it. The text outside them is cut into steps by [`tokens_of`].
///
/// A group counts its depth, so a parenthesis inside one belongs to it and
/// only the parenthesis that brings the depth to zero closes it: `#4 (a note
/// (see the docs))` is one group of prose, and `#4 (in flight (rebasing), PR
/// #15)` still names the pull request of its step.
///
/// A closing parenthesis with no group open belongs to the token it arrived
/// in, so `#4)` is one token that names no issue and earns the message that
/// says so.
///
/// # Errors
///
/// Gives the text of a group that opens and never closes, so the caller can
/// name the stream it stands in. A nested parenthesis closes the group it
/// opened and no other, so `#4 (a (b) c` opens a group that never closes.
fn pieces(order: &str) -> Result<Vec<Piece>, String> {
    let mut pieces: Vec<Piece> = Vec::new();
    let mut outer = String::new();
    let mut group: Option<OpenGroup> = None;
    for c in order.chars() {
        match group.as_mut() {
            Some(open) if c == GROUP_CLOSE && open.depth == 1 => {
                pieces.push(Piece::Group(std::mem::take(&mut open.text)));
                group = None;
            }
            Some(open) => {
                match c {
                    GROUP_OPEN => open.depth += 1,
                    // The arm above holds the last parenthesis of the group,
                    // so the depth here is two at the least.
                    GROUP_CLOSE => open.depth -= 1,
                    _ => {}
                }
                open.text.push(c);
            }
            None if c == GROUP_OPEN => {
                push_steps(&mut outer, &mut pieces);
                group = Some(OpenGroup {
                    text: String::new(),
                    depth: 1,
                });
            }
            None => outer.push(c),
        }
    }
    if let Some(open) = group {
        return Err(open.text);
    }
    push_steps(&mut outer, &mut pieces);
    Ok(pieces)
}

/// Put the steps `outer` writes into `pieces`, and empty it.
fn push_steps(outer: &mut String, pieces: &mut Vec<Piece>) {
    pieces.extend(tokens_of(outer).into_iter().map(Piece::Step));
    outer.clear();
}

/// Cut `text` into the tokens that each name one number.
///
/// Whitespace and a separator of a chain each end a token. So does a `#` that
/// arrives while a token is open, which is what makes `#1#2` two steps written
/// with no separator at all. A `#` that arrives on the `PR` of a pull request
/// ends nothing, because `PR#344` is one number written the way a plan writes
/// it.
fn tokens_of(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut token = String::new();
    for c in text.chars() {
        if c.is_whitespace() || SEPARATORS.contains(&c) {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            continue;
        }
        if c == HASH && !token.is_empty() && !token.eq_ignore_ascii_case(PULL_REQUEST_PREFIX) {
            tokens.push(std::mem::take(&mut token));
        }
        token.push(c);
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

/// The number one group gives its step, or `None` when the group is prose
/// alone.
///
/// Only a token that carries the `#` is a number here. Every other word is
/// prose the reader drops, so an annotation states a count, a width, or who
/// does the work without any of it reaching the chain. `text` is the whole
/// group, so the prose it drops holds the parentheses of a nested group as
/// well: the word `(rebasing)` carries no `#` and it is a word.
///
/// # Errors
///
/// Gives [`PlanError::NotAnIssue`] for a token that carries the `#` and names
/// no issue, and [`PlanError::SecondPair`] for a second number in one group,
/// because a step closes one issue.
fn annotation_of(text: &str, label: &str) -> Result<Option<Annotation>, PlanError> {
    let mut found: Option<Annotation> = None;
    let mut marked = false;
    for token in tokens_of(text) {
        if token.eq_ignore_ascii_case(PULL_REQUEST_PREFIX) {
            marked = true;
            continue;
        }
        let bare = without_pull_request_prefix(&token);
        let glued = bare.len() != token.len();
        if !bare.starts_with(HASH) {
            // Prose, and prose that stands between a mark and a number takes
            // the mark with it: `PR` marks the number it stands in front of.
            marked = false;
            continue;
        }
        let number = read_number(bare).ok_or_else(|| PlanError::NotAnIssue {
            stream: Snippet::new(label),
            token: Snippet::new(&group_text(&token)),
        })?;
        if found.is_some() {
            return Err(PlanError::SecondPair {
                stream: Snippet::new(label),
                token: Snippet::new(&group_text(&token)),
            });
        }
        found = Some(Annotation {
            number,
            marked: marked || glued,
        });
        marked = false;
    }
    Ok(found)
}

/// `text` with the parentheses of a group around it, as a message writes it.
fn group_text(text: &str) -> String {
    format!("{GROUP_OPEN}{text}{GROUP_CLOSE}")
}

/// The number `token` names and whether it carries the `PR` mark, or `None`
/// when it names no number.
fn marked_number(token: &str) -> Option<(IssueNumber, bool)> {
    let bare = without_pull_request_prefix(token);
    let marked = bare.len() != token.len();
    read_number(bare).map(|number| (number, marked))
}

/// `token` with its `PR` dropped, or `token` when it carries none.
///
/// Cut by characters and never by bytes: a plan arrives from a paste, and a
/// paste holds whatever the reader copied.
fn without_pull_request_prefix(token: &str) -> &str {
    let mut characters = token.chars();
    let head: String = characters
        .by_ref()
        .take(PULL_REQUEST_PREFIX.chars().count())
        .collect();
    if head.eq_ignore_ascii_case(PULL_REQUEST_PREFIX) {
        characters.as_str()
    } else {
        token
    }
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::*;

    /// The plan of issue #413, as a record for each stream.
    ///
    /// Real prose, because the trap this module exists for is real prose: the
    /// notes of three of these streams name numbers that are not issues.
    const RECORDS: &str = "\
Stream: S1 gitscratch → grind → grime
Order: PR#344 (#341) → PR#343 (#329) → PR#342 (#328) → #330 → #331
Zone: src/gitscratch, src/grist, new src/grind, src/grime
Notes: Three sibling PRs off one merge base. All three edit tests/safety.rs and both READMEs, so each merge
forces a rebase of the next. #341 is the bug (a rebase halt with nothing unmerged drops work), so it goes
ahead of the grind consumer. PR#342 pays the largest rebase.
────────────────────────────────────────
Stream: S2 ic
Order: #350 → #187 → #188
Zone: src/ic, src/termgfx
Notes: All three land inside display_image (main.rs:1566-1650). Highest collision in the set. Branch
ic-xtermjs already holds 5 commits of #188 and is dirty.
────────────────────────────────────────
Stream: S3 crap
Order: #314 → #315
Zone: src/crap
Notes: The two hunks sit 265 lines apart in a 5113-line file, so the rebase is cheap.
────────────────────────────────────────
Stream: S4 prcp
Order: #265 → #266
Zone: src/prcp
Notes: #320 landed 2026-08-25 and took the shell integration with it.
────────────────────────────────────────
Stream: S5 tvfind
Order: #321
Zone: src/tvfind
Notes: One issue, no neighbors.
────────────────────────────────────────
Stream: S6 vpn-tunnel
Order: #191 → #192
Zone: src/vpn-tunnel
Notes: Both edits land within a 30-line window of compose.rs.
────────────────────────────────────────
Stream: S7 dwt
Order: #196
Zone: src/dwt
Notes: Independent of everything above.";

    /// The same seven streams, as one table.
    ///
    /// The same labels and the same `Order` cells as [`RECORDS`], and notes
    /// that carry the same traps, so the two forms are asked for one answer.
    const TABLE: &str = "\
| Stream | Order | Zone | Notes |
| --- | --- | --- | --- |
| S1 gitscratch → grind → grime | PR#344 (#341) → PR#343 (#329) → PR#342 (#328) → #330 → #331 | src/gitscratch, src/grist, new src/grind, src/grime | Three sibling PRs off one merge base. #341 is the bug. PR#342 pays the largest rebase. |
| S2 ic | #350 → #187 → #188 | src/ic, src/termgfx | All three land inside display_image (main.rs:1566-1650). Branch ic-xtermjs holds 5 commits of #188. |
| S3 crap | #314 → #315 | src/crap | The two hunks sit 265 lines apart in a 5113-line file, so the rebase is cheap. |
| S4 prcp | #265 → #266 | src/prcp | #320 landed 2026-08-25 and took the shell integration with it. |
| S5 tvfind | #321 | src/tvfind | One issue, no neighbors. |
| S6 vpn-tunnel | #191 → #192 | src/vpn-tunnel | Both edits land within a 30-line window of compose.rs. |
| S7 dwt | #196 | src/dwt | Independent of everything above. |";

    /// A table of one stream, and the Housekeeping table a report writes under
    /// it.
    ///
    /// A report of parallel work holds more than the stream table. The rows of
    /// this second one are issues to close, and an issue to close is no stream
    /// of work to start.
    const TABLE_AND_HOUSEKEEPING: &str = "\
| Stream | Order | Zone | Notes |
|---|---|---|---|
| S1 | #344 | src/a | fine |

## Housekeeping
| Closeable | #330 |
| Stale | #331 |";

    /// The same report, with a header of its own on the second table.
    ///
    /// The cell of the second table that stands where the `Order` column
    /// stands holds a word, so a reader that walks into the table names that
    /// word as an issue that is not one.
    const TABLE_AND_A_TABLE_OF_PROSE: &str = "\
| Stream | Order | Zone | Notes |
|---|---|---|---|
| S1 | #344 | src/a | fine |

## Housekeeping
| Issue | Action |
| #330 | close |";

    /// A Markdown table whose first row names no chain.
    ///
    /// The `Order` cell of stream A is empty. A Markdown table writes one row
    /// on one line, so that cell is a stream with nothing to start, and never
    /// a line that continues the row above it.
    const MARKDOWN_WITH_AN_EMPTY_FIRST_ORDER: &str = "\
| Stream | Order | Zone | Notes |
| --- | --- | --- | --- |
| A |  | src/a | nothing yet |
| B | #1 | src/b | go |";

    /// The same two rows, the other way round.
    ///
    /// The empty `Order` cell now stands under a row that holds a chain, which
    /// is where a reader of wraps joins the two and names the pair `A B`.
    const MARKDOWN_WITH_AN_EMPTY_SECOND_ORDER: &str = "\
| Stream | Order | Zone | Notes |
| --- | --- | --- | --- |
| A | #1 | src/a | go |
| B |  | src/b | nothing yet |";

    /// The same report of the same skill, drawn with a narrower `Order`
    /// column.
    ///
    /// The plan of this repository, character for character as it arrives on
    /// the clipboard. Its `Order` column is 28 columns wide, so the chain of
    /// S1 wraps twice, and neither line it wraps onto says that it continues a
    /// row: the second one opens with the annotation `(#329)`, and the third
    /// one opens with the step `#330`. The `├─┼─┤` rules are what say where
    /// each row of this table ends.
    const NARROW_BOX_TABLE: &str = include_str!("../fixtures/plan-parallel-work-narrow-order.txt");

    /// A plan of four streams, three of which name what they wait for.
    ///
    /// The `Waits for` cell of S1 holds one step and the cell of S2 holds two,
    /// written with the comma a reader types. S0 and S3 wait for nothing, and
    /// an empty cell is how a plan writes that.
    const WAITS_TABLE: &str = "\
| Stream | Order | Waits for | Zone | Notes |
|--------|-------|-----------|------|-------|
| S0 — daemon leak | #96 | | crates/tsm (serve.rs) | Do first, solo. |
| S1 — lifecycle | #91 | #96 | crates/tsm (kill.rs) | |
| S2 — install | #89 → #94 | #96, #91 | crates/tsm (shell-init) | Same hotspot as S1. |
| S3 — keymap | #86 | | packages/web | Disjoint. |";

    /// The same four streams, as a record for each one.
    ///
    /// S0 writes an empty `Waits for` field and S3 writes none at all, because
    /// a stream that waits for nothing is written both ways and the two say
    /// the same thing.
    const WAITS_RECORDS: &str = "\
Stream: S0 — daemon leak
Order: #96
Waits for:
Zone: crates/tsm (serve.rs)
Notes: Do first, solo.
────────────────────────────────────────
Stream: S1 — lifecycle
Order: #91
Waits for: #96
Zone: crates/tsm (kill.rs)
────────────────────────────────────────
Stream: S2 — install
Order: #89 → #94
Waits for: #96, #91
Zone: crates/tsm (shell-init)
Notes: Same hotspot as S1.
────────────────────────────────────────
Stream: S3 — keymap
Order: #86
Zone: packages/web
Notes: Disjoint.";

    /// The same four streams, drawn as a box table.
    ///
    /// The form a reader copies out of a terminal. The `Waits for` cells of S0
    /// and S3 are a run of spaces here, because a drawn cell is padded to the
    /// width of its column.
    const WAITS_BOX_TABLE: &str = "\
┌──────────────────┬───────────┬───────────┬─────────────────────────┬─────────────────────┐
│ Stream           │ Order     │ Waits for │ Zone                    │ Notes               │
├──────────────────┼───────────┼───────────┼─────────────────────────┼─────────────────────┤
│ S0 — daemon leak │ #96       │           │ crates/tsm (serve.rs)   │ Do first, solo.     │
├──────────────────┼───────────┼───────────┼─────────────────────────┼─────────────────────┤
│ S1 — lifecycle   │ #91       │ #96       │ crates/tsm (kill.rs)    │                     │
├──────────────────┼───────────┼───────────┼─────────────────────────┼─────────────────────┤
│ S2 — install     │ #89 → #94 │ #96, #91  │ crates/tsm (shell-init) │ Same hotspot as S1. │
├──────────────────┼───────────┼───────────┼─────────────────────────┼─────────────────────┤
│ S3 — keymap      │ #86       │           │ packages/web            │ Disjoint.           │
└──────────────────┴───────────┴───────────┴─────────────────────────┴─────────────────────┘";

    /// The label of the one stream of [`table_that_waits_for`].
    ///
    /// Every message about that stream repeats it back, so the tests that read
    /// a message name the stream from here.
    const ONE_STREAM_LABEL: &str = "S1";

    /// The numbers of one step: the work, and the issue the work closes.
    type StepNumbers = (u64, Option<u64>);

    /// The label of one stream, and the numbers of every step of it.
    type StreamShape<'a> = (&'a str, Vec<StepNumbers>);

    /// The label of every stream of `plan`, and the numbers of every step.
    fn shape(plan: &Plan) -> Vec<StreamShape<'_>> {
        plan.streams()
            .iter()
            .map(|stream| (stream.label(), steps_of(stream)))
            .collect()
    }

    /// The numbers of every step of `stream`, the pair second.
    fn steps_of(stream: &Stream) -> Vec<StepNumbers> {
        stream
            .steps()
            .iter()
            .map(|step| (step.number().get(), step.closes().map(IssueNumber::get)))
            .collect()
    }

    /// The numbers of every step of the stream at `index`.
    fn steps_at(plan: &Plan, index: usize) -> Vec<StepNumbers> {
        steps_of(
            plan.streams()
                .get(index)
                .expect("the plan holds this stream"),
        )
    }

    /// The numbers of every step `stream` waits for, the pair second.
    fn waits_of(stream: &Stream) -> Vec<StepNumbers> {
        stream
            .waits_for()
            .iter()
            .map(|step| (step.number().get(), step.closes().map(IssueNumber::get)))
            .collect()
    }

    /// The numbers of every step the stream at `index` waits for.
    fn waits_at(plan: &Plan, index: usize) -> Vec<StepNumbers> {
        waits_of(
            plan.streams()
                .get(index)
                .expect("the plan holds this stream"),
        )
    }

    /// A table of one stream, whose `Waits for` cell holds `waits`.
    ///
    /// One row of three columns, so a test about that one cell writes the cell
    /// and nothing else. The stream is named [`ONE_STREAM_LABEL`], because a
    /// message about the cell repeats the name of the stream back.
    fn table_that_waits_for(waits: &str) -> String {
        format!(
            "| Stream | Order | Waits for |\n\
             | --- | --- | --- |\n\
             | {ONE_STREAM_LABEL} | #1 | {waits} |"
        )
    }

    /// A record for one stream, whose `Waits for` field is written with `key`
    /// and holds `waits`.
    ///
    /// The key is an argument because a reader writes it in whatever case they
    /// type, and one record read twice is what says the two spellings are one
    /// field.
    fn record_that_waits_for(key: &str, waits: &str) -> String {
        format!("Stream: {ONE_STREAM_LABEL}\nOrder: #1\n{key}: {waits}")
    }

    /// [`BOX_TABLE`], drawn with `+---+` and `|`.
    ///
    /// Built out of the box form rather than typed a second time, so the two
    /// hold one plan and a test that reads them apart is reading the drawing
    /// and not the plan. The em dash of a label and the arrow of an `Order`
    /// cell are not drawing, so they stay.
    fn ascii_table() -> String {
        BOX_TABLE
            .chars()
            .map(|c| match c {
                '│' => '|',
                '─' => '-',
                '┌' | '┬' | '┐' | '├' | '┼' | '┤' | '└' | '┴' | '┘' => '+',
                other => other,
            })
            .collect()
    }

    /// The plan `text` writes.
    fn plan_of(text: &str) -> Plan {
        parse(text).expect("the text is a plan")
    }

    /// The numbers of `plan`, as a reader writes them.
    fn numbers_of(plan: &Plan) -> Vec<u64> {
        plan.numbers().iter().map(|number| number.get()).collect()
    }

    #[test]
    fn reads_a_record_for_each_stream_of_the_plan() {
        assert_eq!(
            shape(&plan_of(RECORDS)),
            vec![
                (
                    "S1 gitscratch → grind → grime",
                    vec![
                        (344, Some(341)),
                        (343, Some(329)),
                        (342, Some(328)),
                        (330, None),
                        (331, None),
                    ],
                ),
                ("S2 ic", vec![(350, None), (187, None), (188, None)]),
                ("S3 crap", vec![(314, None), (315, None)]),
                ("S4 prcp", vec![(265, None), (266, None)]),
                ("S5 tvfind", vec![(321, None)]),
                ("S6 vpn-tunnel", vec![(191, None), (192, None)]),
                ("S7 dwt", vec![(196, None)]),
            ]
        );
    }

    #[test]
    fn the_notes_of_a_stream_give_no_number_to_its_chain() {
        let plan = plan_of(RECORDS);
        assert_eq!(
            steps_at(&plan, 2),
            vec![(314, None), (315, None)],
            "the notes of S3 name 265 and 5113, which a reader that hunts numbers takes for issues"
        );
        assert_eq!(
            steps_at(&plan, 0).len(),
            5,
            "the notes of S1 name #341 and PR#342"
        );
        assert_eq!(
            steps_at(&plan, 1).len(),
            3,
            "the notes of S2 name main.rs:1566-1650, 5 commits, and #188"
        );
    }

    #[test]
    fn a_notes_field_of_three_lines_gives_no_number_to_the_chain() {
        // The second line of the notes of S1 holds #341 and the third holds
        // PR#342. A reader of continuation lines takes each of them a second
        // time, which writes the same issue into the chain twice.
        let steps = steps_at(&plan_of(RECORDS), 0);
        assert_eq!(steps.len(), 5);
        let numbers: Vec<u64> = steps
            .iter()
            .flat_map(|(number, closes)| [Some(*number), *closes])
            .flatten()
            .collect();
        let mut once = numbers.clone();
        once.sort_unstable();
        once.dedup();
        assert_eq!(once.len(), numbers.len(), "{numbers:?}");
    }

    #[test]
    fn a_rule_between_two_records_is_not_a_stream() {
        let no_rules: String = RECORDS
            .lines()
            .filter(|line| !line.starts_with('\u{2500}'))
            .collect::<Vec<_>>()
            .join("\n");
        let plan = plan_of(&no_rules);
        assert_eq!(plan.streams().len(), 7);
        assert_eq!(plan, plan_of(RECORDS));
    }

    #[test]
    fn a_record_with_no_stream_field_takes_its_place_as_a_label() {
        let no_names: String = RECORDS
            .lines()
            .filter(|line| !line.starts_with("Stream:"))
            .collect::<Vec<_>>()
            .join("\n");
        let named = plan_of(RECORDS);
        let unnamed = plan_of(&no_names);
        assert_eq!(
            unnamed
                .streams()
                .iter()
                .map(Stream::label)
                .collect::<Vec<_>>(),
            vec![
                "Stream 1", "Stream 2", "Stream 3", "Stream 4", "Stream 5", "Stream 6", "Stream 7",
            ]
        );
        assert_eq!(
            unnamed.streams().iter().map(steps_of).collect::<Vec<_>>(),
            named.streams().iter().map(steps_of).collect::<Vec<_>>()
        );
    }

    #[test]
    fn reads_the_box_drawn_form_of_a_plan() {
        // The third written form, and the one a reader actually holds: the
        // report of the skill, copied out of a terminal.
        assert_eq!(
            shape(&plan_of(BOX_TABLE)),
            vec![
                ("A — visualizers", vec![(15, Some(4)), (7, None)]),
                ("B — audio engine", vec![(11, None), (5, None), (13, None)]),
                ("C — MIDI array", vec![(9, None), (10, None), (12, None)]),
                ("D — manifest", vec![(6, None)]),
            ]
        );
    }

    #[test]
    fn an_order_cell_joins_over_the_lines_its_row_wraps_onto() {
        // The `Order` cell of stream A is `#4 (in flight, PR #15)` on one line
        // and `→ #7` on the next. A reader that takes the second line for a
        // row of its own gives stream A one step and the plan a fifth stream.
        assert_eq!(
            steps_at(&plan_of(BOX_TABLE), 0),
            vec![(15, Some(4)), (7, None)]
        );
    }

    #[test]
    fn a_label_joins_over_the_lines_its_row_wraps_onto() {
        // The label of stream B wraps, so the second line of that row carries
        // `engine` in its first cell and nothing in its `Order` cell. A reader
        // that calls a line with a non-empty first cell a new row loses the
        // word and gains a stream with no chain.
        assert_eq!(
            plan_of(BOX_TABLE)
                .streams()
                .iter()
                .map(Stream::label)
                .collect::<Vec<_>>(),
            vec![
                "A — visualizers",
                "B — audio engine",
                "C — MIDI array",
                "D — manifest",
            ]
        );
    }

    #[test]
    fn a_row_that_wraps_between_a_step_and_its_annotation_stays_one_row() {
        // `PR#343` ends the first line of the `Order` cell of S1, and `(#329)`
        // opens the second. A reader that takes that second line for a row of
        // its own refuses the whole plan, because an annotation annotates the
        // step to its left and this one has none:
        //
        //     wn: stream "grind → grime": "(#329)" stands before any issue
        //     number
        assert_eq!(
            steps_at(&plan_of(NARROW_BOX_TABLE), 0),
            vec![
                (344, Some(341)),
                (343, Some(329)),
                (342, Some(328)),
                (330, None),
                (331, None),
            ]
        );
    }

    #[test]
    fn a_row_that_wraps_between_two_steps_stays_one_row() {
        // The third line of the `Order` cell of S1 is `#330 → #331`, which
        // opens a chain and is one. Nothing in that line says it continues a
        // row, so a reader that asks the line gives the plan an eighth stream
        // that carries no label and two steps that belong to S1.
        assert_eq!(
            shape(&plan_of(NARROW_BOX_TABLE)),
            vec![
                (
                    "S1 gitscratch → grind → grime",
                    vec![
                        (344, Some(341)),
                        (343, Some(329)),
                        (342, Some(328)),
                        (330, None),
                        (331, None),
                    ],
                ),
                ("S2 ic", vec![(350, None), (187, None), (188, None)]),
                ("S3 crap", vec![(314, None), (296, None)]),
                ("S4 prcp", vec![(265, None), (295, None)]),
                ("S5 tvfind", vec![(321, None)]),
                ("S6 vpn-tunnel", vec![(191, None)]),
                ("S7 dwt", vec![(196, None)]),
            ]
        );
    }

    #[test]
    fn a_box_table_with_no_interior_rules_gives_the_same_streams() {
        // A table with no rule between two of its rows is read by its cells
        // instead, so a renderer that draws its outer border alone gives four
        // streams as well. Every wrap of this one says so in its `Order` cell.
        let no_rules: String = BOX_TABLE
            .lines()
            .filter(|line| !line.starts_with('├'))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(plan_of(&no_rules), plan_of(BOX_TABLE));
    }

    #[test]
    fn a_box_table_with_a_header_rule_alone_gives_the_same_streams() {
        // A renderer that draws one rule under the header and none between
        // the rows opens the body of the table with `├─────┼─────┤`. That is
        // a rule and it is no Markdown divider, so the cells still say where
        // each row ends and the wrap of stream B still joins the row above it.
        let mut rules = 0;
        let header_rule_alone: String = BOX_TABLE
            .lines()
            .filter(|line| {
                if !line.starts_with('├') {
                    return true;
                }
                rules += 1;
                rules == 1
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            header_rule_alone.matches('├').count(),
            1,
            "the table keeps the one rule under its header"
        );
        assert_eq!(plan_of(&header_rule_alone), plan_of(BOX_TABLE));
    }

    #[test]
    fn an_ascii_table_gives_the_streams_of_the_box_table() {
        // Not a fourth shape to write code for: `+---+` is a rule line and `|`
        // is a bar, so the two rules that read the box form already read this
        // one.
        assert_eq!(plan_of(&ascii_table()), plan_of(BOX_TABLE));
    }

    #[test]
    fn every_drawing_of_a_rule_is_a_rule() {
        // `is_rule` is asked here, and not a plan that carries the line. A
        // plan hides the answer: a divider that stays reaches the row reader,
        // and the row reader takes a divider for a continuation of the row
        // above it whenever the `Order` cell opens with a hyphen. The test
        // then stays green while the rule it names is broken.
        //
        // A Markdown divider is a rule with spaces in it, and a row of empty
        // cells is a rule with cells in it. The two stand side by side here
        // because one character of drawing is all that parts them.
        let lines: &[(&str, &str, bool)] = &[
            ("a box table opens", "┌─────┬─────┐", true),
            ("a box table divides", "├─────┼─────┤", true),
            ("a box table closes", "└─────┴─────┘", true),
            ("an ascii table", "+-----+-----+", true),
            (
                "a record separator",
                "────────────────────────────────────────",
                true,
            ),
            ("a markdown divider, tight", "|---|---|", true),
            ("a markdown divider, spaced", "| --- | --- |", true),
            ("an alignment colon, tight", "|:---|---:|", true),
            ("an alignment colon, spaced", "|:--- | ---:|", true),
            ("an alignment on both sides", "| :---: | :---: |", true),
            ("an alignment to the left", "| :--- | :--- |", true),
            ("the tail of an ascii arrow", "--", false),
            ("a row of the table", "| S1 | #350 |", false),
            ("a row of empty cells", "|   |   |", false),
        ];
        for (name, line, expected) in lines {
            assert_eq!(
                is_rule(line),
                *expected,
                "{name} is {}a rule, and the line is `{line}`",
                if *expected { "" } else { "no " }
            );
        }
    }

    #[test]
    fn a_divider_row_that_carries_an_alignment_colon_contributes_no_stream() {
        // A divider is a rule line, colons and all. A reader that takes it for
        // a row gives the plan a stream whose `Order` field is `:---:`.
        let plan = plan_of("| Stream | Order |\n| :---: | :---: |\n| S1 | #350 → #187 |");
        assert_eq!(shape(&plan), vec![("S1", vec![(350, None), (187, None)])]);
    }

    #[test]
    fn a_wide_cell_does_not_shift_the_cell_beside_it() {
        // A reader that cuts a row at a column position needs the display
        // width of every character in front of that column, and an em dash is
        // not one column in every font the width tables know. A split on the
        // bar needs no width at all, so this row reads whatever stands in it.
        let notes = format!("{} — {} 日本語", "a".repeat(110), "b".repeat(105));
        assert!(
            UnicodeWidthStr::width(notes.as_str()) >= 220,
            "the fixture is a wide cell, and it is {} columns",
            UnicodeWidthStr::width(notes.as_str())
        );
        let table = format!(
            "| Stream | Order | Notes |\n| --- | --- | --- |\n| S1 | #350 → #187 | {notes} |"
        );
        assert_eq!(
            steps_at(&plan_of(&table), 0),
            vec![(350, None), (187, None)]
        );
    }

    #[test]
    fn the_table_form_gives_the_streams_of_the_record_form() {
        let plan = plan_of(TABLE);
        assert_eq!(plan.streams().len(), 7);
        assert_eq!(plan, plan_of(RECORDS));
    }

    #[test]
    fn a_table_with_no_outer_bars_gives_the_same_streams() {
        let bare: String = TABLE
            .lines()
            .map(|line| {
                line.trim()
                    .trim_start_matches('|')
                    .trim_end_matches('|')
                    .trim()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let plan = plan_of(&bare);
        assert_eq!(plan.streams().len(), 7);
        assert_eq!(plan, plan_of(TABLE));
    }

    #[test]
    fn a_table_of_a_plan_ends_where_the_table_ends() {
        // The empty line under S1 closes the table. Every row below it belongs
        // to another section of the report, and another table is not more work
        // to start.
        assert_eq!(
            shape(&plan_of(TABLE_AND_HOUSEKEEPING)),
            vec![("S1", vec![(344, None)])]
        );
    }

    #[test]
    fn a_later_table_of_prose_leaves_the_plan_alone() {
        // A reader that walks past the end of the table reads the cell under
        // `Action` as an issue number, and one word of a section the plan
        // never named takes the whole plan down.
        assert_eq!(
            shape(&plan_of(TABLE_AND_A_TABLE_OF_PROSE)),
            vec![("S1", vec![(344, None)])]
        );
    }

    #[test]
    fn refuses_the_first_markdown_row_whose_order_cell_is_empty() {
        // A Markdown table writes one row on one line, so no line of it
        // continues another and an empty `Order` cell names no chain. A reader
        // that takes the line for a wrap drops stream A and answers stream B
        // alone, which is a plan short of one stream and no message anywhere.
        assert_eq!(
            parse(MARKDOWN_WITH_AN_EMPTY_FIRST_ORDER),
            Err(PlanError::NoIssues(Snippet::new("A")))
        );
    }

    #[test]
    fn refuses_a_later_markdown_row_whose_order_cell_is_empty() {
        // The same row under a row that holds a chain. A reader that takes it
        // for a wrap joins the two and answers one stream named `A B`, which
        // is a label no line of the table writes.
        assert_eq!(
            parse(MARKDOWN_WITH_AN_EMPTY_SECOND_ORDER),
            Err(PlanError::NoIssues(Snippet::new("B")))
        );
    }

    #[test]
    fn refuses_a_row_whose_cell_count_the_header_does_not_have() {
        // A cell that holds a stray bar is rare, and guessing at it is worse
        // than refusing it: every cell after that bar stands under the wrong
        // column, so the Notes of the row become its Order.
        let row = "| S1 | #350 | src/ic | a note with a | bar |";
        let message = parse(&format!(
            "| Stream | Order | Zone | Notes |\n| --- | --- | --- | --- |\n{row}"
        ))
        .expect_err("a row of five cells is no row of a table of four")
        .to_string();
        assert!(message.contains("5 cells"), "{message}");
        assert!(message.contains("the header has 4"), "{message}");
        assert!(message.contains(row), "{message}");
    }

    #[test]
    fn a_long_row_is_cut_in_the_message() {
        // A row of a box-drawn table is 250 columns wide, and a message that
        // repeats the whole of one hides its own last line.
        let notes = "n".repeat(200);
        let row = format!("| S1 | #350 | src/ic | {notes} | and one cell too many |");
        let message = parse(&format!(
            "| Stream | Order | Zone | Notes |\n| --- | --- | --- | --- |\n{row}"
        ))
        .expect_err("a row of five cells is no row of a table of four")
        .to_string();
        assert!(!message.contains(&notes), "{message}");
        let cut: String = row.chars().take(crate::chain::SNIPPET_CHARS).collect();
        assert!(message.contains(&format!("\"{cut}…\"")), "{message}");
    }

    #[test]
    fn a_group_names_the_issue_the_step_before_it_closes() {
        let steps = steps_at(&plan_of("Order: PR#344 (#341) → #330"), 0);
        assert_eq!(steps, vec![(344, Some(341)), (330, None)]);
    }

    #[test]
    fn a_pr_prefix_is_read_whatever_its_case_and_a_bare_number_is_read() {
        assert_eq!(
            steps_at(
                &plan_of("Order: pr#344 (#341) → Pr#343 → PR342 → 330 → #331"),
                0
            ),
            vec![
                (344, Some(341)),
                (343, None),
                (342, None),
                (330, None),
                (331, None),
            ]
        );
    }

    #[test]
    fn an_annotation_names_the_pull_request_of_the_step_before_it() {
        // `#4 (in flight, PR #15)` is the issue #4, whose work is the pull
        // request #15. The mark says which of the two numbers is the work, so
        // the step holds the pull request and the issue that work closes, the
        // way `PR#344 (#341)` does.
        assert_eq!(
            steps_at(&plan_of("Order: #4 (in flight, PR #15) → #7"), 0),
            vec![(15, Some(4)), (7, None)]
        );
    }

    #[test]
    fn a_glued_mark_inside_an_annotation_marks_the_number_as_well() {
        assert_eq!(
            steps_at(&plan_of("Order: #4 (PR#15)"), 0),
            vec![(15, Some(4))]
        );
    }

    #[test]
    fn an_annotation_that_names_no_issue_is_prose() {
        // `#12 (human)` is one step of one number, and the word is a note the
        // reader wrote for themselves.
        assert_eq!(
            steps_at(&plan_of("Order: #9 → #10 → #12 (human)"), 0),
            vec![(9, None), (10, None), (12, None)]
        );
    }

    #[test]
    fn a_number_of_an_annotation_that_carries_no_hash_is_prose() {
        // The hash is what makes an annotation safe to read. A count of lines
        // carries none, so it stays prose and never reaches the chain.
        assert_eq!(
            steps_at(&plan_of("Order: #4 (30-line window)"), 0),
            vec![(4, None)]
        );
    }

    #[test]
    fn an_annotation_whose_prose_carries_a_parenthesis_is_one_group() {
        // An annotation holds prose, and prose holds parentheses. A reader
        // that closes the group on the first `)` leaves the second one outside
        // it, where a stray parenthesis is a token that names no issue:
        //
        //     wn: stream "Stream 1": ")" is not an issue number
        assert_eq!(
            steps_at(&plan_of("Order: #4 (a note (see the docs)) → #7"), 0),
            vec![(4, None), (7, None)]
        );
    }

    #[test]
    fn a_nested_parenthesis_does_not_hide_the_number_of_an_annotation() {
        // The number of the pair stands after the nested parenthesis, so a
        // reader that closes the group early reads `PR` and `#15)` as two
        // steps of the chain and refuses the plan.
        assert_eq!(
            steps_at(&plan_of("Order: #4 (in flight (rebasing), PR #15)"), 0),
            vec![(15, Some(4))]
        );
    }

    #[test]
    fn a_key_is_read_whatever_its_case() {
        let plan = plan_of("stream: S1\nORDER: #1 → #2");
        assert_eq!(steps_at(&plan, 0), vec![(1, None), (2, None)]);
        assert_eq!(plan, plan_of("Stream: S1\nOrder: #1 → #2"));
    }

    #[test]
    fn an_order_field_of_more_than_one_line_joins_with_one_space() {
        assert_eq!(
            steps_at(&plan_of("Stream: S1\nOrder: #1 → #2 →\n#3"), 0),
            vec![(1, None), (2, None), (3, None)]
        );
    }

    #[test]
    fn a_word_with_a_colon_inside_prose_is_not_a_key() {
        // A key stands first or it is not a key. The word here opens no field,
        // so the line continues the notes and gives no number to the chain.
        let plan = plan_of(
            "Stream: S1\nOrder: #1\nNotes: the plan of a day\nFinish-what-we-started: #99 first",
        );
        assert_eq!(steps_at(&plan, 0), vec![(1, None)]);
    }

    #[test]
    fn an_empty_line_closes_the_field_it_holds_open() {
        // Text after an empty line that opens no field is loose prose, and
        // loose prose is not part of the chain above it.
        let plan = plan_of("Stream: S1\nOrder: #1 → #2\n\n#3 and #4 are notes to myself");
        assert_eq!(steps_at(&plan, 0), vec![(1, None), (2, None)]);
    }

    #[test]
    fn every_number_of_every_stream_arrives_once() {
        assert_eq!(
            numbers_of(&plan_of(RECORDS)),
            vec![
                344, 341, 343, 329, 342, 328, 330, 331, 350, 187, 188, 314, 315, 265, 266, 321,
                191, 192, 196,
            ]
        );
        assert_eq!(
            numbers_of(&plan_of(
                "Stream: A\nOrder: PR#344 (#341) → #330\nStream: B\nOrder: #330 → #341"
            )),
            vec![344, 341, 330],
            "one number of two streams is one number to ask GitHub about"
        );
    }

    #[test]
    fn a_chain_is_not_a_plan() {
        assert!(!looks_like_a_plan("#277 → #278 ∥ #279"));
        assert!(!looks_like_a_plan("#1 || #2"));
        assert!(!looks_like_a_plan(""));
    }

    #[test]
    fn a_stream_field_or_an_order_field_makes_a_plan() {
        assert!(looks_like_a_plan(RECORDS));
        assert!(looks_like_a_plan(TABLE));
        assert!(looks_like_a_plan("Order: #1 → #2"));
        assert!(looks_like_a_plan("  stream: S1"));
        assert!(looks_like_a_plan("| Stream | Order |"));
    }

    #[test]
    fn refuses_a_token_of_an_order_field_that_is_not_a_number() {
        assert_eq!(
            parse("Stream: S1 ic\nOrder: #277 an #278"),
            Err(PlanError::NotAnIssue {
                stream: Snippet::new("S1 ic"),
                token: Snippet::new("an"),
            })
        );
        assert_eq!(
            parse("Stream: S1 ic\nOrder: #277 an #278")
                .expect_err("the word is not an issue number")
                .to_string(),
            "stream \"S1 ic\": \"an\" is not an issue number"
        );
    }

    #[test]
    fn refuses_a_plan_that_holds_no_order_field() {
        let message = parse("Stream: S1 ic\nZone: src/ic\nStream: S2 crap\nZone: src/crap")
            .expect_err("a plan with no chain names nothing to do")
            .to_string();
        assert!(message.contains("no Order field"), "{message}");
        assert_eq!(
            parse("Stream: S1 ic\nZone: src/ic"),
            Err(PlanError::NoOrder)
        );
    }

    #[test]
    fn refuses_one_stream_that_holds_no_order_field() {
        assert_eq!(
            parse("Stream: S1 ic\nOrder: #350\nStream: S2 crap\nZone: src/crap"),
            Err(PlanError::StreamWithoutOrder(Snippet::new("S2 crap")))
        );
    }

    #[test]
    fn refuses_an_order_field_that_holds_no_number() {
        assert_eq!(
            parse("Stream: S1 ic\nOrder:\nZone: src/ic"),
            Err(PlanError::NoIssues(Snippet::new("S1 ic")))
        );
    }

    #[test]
    fn refuses_a_group_that_stands_before_every_step() {
        assert_eq!(
            parse("Stream: S1 ic\nOrder: (#341) → #330"),
            Err(PlanError::UnattachedPair {
                stream: Snippet::new("S1 ic"),
                token: Snippet::new("(#341)"),
            })
        );
    }

    #[test]
    fn refuses_a_group_that_never_closes() {
        // Where an open group ends is a guess, and a guess about a chain is
        // worse than a refusal. The message names the stream and repeats the
        // group back from its parenthesis.
        let message = parse("Stream: S1 ic\nOrder: #4 (in flight")
            .expect_err("a group that never closes is not a chain")
            .to_string();
        assert!(message.contains("S1 ic"), "{message}");
        assert!(message.contains("\"(in flight\""), "{message}");
    }

    #[test]
    fn refuses_a_group_that_never_closes_around_a_nested_one() {
        // A nested parenthesis closes the group it opened and no other, so a
        // group whose own parenthesis never arrives is still a group that
        // never closes. The message repeats it back from the parenthesis that
        // opened it, the nested pair and all.
        //
        // A reader that closes the outer group on the parenthesis of the
        // nested one answers a different message, about the word that stands
        // after it:
        //
        //     wn: stream "S1 ic": "c" is not an issue number
        for order in ["#4 (a (b", "#4 (a (b) c"] {
            let message = parse(&format!("Stream: S1 ic\nOrder: {order}"))
                .expect_err("a group that never closes is not a chain")
                .to_string();
            assert!(message.contains("S1 ic"), "{message}");
            assert!(
                message.contains("has no closing parenthesis"),
                "the order is `{order}` and the message is {message}"
            );
            let group = order
                .split_once(GROUP_OPEN)
                .expect("the order opens a group");
            assert!(
                message.contains(&format!("\"{GROUP_OPEN}{}\"", group.1)),
                "the order is `{order}` and the message is {message}"
            );
        }
    }

    #[test]
    fn a_closing_parenthesis_with_no_group_open_belongs_to_its_token() {
        // Counting the depth of a group must not give a stray parenthesis a
        // group to close. `#4)` is one token, it names no issue, and the
        // message says so and repeats the whole token back.
        assert_eq!(
            parse("Stream: S1 ic\nOrder: #4)"),
            Err(PlanError::NotAnIssue {
                stream: Snippet::new("S1 ic"),
                token: Snippet::new("#4)"),
            })
        );
        assert_eq!(
            parse("Stream: S1 ic\nOrder: #4 (human) → #7)"),
            Err(PlanError::NotAnIssue {
                stream: Snippet::new("S1 ic"),
                token: Snippet::new("#7)"),
            }),
            "the group before it closed, so this parenthesis closes nothing"
        );
    }

    #[test]
    fn refuses_a_second_group_on_one_step() {
        assert_eq!(
            parse("Stream: S1 ic\nOrder: PR#344 (#341) (#329)"),
            Err(PlanError::SecondPair {
                stream: Snippet::new("S1 ic"),
                token: Snippet::new("(#329)"),
            })
        );
        assert_eq!(
            parse("Stream: S1 ic\nOrder: PR#344 (#341 #329)"),
            Err(PlanError::SecondPair {
                stream: Snippet::new("S1 ic"),
                token: Snippet::new("(#329)"),
            })
        );
    }

    #[test]
    fn a_long_stream_label_is_cut_in_the_message() {
        // A plan arrives from the clipboard, and a clipboard holds a page of
        // prose as readily as a label. A message that repeats the whole page
        // hides its own last line.
        let label = "a".repeat(200);
        let message = parse(&format!("Stream: {label}\nOrder: an"))
            .expect_err("the word is not an issue number")
            .to_string();
        let cut: String = label.chars().take(crate::chain::SNIPPET_CHARS).collect();
        assert!(!message.contains(&label), "{message}");
        assert!(message.contains(&format!("\"{cut}…\"")), "{message}");
    }

    #[test]
    fn a_waits_for_column_gives_the_steps_each_stream_waits_for() {
        // The fifth column of a plan. S1 waits for the stream that stands
        // alone, and S2 waits for both of the streams above it.
        let plan = plan_of(WAITS_TABLE);
        assert_eq!(
            shape(&plan),
            vec![
                ("S0 — daemon leak", vec![(96, None)]),
                ("S1 — lifecycle", vec![(91, None)]),
                ("S2 — install", vec![(89, None), (94, None)]),
                ("S3 — keymap", vec![(86, None)]),
            ]
        );
        assert_eq!(waits_at(&plan, 1), vec![(96, None)]);
        assert_eq!(waits_at(&plan, 2), vec![(96, None), (91, None)]);
    }

    #[test]
    fn a_box_drawn_waits_for_column_gives_the_same_waits() {
        // How a table is drawn says nothing about the plan in it, so the box
        // form of these four streams is the Markdown form of them.
        assert_eq!(plan_of(WAITS_BOX_TABLE), plan_of(WAITS_TABLE));
        assert_eq!(
            waits_at(&plan_of(WAITS_BOX_TABLE), 2),
            vec![(96, None), (91, None)]
        );
    }

    #[test]
    fn an_arrow_in_a_waits_for_cell_writes_a_set_and_not_a_chain() {
        // `Order` is a chain and `Waits for` is a set. So an arrow between two
        // blockers names no order between them: the stream waits for both, and
        // it starts when both are finished.
        assert_eq!(
            waits_at(&plan_of(&table_that_waits_for("#96 → #91")), 0),
            vec![(96, None), (91, None)]
        );
        assert_eq!(
            plan_of(&table_that_waits_for("#96 → #91")),
            plan_of(&table_that_waits_for("#96, #91")),
            "the two separators write the same set of blockers"
        );
    }

    #[test]
    fn a_pair_in_a_waits_for_cell_is_one_step() {
        // A stream waits for a piece of work, and one piece of work is
        // sometimes a pull request and the issue that pull request closes.
        // Both spellings of the pair read here as they read in an `Order`
        // cell, because one function reads both cells.
        assert_eq!(
            waits_at(&plan_of(&table_that_waits_for("PR#344 (#341)")), 0),
            vec![(344, Some(341))]
        );
        assert_eq!(
            waits_at(&plan_of(&table_that_waits_for("#4 (in flight, PR #15)")), 0),
            vec![(15, Some(4))]
        );
    }

    #[test]
    fn an_empty_waits_for_cell_gives_a_stream_that_waits_for_nothing() {
        // The common case, and no error. A plan of parallel work is a plan of
        // streams that stand apart, and most of them wait for nothing at all.
        assert!(waits_at(&plan_of(&table_that_waits_for("")), 0).is_empty());
        assert!(
            waits_at(&plan_of(&table_that_waits_for("   ")), 0).is_empty(),
            "a drawn cell is padded to the width of its column"
        );
    }

    #[test]
    fn a_plan_with_no_waits_for_column_reads_the_way_it_always_did() {
        // The fifth column is new, and every plan written before it stands.
        // The report of the skill names no blocker at all, so it holds the
        // four streams it always held and each one waits for nothing.
        let plan = plan_of(BOX_TABLE);
        assert_eq!(
            shape(&plan),
            vec![
                ("A — visualizers", vec![(15, Some(4)), (7, None)]),
                ("B — audio engine", vec![(11, None), (5, None), (13, None)]),
                ("C — MIDI array", vec![(9, None), (10, None), (12, None)]),
                ("D — manifest", vec![(6, None)]),
            ]
        );
        for form in [BOX_TABLE, TABLE, RECORDS, NARROW_BOX_TABLE] {
            assert!(
                plan_of(form)
                    .streams()
                    .iter()
                    .all(|stream| waits_of(stream).is_empty()),
                "no stream of this plan names a blocker"
            );
        }
    }

    #[test]
    fn refuses_a_row_that_carries_no_waits_for_cell() {
        // A header of five columns and a row of four put every cell after the
        // missing one under the wrong column, so the `Zone` of the row would
        // be the work it waits for. The width of the row answers first, and it
        // repeats the row back.
        let row = "| S1 | #1 | src/a | fine |";
        assert_eq!(
            parse(&format!(
                "| Stream | Order | Waits for | Zone | Notes |\n\
                 | --- | --- | --- | --- | --- |\n\
                 {row}"
            )),
            Err(PlanError::RowWidth {
                cells: 4,
                header: 5,
                line: Snippet::new(row),
            })
        );
    }

    #[test]
    fn the_numbers_of_a_plan_hold_a_blocker_that_stands_in_no_order_field() {
        // One query answers the whole plan, so every number of the plan is in
        // this list. A blocker is sometimes the work of no stream of the plan,
        // and a reader that walks the chains alone leaves that number out. The
        // answer then says nothing about the work a stream waits for.
        assert_eq!(
            numbers_of(&plan_of(&table_that_waits_for("#96"))),
            vec![1, 96]
        );
        assert_eq!(
            numbers_of(&plan_of(&table_that_waits_for("PR#344 (#341)"))),
            vec![1, 344, 341],
            "the number of a step comes before the number the step closes"
        );
        assert_eq!(
            numbers_of(&plan_of(WAITS_TABLE)),
            vec![96, 91, 89, 94, 86],
            "a blocker that is the work of another stream arrives once"
        );
    }

    #[test]
    fn the_record_form_gives_the_waits_of_the_table_form() {
        // The record form writes the column as a field, and the two forms give
        // one plan. A `Waits for` field opens no stream of its own, the way a
        // `Zone` field opens none: the record it stands in already has a name
        // and a chain.
        let plan = plan_of(WAITS_RECORDS);
        assert_eq!(plan.streams().len(), 4);
        assert_eq!(waits_at(&plan, 1), vec![(96, None)]);
        assert_eq!(waits_at(&plan, 2), vec![(96, None), (91, None)]);
        assert_eq!(plan, plan_of(WAITS_TABLE));
    }

    #[test]
    fn a_record_with_no_waits_for_field_waits_for_nothing() {
        // An absent field and an empty one say the same thing, the way an
        // absent column and an empty cell do.
        assert!(waits_at(&plan_of("Stream: S1\nOrder: #1"), 0).is_empty());
        assert!(waits_at(&plan_of(&record_that_waits_for("Waits for", "")), 0).is_empty());
    }

    #[test]
    fn a_waits_for_key_is_read_whatever_its_case() {
        // The four keys of a record are read with the case ignored, and the
        // fifth one is read the same way.
        let plan = plan_of(&record_that_waits_for("Waits for", "#96"));
        assert_eq!(waits_at(&plan, 0), vec![(96, None)]);
        assert_eq!(plan, plan_of(&record_that_waits_for("waits for", "#96")));
        assert_eq!(plan, plan_of(&record_that_waits_for("WAITS FOR", "#96")));
    }

    #[test]
    fn refuses_a_waits_for_cell_that_names_no_issue() {
        // The reason a stream waits is prose, and the prose of a stream
        // belongs in `Notes`. So a cell of prose here names no work to wait
        // for, and it earns the message a bad `Order` cell earns: the stream,
        // and the word.
        assert_eq!(
            parse(&table_that_waits_for("after the leak lands")),
            Err(PlanError::NotAnIssue {
                stream: Snippet::new(ONE_STREAM_LABEL),
                token: Snippet::new("after"),
            })
        );
    }
}
