//! Reading a plan of parallel work out of a page of text.
//!
//! A chain says one order: do these issues, one after the other. A plan says
//! several orders side by side, one for each part of the repository that a
//! reader can work in without walking into the work of a neighbor. Such a plan
//! is written as a record for each stream, or as one table row for each stream.
//! This module reads both, and it gives back the same streams either way.
//!
//! # Why only the `Order` field holds a chain
//!
//! A stream carries prose as well as a chain. The prose is about code, and
//! prose about code is full of numbers: `main.rs:1566-1650` names two lines of
//! a file, and `265 lines apart in a 5113-line file` names a distance and a
//! length. None of them is an issue.
//!
//! So this module reads the `Order` field and it reads nothing else. `Stream`,
//! `Zone`, and `Notes` never give a number to a chain. A reader who writes a
//! note about line 5113 gets the issues of the plan, and not issue 5113.
//!
//! # The pair
//!
//! A step of a plan is one piece of work, and one piece of work is sometimes
//! two numbers: `PR#344 (#341)` is a pull request that closes an issue. The
//! step holds both, because the state of the work is the state of the pull
//! request and the reader still wants to see which issue it finishes.

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

/// The character that stands between two cells of a table.
const TABLE_BAR: char = '|';

/// The characters a table draws the rule under its header with.
const DELIMITER_CHARS: &[char] = &['-', ':', ' '];

/// The character that opens the group of a pair.
const GROUP_OPEN: char = '(';

/// The character that closes the group of a pair.
const GROUP_CLOSE: char = ')';

/// The prefix a plan writes before the number of a pull request.
///
/// It carries no meaning for this module, because GitHub numbers a pull
/// request out of the same series as an issue. It is read and dropped so a
/// plan written the way a reader reads it is a plan this module reads too.
const PULL_REQUEST_PREFIX: &str = "pr";

/// The word a stream with no `Stream` field takes as the first half of its
/// label. The second half is the place of the stream in the plan.
const UNNAMED_LABEL: &str = "Stream";

/// The lowest number of characters a line of rule holds.
///
/// Two characters are an arrow (`--`) or the start of a word. Three are a rule.
const RULE_CHARS: usize = 3;

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
    #[must_use]
    pub fn numbers(&self) -> Vec<IssueNumber> {
        let mut numbers: Vec<IssueNumber> = Vec::new();
        for step in self.streams.iter().flat_map(Stream::steps) {
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
    /// A second group stands on one step, and a step closes one issue.
    #[error("stream {stream:?}: {token:?} is a second issue for one step")]
    SecondPair {
        /// The label of the stream that holds the group.
        stream: Snippet,
        /// The group itself.
        token: Snippet,
    },
}

/// The four fields a stream of a plan is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    /// The name of the stream.
    Stream,
    /// The chain of the stream. The one field this module reads for numbers.
    Order,
    /// The part of the repository the stream works in.
    Zone,
    /// The prose of the stream.
    Notes,
}

impl Key {
    /// The four keys, to read a line against.
    const ALL: [Self; 4] = [Self::Stream, Self::Order, Self::Zone, Self::Notes];

    /// The word the key is written with.
    fn word(self) -> &'static str {
        match self {
            Self::Stream => "Stream",
            Self::Order => "Order",
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
/// [`PlanError::NotAnIssue`] for a token of an `Order` field that names no
/// issue, and [`PlanError::UnattachedPair`] or [`PlanError::SecondPair`] for a
/// group that attaches to no step or to a step that already holds one.
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
/// The two fields this module reads. `Zone` and `Notes` open a field and close
/// the field before it, and the text of them goes nowhere.
#[derive(Default)]
struct Record {
    /// The text of the `Stream` field, when the record holds one.
    label: Option<String>,
    /// The text of the `Order` field, when the record holds one.
    order: Option<String>,
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
        .map(|(place, record)| stream_of(record.label.as_deref(), place, record.order.as_deref()))
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
            Key::Zone | Key::Notes => false,
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

/// Is `line` a rule a reader draws between two streams?
///
/// A rule carries no field and no prose. Three characters at the least,
/// because `--` is the tail of an arrow and a rule is a line.
fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().count() >= RULE_CHARS && trimmed.chars().all(is_rule_char)
}

/// Is `c` a character a reader draws a rule with?
///
/// The box-drawing block holds every line and every corner, and the four marks
/// beside it are the ones a keyboard writes: the hyphen, the equals sign, the
/// underscore, and the two dashes a word processor writes for a hyphen.
fn is_rule_char(c: char) -> bool {
    matches!(
        c,
        '\u{2500}'..='\u{257f}' | '-' | '=' | '_' | '\u{2013}' | '\u{2014}'
    )
}

/// The row of `text` that names the columns of a table.
///
/// Gives the line the body of the table starts on, and the cells of the header
/// itself. A row that names a `Stream` column or an `Order` column is the
/// header, because those are the two columns this module reads.
fn find_header(text: &str) -> Option<(usize, Vec<&str>)> {
    text.lines().enumerate().find_map(|(place, line)| {
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
/// Gives [`PlanError::NoOrder`] for a table with no `Order` column, and the
/// errors of [`stream_of`] for one row of it.
fn table_streams(text: &str, body: usize, header: &[&str]) -> Result<Vec<Stream>, PlanError> {
    let order_at = column_of(header, Key::Order).ok_or(PlanError::NoOrder)?;
    let stream_at = column_of(header, Key::Stream);
    let mut streams: Vec<Stream> = Vec::new();
    for line in text.lines().skip(body) {
        let Some(cells) = table_cells(line) else {
            continue;
        };
        if cells.iter().all(|cell| cell.is_empty()) || is_delimiter(&cells) {
            continue;
        }
        let named = stream_at.and_then(|at| cells.get(at)).copied();
        let stream = stream_of(named, streams.len(), cells.get(order_at).copied())?;
        streams.push(stream);
    }
    Ok(streams)
}

/// The cells of `line`, or `None` when `line` is no row of a table.
///
/// The bar of a table is the bar of a chain as well, so a row is read by its
/// cells and never by its shape: `#1 || #2` holds two bars and names no
/// column, and it is a chain.
fn table_cells(line: &str) -> Option<Vec<&str>> {
    if !line.contains(TABLE_BAR) {
        return None;
    }
    let mut cells: Vec<&str> = line.split(TABLE_BAR).map(str::trim).collect();
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

/// Is this row the rule a table draws under its header?
fn is_delimiter(cells: &[&str]) -> bool {
    cells
        .iter()
        .all(|cell| !cell.is_empty() && cell.chars().all(|c| DELIMITER_CHARS.contains(&c)))
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
/// the stream in the plan, and `order` is the text of the `Order` field or
/// cell.
///
/// # Errors
///
/// Gives [`PlanError::StreamWithoutOrder`] when the stream names no `Order`
/// field at all, and the errors of [`read_order`] for the chain in one.
fn stream_of(named: Option<&str>, place: usize, order: Option<&str>) -> Result<Stream, PlanError> {
    let label = label_of(named, place);
    let order = order.ok_or_else(|| PlanError::StreamWithoutOrder(Snippet::new(&label)))?;
    let steps = read_order(order, &label)?;
    Ok(Stream { label, steps })
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
/// [`PlanError::SecondPair`] for a second group on one step, and
/// [`PlanError::NoIssues`] for a field with no number in it.
fn read_order(order: &str, label: &str) -> Result<Vec<Step>, PlanError> {
    let mut steps: Vec<Step> = Vec::new();
    for piece in pieces(order) {
        match piece {
            Piece::Step(token) => {
                let number = step_number(&token).ok_or_else(|| PlanError::NotAnIssue {
                    stream: Snippet::new(label),
                    token: Snippet::new(&token),
                })?;
                // Through the constructor, so one place builds a step. A
                // group that follows attaches to it below.
                steps.push(Step::new(number, None));
            }
            Piece::Group(token) => {
                // The group is repeated back with its parentheses, because
                // that is how the reader wrote it and how they find it again.
                let written = format!("{GROUP_OPEN}{token}{GROUP_CLOSE}");
                let number = step_number(&token).ok_or_else(|| PlanError::NotAnIssue {
                    stream: Snippet::new(label),
                    token: Snippet::new(&written),
                })?;
                let step = steps.last_mut().ok_or_else(|| PlanError::UnattachedPair {
                    stream: Snippet::new(label),
                    token: Snippet::new(&written),
                })?;
                if step.closes.is_some() {
                    return Err(PlanError::SecondPair {
                        stream: Snippet::new(label),
                        token: Snippet::new(&written),
                    });
                }
                step.closes = Some(number);
            }
        }
    }
    if steps.is_empty() {
        return Err(PlanError::NoIssues(Snippet::new(label)));
    }
    Ok(steps)
}

/// One piece of an `Order` field.
enum Piece {
    /// A step of the stream, as the field writes it.
    Step(String),
    /// A group: the issue the step before it closes.
    Group(String),
}

/// Cut `order` into its steps and its groups.
///
/// Whitespace, a separator of a chain, and a parenthesis each end a token. So
/// does a `#` that arrives while a token is open, which is what makes `#1#2`
/// two steps written with no separator at all. A `#` that arrives on the `PR`
/// of a pull request ends nothing, because `PR#344` is one number written the
/// way a plan writes it.
fn pieces(order: &str) -> Vec<Piece> {
    let mut pieces: Vec<Piece> = Vec::new();
    let mut token = String::new();
    let mut in_group = false;
    for c in order.chars() {
        if c == GROUP_OPEN || c == GROUP_CLOSE {
            end_token(&mut token, in_group, &mut pieces);
            in_group = c == GROUP_OPEN;
            continue;
        }
        if c.is_whitespace() || SEPARATORS.contains(&c) {
            end_token(&mut token, in_group, &mut pieces);
            continue;
        }
        if c == HASH && !token.is_empty() && !token.eq_ignore_ascii_case(PULL_REQUEST_PREFIX) {
            end_token(&mut token, in_group, &mut pieces);
        }
        token.push(c);
    }
    end_token(&mut token, in_group, &mut pieces);
    pieces
}

/// Put the token that is open into `pieces`, and open a new one.
fn end_token(token: &mut String, in_group: bool, pieces: &mut Vec<Piece>) {
    if token.is_empty() {
        return;
    }
    let text = std::mem::take(token);
    pieces.push(if in_group {
        Piece::Group(text)
    } else {
        Piece::Step(text)
    });
}

/// The issue number `token` names, or `None` when it names none.
fn step_number(token: &str) -> Option<IssueNumber> {
    read_number(without_pull_request_prefix(token))
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
}
