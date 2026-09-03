//! Painting a [`Report`] into the block of text the reader sees.
//!
//! The block has two parts. Every issue of the chain gets one row, in the
//! order the chain wrote it, so the reader can check the plan they typed
//! against the plan GitHub holds. Under the rows stands the answer, and the
//! answer names the command that starts the work.
//!
//! # The command comes in as an argument
//!
//! Every function here takes what it needs, the start command included. This
//! module reads nothing from the environment, so a test of it calls
//! [`render`] and depends on no process-global state.
//!
//! # One row never wraps
//!
//! A title is the one piece of a row with no bound on its length, and a row
//! that wraps costs two lines where the second one carries no number. So the
//! title is cut to the columns the row has left, through
//! [`textfit::truncate_to_budget`], which gives an empty title rather than a
//! marker that is itself one column too wide.
//!
//! A summary line of a plan is the one line that may wrap, and only in a
//! window too narrow to hold the shortest label of a stream beside the answer.
//! See [`summary`].
//!
//! # A plan is one block for each stream
//!
//! A plan of parallel work holds many streams, and [`render_plan`] paints one
//! block for each of them. A block carries no answer of its own: the summary
//! under the last block names the issue to start in every stream, so the
//! reader reads the answers together and picks the stream they want.
//!
//! # A picture is one block with one column more
//!
//! A picture joins two streams, so the row over a row is not the work that row
//! waits for. [`render_graph`] paints one block with a last column that names
//! that work, and the answer under it names one command for each step somebody
//! can start now.
//!
//! That column takes its columns out of the window before the title does. It
//! is the one thing a reader of a blocked row came for, and a title is text
//! the reader can already read in the picture they pasted. So the title is cut
//! to what the column leaves, and the row still fits the window.

use colored::{ColoredString, Colorize};
use textfit::{pad_right, truncate_to_budget};
use unicode_width::UnicodeWidthStr;

use crate::chain::{list, IssueNumber};
use crate::report::{Entry, Report, Status};
use crate::StartCommand;

/// The mark of an issue whose work is done.
const MARK_DONE: char = '✓';
/// The mark of an issue that was closed without the work being done.
const MARK_DROPPED: char = '⊘';
/// The mark of the issue to start.
const MARK_NEXT: char = '→';
/// The mark of an issue that is open and stands behind the one to start.
const MARK_LATER: char = '·';
/// The mark of a number the repository does not have.
const MARK_MISSING: char = '?';

/// The columns between the number and the title.
const COLUMN_GAP: usize = 2;

/// The words that open the last column of a row of a picture.
const WAITS_FOR: &str = "waits for ";

/// What stands between two numbers of that column.
///
/// A comma, and not the `and` of [`list`], because the column is a list and a
/// sentence stands in a note.
const NUMBER_SEPARATOR: &str = ", ";

/// The last column of a row of one line of work.
///
/// A chain and a stream write no such column: the row above a row is the work
/// it waits for, so the reader reads the line above it and needs no list.
const NO_WAITS: &str = "";

/// The columns a row of one line of work pads its title to.
///
/// Nothing stands after the title of such a row, so the title is padded to
/// nothing and the row ends where the title ends.
const NO_TITLE_WIDTH: usize = 0;

/// The columns the mark of a row occupies. Every mark this module writes is
/// one column wide.
const MARK_WIDTH: usize = 1;

/// The title of a row whose number names no issue.
const MISSING_TITLE: &str = "(no such issue)";

/// The columns a row of a plan stands in from the left edge. A block reads as
/// one thing under the label of its stream, and the indent is what makes it
/// one thing.
const PLAN_INDENT: usize = 2;

/// The line that opens the summary of a plan.
const SUMMARY_HEADING: &str = "Take one from each stream:";

/// The columns the label of a summary line keeps, however narrow the window
/// is. This is the width of `Stream 1`, the name `plan::label_of` gives a
/// stream the plan does not name, and thus the shortest label the tool itself
/// ever writes.
const MIN_LABEL_WIDTH: usize = 8;

/// The answer of a stream where every step is finished.
const EVERY_ISSUE_CLOSED: &str = "every issue is closed";

/// The answer of a stream where nothing is open and something could not be
/// read.
const NO_ISSUE_OPEN: &str = "no issue is open";

/// The answer of a picture where every step is finished.
const GRAPH_CLOSED: &str = "Every issue in the graph is closed. Nothing to start.";

/// The answer of a picture where nothing is open and something could not be
/// read.
const GRAPH_NOT_OPEN: &str = "No issue in the graph is open.";

/// The answer of a picture that holds an open step and no step to start.
///
/// Every open step of such a picture waits for work that is missing or
/// dropped. The sentence says so, because a silent "nothing to start" reads as
/// "the plan is done", which is the opposite of the truth.
const GRAPH_NOT_READY: &str = concat!(
    "No issue in the graph is ready. ",
    "Every open issue waits for work that is not finished.",
);

/// Paint the chain, the notes it earns, and the answer.
///
/// `repo` names the repository the states came from, and appears only in the
/// note about a number that repository does not have. `width` is the columns
/// the block has to fit in. `start` is the command the answer names, because
/// the answer is only useful if the next thing to type is on the screen.
#[must_use]
pub fn render(report: &Report, repo: &str, width: usize, start: &StartCommand) -> String {
    let entries = report.entries();
    if entries.is_empty() {
        return String::new();
    }

    let number_width = entries
        .iter()
        .map(|entry| UnicodeWidthStr::width(entry.label().as_str()))
        .max()
        .unwrap_or(0);

    let mut lines: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(position, entry)| {
            row(
                entry,
                report.is_ready(position),
                number_width,
                width,
                NO_WAITS,
                NO_TITLE_WIDTH,
            )
        })
        .collect();

    lines.push(String::new());
    lines.extend(notes(report, repo));
    lines.push(answer(report, start));
    lines.join("\n")
}

/// How one row is marked and painted, which is what its state decides.
struct Style {
    mark: char,
    /// The color of the mark.
    paint_mark: fn(&str) -> ColoredString,
    /// The color of the number and of the title.
    paint_text: fn(&str) -> ColoredString,
}

/// The style of one row. `is_next` is what parts the issue to start from the
/// open issues that stand behind it.
fn style(status: Status, is_next: bool) -> Style {
    match status {
        Status::Done => Style {
            mark: MARK_DONE,
            paint_mark: |s| s.green(),
            paint_text: |s| s.dimmed(),
        },
        Status::Dropped => Style {
            mark: MARK_DROPPED,
            paint_mark: |s| s.yellow(),
            paint_text: |s| s.dimmed(),
        },
        Status::Missing => Style {
            mark: MARK_MISSING,
            paint_mark: |s| s.red().bold(),
            paint_text: |s| s.red(),
        },
        Status::Open if is_next => Style {
            mark: MARK_NEXT,
            paint_mark: |s| s.yellow().bold(),
            paint_text: |s| s.bold(),
        },
        Status::Open => Style {
            mark: MARK_LATER,
            paint_mark: |s| s.dimmed(),
            paint_text: |s| s.normal(),
        },
    }
}

/// The columns a row spends before its title: the mark, the space after it,
/// the number column, and the gap after that.
///
/// A block that measures its titles asks the same question a row asks, so both
/// of them ask it here. Two answers to it would let the titles of a block and
/// the rows of that block part company.
fn title_start(number_width: usize) -> usize {
    MARK_WIDTH + 1 + number_width + COLUMN_GAP
}

/// The title a row writes, cut to `budget` columns.
///
/// A row whose number names no issue writes [`MISSING_TITLE`], because a row
/// that carried no title there would say nothing about the number beside it.
/// The cut lives here and not at each call, so a block that measures its
/// titles measures the same text its rows write.
fn fitted_title(entry: &Entry, budget: usize) -> String {
    let title = if entry.status == Status::Missing {
        MISSING_TITLE
    } else {
        entry.title.as_str()
    };
    truncate_to_budget(title, budget)
}

/// One row: the mark, the number, as much of the title as the width holds, and
/// what the step waits for.
///
/// A row that has no columns left for a title ends at the number, rather than
/// in the spaces that would have stood before one.
///
/// The number a row writes is [`Entry::label`], so a step of a plan that names
/// a pull request and the issue it closes writes both. The width of the column
/// comes out of the same call, and the two can never part company.
///
/// `waits` is the text of the last column, and `title_width` is the columns
/// the title is padded to so that column lines up. One line of work waits for
/// the row above it, so a chain and a stream pass an empty text and no width,
/// and the row then ends at its title. Two shapes of input read one function
/// here, because two functions that paint one row drift apart.
///
/// `width` is the columns the mark, the number, and the title have. The caller
/// takes the columns of the last column out of the window first, so a row that
/// names what it waits for still fits the window.
fn row(
    entry: &Entry,
    is_next: bool,
    number_width: usize,
    width: usize,
    waits: &str,
    title_width: usize,
) -> String {
    let style = style(entry.status, is_next);
    let number = entry.label();
    let mark = (style.paint_mark)(&style.mark.to_string());

    let title = fitted_title(entry, width.saturating_sub(title_start(number_width)));

    if title.is_empty() && waits.is_empty() {
        return format!("{mark} {}", (style.paint_text)(&number));
    }
    let number = (style.paint_text)(&pad_right(&number, number_width));
    let gap = " ".repeat(COLUMN_GAP);
    if waits.is_empty() {
        return format!("{mark} {number}{gap}{}", (style.paint_text)(&title));
    }
    if title.is_empty() {
        // The window holds no columns for a title, so the gap after the number
        // is the one gap of the row and the reader still reads what the step
        // waits for.
        return format!("{mark} {number}{gap}{}", waits.dimmed());
    }
    let pad = " ".repeat(title_width.saturating_sub(UnicodeWidthStr::width(title.as_str())));
    format!(
        "{mark} {number}{gap}{}{pad}{gap}{}",
        (style.paint_text)(&title),
        waits.dimmed()
    )
}

/// The lines between the rows and the answer: what the chain says that the
/// answer alone does not.
fn notes(report: &Report, repo: &str) -> Vec<String> {
    let mut notes = Vec::new();

    let missing = report.missing();
    if !missing.is_empty() {
        let verb = if missing.len() == 1 { "is" } else { "are" };
        notes.push(
            format!("{} {verb} not in {repo}.", list(&missing))
                .red()
                .to_string(),
        );
    }

    notes.extend(
        report
            .pairs_that_disagree()
            .into_iter()
            .filter_map(|entry| {
                entry.closes.map(|closes| {
                    format!(
                        "{} is {} and {} is {}.",
                        entry.number,
                        word(entry.status),
                        closes.number,
                        word(closes.status)
                    )
                    .yellow()
                    .to_string()
                })
            }),
    );

    let early = report.finished_out_of_order();
    if !early.is_empty() {
        let verb = if early.len() == 1 { "is" } else { "are" };
        notes.push(
            format!("{} {verb} already closed, out of order.", list(&early))
                .yellow()
                .to_string(),
        );
    }

    notes
}

/// The word a sentence writes for one state.
///
/// A note reads as a sentence, so a state stands in it as a word and not as
/// the mark of a row. [`Status::Missing`] never reaches the note about a pair,
/// because [`Report::pairs_that_disagree`] drops a step whose state nobody
/// knows, and the red note about that number already tells the reader to look.
fn word(status: Status) -> &'static str {
    match status {
        Status::Open => "open",
        Status::Done => "closed",
        Status::Dropped => "closed without the work being done",
        Status::Missing => "not in the repository",
    }
}

/// The answer: the issue to start and the command that starts it.
fn answer(report: &Report, start: &StartCommand) -> String {
    let Some(entry) = report.next_entry() else {
        return if report
            .entries()
            .iter()
            .all(|entry| entry.status.is_finished())
        {
            "Every issue in the chain is closed. Nothing to start."
                .dimmed()
                .to_string()
        } else {
            // Nothing is open and something is not an issue at all, so the
            // chain is not finished. Saying it is would be a guess about the
            // number nobody could read.
            "No issue in the chain is open.".dimmed().to_string()
        };
    };
    start_line(entry.number, start)
}

/// The sentence that names one issue to start, and the command that starts it.
///
/// A chain names one such issue, and a picture names one for each stream that
/// is ready. Both write this sentence, so a reader who learned it on a chain
/// reads the answer of a picture without learning a second one.
fn start_line(number: IssueNumber, start: &StartCommand) -> String {
    format!(
        "Start {} next with '{}'",
        number.to_string().bold(),
        command(start, number).cyan().bold()
    )
}

/// Paint a plan drawn as a picture: the rows, the notes they earn, and the
/// answer.
///
/// `repo`, `width`, and `start` mean what they mean in [`render`].
///
/// A picture names at least two steps, because a net that joins fewer of them
/// claims no text. So this function paints no empty block, and the rows always
/// stand over the answer.
#[must_use]
#[allow(
    dead_code,
    reason = "the run of a picture calls this in the slice that answers a graph"
)]
pub fn render_graph(report: &Report, repo: &str, width: usize, start: &StartCommand) -> String {
    let entries = report.entries();
    let number_width = entries
        .iter()
        .map(|entry| UnicodeWidthStr::width(entry.label().as_str()))
        .max()
        .unwrap_or(0);

    let waits: Vec<String> = (0..entries.len())
        .map(|position| waits_text(report.waits_for(position)))
        .collect();

    // The last column takes its columns out of the window first, and the
    // titles are then cut to what is left. A block whose rows all wait for
    // nothing keeps the whole window, gap and all.
    let waits_width = waits
        .iter()
        .map(|text| UnicodeWidthStr::width(text.as_str()))
        .max()
        .unwrap_or(0);
    let row_width = if waits_width == 0 {
        width
    } else {
        width.saturating_sub(COLUMN_GAP + waits_width)
    };

    let budget = row_width.saturating_sub(title_start(number_width));
    let title_width = entries
        .iter()
        .map(|entry| UnicodeWidthStr::width(fitted_title(entry, budget).as_str()))
        .max()
        .unwrap_or(0);

    let mut lines: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(position, entry)| {
            row(
                entry,
                report.is_ready(position),
                number_width,
                row_width,
                &waits[position],
                title_width,
            )
        })
        .collect();

    lines.push(String::new());
    lines.extend(notes(report, repo));
    lines.extend(graph_answer(report, start));
    lines.join("\n")
}

/// The last column of one row: the numbers the step waits for.
///
/// A step that waits for nothing gives an empty text, and the row then ends at
/// its title. Every other step gives `waits for #247, #248`, which is the
/// question a reader of a blocked row asks and the only place the answer of a
/// picture holds it.
fn waits_text(numbers: &[IssueNumber]) -> String {
    if numbers.is_empty() {
        return String::new();
    }
    let written: Vec<String> = numbers.iter().map(ToString::to_string).collect();
    format!("{WAITS_FOR}{}", written.join(NUMBER_SEPARATOR))
}

/// The answer of a picture: one line for each step somebody can start now.
///
/// The lines stand in the order of the rows, so a reader who read the rows
/// reads the answers in the same order and finds the row of each of them.
fn graph_answer(report: &Report, start: &StartCommand) -> Vec<String> {
    let ready: Vec<String> = report
        .entries()
        .iter()
        .enumerate()
        .filter(|(position, _)| report.is_ready(*position))
        .map(|(_, entry)| start_line(entry.number, start))
        .collect();
    if ready.is_empty() {
        return vec![nothing_to_start(report)];
    }
    ready
}

/// What the answer of a picture says when no step of it is ready.
///
/// A picture with every step finished is a plan somebody finished, and the
/// answer says so. A picture that still holds an open step says why nobody
/// starts it, because every one of those steps waits for work that is missing
/// or dropped. A picture with nothing open and a step nobody could read is not
/// finished, and saying it is would be a guess about the number nobody could
/// read.
fn nothing_to_start(report: &Report) -> String {
    let entries = report.entries();
    if entries.iter().all(|entry| entry.status.is_finished()) {
        return GRAPH_CLOSED.dimmed().to_string();
    }
    if entries.iter().any(|entry| entry.status.is_open()) {
        return GRAPH_NOT_READY.dimmed().to_string();
    }
    GRAPH_NOT_OPEN.dimmed().to_string()
}

/// One stream of a plan, painted as one block.
///
/// The label and the report arrive together because the block writes both: the
/// label heads the block, and the same label stands in the summary line of that
/// stream. A stream that carried its label somewhere else would let the two
/// part company.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamReport {
    /// The label the plan gave the stream, such as `S1 gitscratch → grime`.
    pub label: String,
    /// What GitHub says about every step of that stream.
    pub report: Report,
}

/// Paint a plan: one block for each stream, and one summary under them all.
///
/// `repo`, `width`, and `start` mean what they mean in [`render`].
///
/// A stream block carries no answer of its own. The summary holds the answer
/// of every stream in one place, so the reader reads it once and picks the
/// stream they want.
#[must_use]
pub fn render_plan(
    streams: &[StreamReport],
    repo: &str,
    width: usize,
    start: &StartCommand,
) -> String {
    if streams.is_empty() {
        return String::new();
    }

    let mut lines: Vec<String> = Vec::new();
    for stream in streams {
        lines.push(truncate_to_budget(&stream.label, width).bold().to_string());
        lines.extend(block(&stream.report, repo, width));
        // The blank line parts this block from the next one, and the last one
        // from the summary. The text ends at the summary, so no blank line
        // stands at the end of it.
        lines.push(String::new());
    }
    lines.push(SUMMARY_HEADING.to_string());
    lines.extend(summary(streams, width, start));
    lines.join("\n")
}

/// The rows of one stream and the notes they earn, indented under the label of
/// that stream.
///
/// The width of the number column is the width of the widest label of this
/// stream alone. Each block thus lines up under itself, and one stream that
/// names a pair does not push the numbers of every other stream to the right.
fn block(report: &Report, repo: &str, width: usize) -> Vec<String> {
    let entries = report.entries();
    let number_width = entries
        .iter()
        .map(|entry| UnicodeWidthStr::width(entry.label().as_str()))
        .max()
        .unwrap_or(0);
    let row_width = width.saturating_sub(PLAN_INDENT);

    let mut lines: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(position, entry)| {
            indent(&row(
                entry,
                report.is_ready(position),
                number_width,
                row_width,
                NO_WAITS,
                NO_TITLE_WIDTH,
            ))
        })
        .collect();

    let notes = notes(report, repo);
    if !notes.is_empty() {
        lines.push(String::new());
        lines.extend(notes.iter().map(|note| indent(note)));
    }
    lines
}

/// Move one line in under the label it belongs to.
fn indent(line: &str) -> String {
    format!("{}{line}", " ".repeat(PLAN_INDENT))
}

/// The answer of one stream, as the summary writes it.
enum Tail {
    /// The stream holds an open issue, and this is the number to start.
    Next(IssueNumber),
    /// The stream names nothing to start, and this says why.
    Nothing(&'static str),
}

impl Tail {
    /// The answer one stream gives.
    fn of(report: &Report) -> Self {
        match report.next_entry() {
            Some(entry) => Self::Next(entry.number),
            None if report
                .entries()
                .iter()
                .all(|entry| entry.status.is_finished()) =>
            {
                Self::Nothing(EVERY_ISSUE_CLOSED)
            }
            // Nothing is open and something is not an issue at all, so the
            // stream is not finished. Saying it is would be a guess about the
            // number nobody could read.
            None => Self::Nothing(NO_ISSUE_OPEN),
        }
    }

    /// The columns `→ #344` occupies, for a stream that names an issue.
    ///
    /// The widest of these is what every such tail is padded to, so the
    /// commands of the summary stand in one column.
    fn mark_width(&self) -> Option<usize> {
        match self {
            Self::Next(number) => Some(UnicodeWidthStr::width(marked(*number).as_str())),
            Self::Nothing(_) => None,
        }
    }

    /// The columns the whole tail occupies, once `→ #344` is padded to
    /// `mark_width`. This is what the label of a summary line gives way to,
    /// as far as [`MIN_LABEL_WIDTH`].
    fn width(&self, mark_width: usize, start: &StartCommand) -> usize {
        match self {
            Self::Next(number) => {
                mark_width + COLUMN_GAP + UnicodeWidthStr::width(command(start, *number).as_str())
            }
            Self::Nothing(text) => UnicodeWidthStr::width(*text),
        }
    }

    /// The tail, painted the way the answer of a chain is painted: the mark is
    /// yellow, the number is bold, and the command is cyan.
    fn paint(&self, mark_width: usize, start: &StartCommand) -> String {
        match self {
            Self::Next(number) => {
                let pad =
                    mark_width.saturating_sub(UnicodeWidthStr::width(marked(*number).as_str()));
                format!(
                    "{} {}{}{}{}",
                    MARK_NEXT.to_string().yellow().bold(),
                    number.to_string().bold(),
                    " ".repeat(pad),
                    " ".repeat(COLUMN_GAP),
                    command(start, *number).cyan().bold()
                )
            }
            Self::Nothing(text) => text.dimmed().to_string(),
        }
    }
}

/// `→ #344`: the mark of the issue to start, and its number.
fn marked(number: IssueNumber) -> String {
    format!("{MARK_NEXT} {number}")
}

/// `si 344`: the command that starts one issue.
fn command(start: &StartCommand, number: IssueNumber) -> String {
    format!("{} {}", start.as_str(), number.get())
}

/// One line for each stream: its label, and the issue to start in it.
///
/// The tail is what the reader came for, so it takes its columns first and the
/// label is cut to what is left. A label that pushed the command off the window
/// would take the answer away. The cut stops at [`MIN_LABEL_WIDTH`]: a window
/// too narrow for both lets the line run past the edge and wrap, because a
/// wrapped line that names its stream is worth more than a line that names
/// none.
fn summary(streams: &[StreamReport], width: usize, start: &StartCommand) -> Vec<String> {
    let tails: Vec<Tail> = streams
        .iter()
        .map(|stream| Tail::of(&stream.report))
        .collect();
    let mark_width = tails.iter().filter_map(Tail::mark_width).max().unwrap_or(0);
    let tail_width = tails
        .iter()
        .map(|tail| tail.width(mark_width, start))
        .max()
        .unwrap_or(0);

    let budget = width
        .saturating_sub(PLAN_INDENT + COLUMN_GAP + tail_width)
        .max(MIN_LABEL_WIDTH);
    let labels: Vec<String> = streams
        .iter()
        .map(|stream| truncate_to_budget(&stream.label, budget))
        .collect();
    let label_width = labels
        .iter()
        .map(|label| UnicodeWidthStr::width(label.as_str()))
        .max()
        .unwrap_or(0);

    labels
        .iter()
        .zip(&tails)
        .map(|(label, tail)| {
            format!(
                "{}{}{}{}",
                " ".repeat(PLAN_INDENT),
                pad_right(label, label_width),
                " ".repeat(COLUMN_GAP),
                tail.paint(mark_width, start)
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::report::{Closes, States};

    /// The repository the test states come from.
    const REPO: &str = "timmattison/tools";

    /// The paste of issue #418: two streams that join.
    ///
    /// A picture, and not a list of entries, because the test then says what a
    /// reader typed. The graph reads the steps in the order 242, 247, 246,
    /// 248, 249, which keeps each stream of the picture together.
    const PASTE: &str = "\
#242 ──→ #247 ──┐
                ├──→ #249  (gallery)
#246 ──→ #248 ──┘";

    /// What GitHub says about the paste when every issue of it is open.
    const ALL_OPEN: &[(u64, Status, &str)] = &[
        (242, Status::Open, "Read the picture"),
        (247, Status::Open, "Answer the picture"),
        (246, Status::Open, "Read the table"),
        (248, Status::Open, "Answer the table"),
        (249, Status::Open, "Paint the gallery"),
    ];

    fn entry(number: u64, status: Status, title: &str) -> Entry {
        Entry {
            number: IssueNumber::new(number).expect("the test number is an issue number"),
            title: title.to_string(),
            status,
            closes: None,
        }
    }

    /// Render, and take the color back out.
    ///
    /// The `colored` crate decides at format time whether to write escape
    /// codes, and one input to that decision is whether standard output is a
    /// terminal. A test that compared the painted text against plain text
    /// would thus pass under a redirected run and fail under a hand-typed
    /// `git commit`. So every test here forces the codes on and strips them.
    fn glyphs(report: &Report, width: usize) -> String {
        glyphs_with_start(report, width, &StartCommand::new(None))
    }

    /// The same, with the start command the caller names.
    fn glyphs_with_start(report: &Report, width: usize, start: &StartCommand) -> String {
        testcolor::strip_ansi(&testcolor::with_forced_ansi(|| {
            render(report, REPO, width, start)
        }))
    }

    fn a_chain() -> Report {
        Report::build(vec![
            entry(277, Status::Done, "First thing"),
            entry(278, Status::Open, "Second thing"),
            entry(279, Status::Open, "Third thing"),
        ])
    }

    #[test]
    fn a_row_for_each_issue_and_the_answer_under_them() {
        assert_eq!(
            glyphs(&a_chain(), 80),
            concat!(
                "✓ #277  First thing\n",
                "→ #278  Second thing\n",
                "· #279  Third thing\n",
                "\n",
                "Start #278 next with 'si 278'",
            )
        );
    }

    #[test]
    fn the_answer_names_the_command_it_was_given() {
        let start = StartCommand::new(Some("gh issue develop"));
        assert!(
            glyphs_with_start(&a_chain(), 80, &start)
                .ends_with("Start #278 next with 'gh issue develop 278'"),
            "the answer names the command the caller gave"
        );
    }

    #[test]
    fn the_numbers_line_up_under_each_other() {
        let report = Report::build(vec![
            entry(9, Status::Done, "Small"),
            entry(1234, Status::Open, "Large"),
        ]);
        assert_eq!(
            glyphs(&report, 80),
            concat!(
                "✓ #9     Small\n",
                "→ #1234  Large\n",
                "\n",
                "Start #1234 next with 'si 1234'",
            )
        );
    }

    #[test]
    fn a_closed_chain_names_no_command() {
        let report = Report::build(vec![
            entry(277, Status::Done, "First thing"),
            entry(278, Status::Dropped, "Second thing"),
        ]);
        assert_eq!(
            glyphs(&report, 80),
            concat!(
                "✓ #277  First thing\n",
                "⊘ #278  Second thing\n",
                "\n",
                "Every issue in the chain is closed. Nothing to start.",
            )
        );
    }

    #[test]
    fn a_missing_number_gets_a_row_and_a_note() {
        let report = Report::build(vec![
            entry(999, Status::Missing, ""),
            entry(278, Status::Open, "Second thing"),
        ]);
        assert_eq!(
            glyphs(&report, 80),
            concat!(
                "? #999  (no such issue)\n",
                "→ #278  Second thing\n",
                "\n",
                "#999 is not in timmattison/tools.\n",
                "Start #278 next with 'si 278'",
            )
        );
    }

    #[test]
    fn two_missing_numbers_share_one_note() {
        let report = Report::build(vec![
            entry(999, Status::Missing, ""),
            entry(1000, Status::Missing, ""),
            entry(278, Status::Open, "Second thing"),
        ]);
        let block = glyphs(&report, 80);
        assert!(
            block.contains("#999 and #1000 are not in timmattison/tools.\n"),
            "one note names both numbers, in {block:?}"
        );
    }

    #[test]
    fn a_chain_with_nothing_open_and_a_missing_number_is_not_called_closed() {
        let report = Report::build(vec![
            entry(277, Status::Done, "First thing"),
            entry(999, Status::Missing, ""),
        ]);
        let block = glyphs(&report, 80);
        assert!(
            block.ends_with("No issue in the chain is open."),
            "the answer does not claim the chain is finished, in {block:?}"
        );
    }

    #[test]
    fn work_done_out_of_order_earns_a_note() {
        let report = Report::build(vec![
            entry(277, Status::Done, "First thing"),
            entry(278, Status::Open, "Second thing"),
            entry(279, Status::Open, "Third thing"),
            entry(280, Status::Done, "Fourth thing"),
        ]);
        let block = glyphs(&report, 80);
        assert!(
            block.contains("#280 is already closed, out of order.\n"),
            "the note names the issue done early, in {block:?}"
        );
        assert!(
            block.ends_with("Start #278 next with 'si 278'"),
            "the answer still stands last, in {block:?}"
        );
    }

    #[test]
    fn three_out_of_order_issues_read_as_a_list() {
        let report = Report::build(vec![
            entry(1, Status::Open, "One"),
            entry(2, Status::Done, "Two"),
            entry(3, Status::Done, "Three"),
            entry(4, Status::Dropped, "Four"),
        ]);
        let block = glyphs(&report, 80);
        assert!(
            block.contains("#2, #3 and #4 are already closed, out of order.\n"),
            "the note lists all three, in {block:?}"
        );
    }

    #[test]
    fn a_long_title_is_cut_to_the_width() {
        // The width is what the caller asks for, and the row fills it. The
        // window of the terminal is one column wider than the width `main`
        // asks for, because `termwindow` keeps the last column of the window
        // empty. That margin belongs to `main`, not to this function.
        let report = Report::build(vec![entry(
            277,
            Status::Open,
            "A title that is far too long for the window it has to fit in",
        )]);
        let block = glyphs(&report, 20);
        let row = block.lines().next().expect("the block holds a row");
        assert_eq!(row, "→ #277  A title tha…");
        assert_eq!(
            UnicodeWidthStr::width(row),
            20,
            "the row fills the width it was given"
        );
    }

    #[test]
    fn a_window_too_narrow_for_a_title_keeps_the_number() {
        // The row has no columns left for a title, and a row never ends in the
        // spaces that would have stood before one.
        let report = Report::build(vec![entry(277, Status::Open, "A title")]);
        let row = glyphs(&report, 8)
            .lines()
            .next()
            .expect("the block holds a row")
            .to_string();
        assert_eq!(row, "→ #277");
    }

    #[test]
    fn a_wide_title_is_cut_by_columns_and_not_by_characters() {
        let report = Report::build(vec![entry(277, Status::Open, "日本語のタイトル")]);
        let row = glyphs(&report, 14)
            .lines()
            .next()
            .expect("the block holds a row")
            .to_string();
        assert!(
            UnicodeWidthStr::width(row.as_str()) <= 14,
            "the row fits the width, in {row:?}"
        );
        assert_eq!(row, "→ #277  日本…");
    }

    #[test]
    fn the_rows_carry_color_and_strip_back_to_the_glyphs() {
        let painted =
            testcolor::with_forced_ansi(|| render(&a_chain(), REPO, 80, &StartCommand::new(None)));
        assert!(
            painted.contains('\u{1b}'),
            "the block is painted, in {painted:?}"
        );
        assert!(
            testcolor::strip_ansi(&painted).starts_with("✓ #277  First thing\n"),
            "the paint comes back out, in {painted:?}"
        );
    }

    #[test]
    fn an_empty_chain_renders_nothing_and_does_not_panic() {
        // parse_chain never gives an empty chain, so this row of the table is
        // about the type rather than about a run of the tool.
        assert_eq!(glyphs(&Report::build(Vec::new()), 80), "");
    }

    /// The entry of a step that names a pull request and the issue it closes.
    fn paired(
        number: u64,
        status: Status,
        title: &str,
        closes: u64,
        closes_status: Status,
    ) -> Entry {
        Entry {
            closes: Some(Closes {
                number: IssueNumber::new(closes).expect("the test number is an issue number"),
                status: closes_status,
            }),
            ..entry(number, status, title)
        }
    }

    /// One stream of a plan, from its label and the states of its steps.
    fn stream(label: &str, entries: Vec<Entry>) -> StreamReport {
        StreamReport {
            label: label.to_string(),
            report: Report::build(entries),
        }
    }

    /// Paint the plan, and take the color back out. See [`glyphs`].
    fn plan_glyphs(streams: &[StreamReport], width: usize) -> String {
        testcolor::strip_ansi(&testcolor::with_forced_ansi(|| {
            render_plan(streams, REPO, width, &StartCommand::new(None))
        }))
    }

    /// A plan of three streams, each with an issue to start.
    fn a_plan() -> Vec<StreamReport> {
        vec![
            stream(
                "S1 gitscratch",
                vec![
                    entry(344, Status::Done, "First thing"),
                    entry(330, Status::Open, "Second thing"),
                ],
            ),
            stream(
                "S2 ic",
                vec![
                    entry(350, Status::Open, "Third thing"),
                    entry(187, Status::Open, "Fourth thing"),
                ],
            ),
            stream("S3 wn", vec![entry(411, Status::Open, "Fifth thing")]),
        ]
    }

    #[test]
    fn a_block_for_each_stream_and_one_summary_under_them() {
        assert_eq!(
            plan_glyphs(&a_plan(), 80),
            concat!(
                "S1 gitscratch\n",
                "  ✓ #344  First thing\n",
                "  → #330  Second thing\n",
                "\n",
                "S2 ic\n",
                "  → #350  Third thing\n",
                "  · #187  Fourth thing\n",
                "\n",
                "S3 wn\n",
                "  → #411  Fifth thing\n",
                "\n",
                "Take one from each stream:\n",
                "  S1 gitscratch  → #330  si 330\n",
                "  S2 ic          → #350  si 350\n",
                "  S3 wn          → #411  si 411",
            )
        );
    }

    #[test]
    fn a_closed_stream_says_so_in_the_summary_and_names_no_command() {
        let streams = vec![
            stream("S1", vec![entry(344, Status::Done, "First")]),
            stream("S2", vec![entry(350, Status::Open, "Second")]),
        ];
        let block = plan_glyphs(&streams, 80);
        assert_eq!(
            block,
            concat!(
                "S1\n",
                "  ✓ #344  First\n",
                "\n",
                "S2\n",
                "  → #350  Second\n",
                "\n",
                "Take one from each stream:\n",
                "  S1  every issue is closed\n",
                "  S2  → #350  si 350",
            )
        );
        assert!(
            !block.contains("si 344"),
            "a closed stream names no command, in {block:?}"
        );
    }

    #[test]
    fn a_stream_with_nothing_open_and_a_missing_number_is_not_called_closed() {
        let streams = vec![
            stream(
                "S1",
                vec![
                    entry(344, Status::Done, "First"),
                    entry(999, Status::Missing, ""),
                ],
            ),
            stream("S2", vec![entry(350, Status::Open, "Second")]),
        ];
        let block = plan_glyphs(&streams, 80);
        assert!(
            block.contains("  S1  no issue is open\n"),
            "the summary does not claim the stream is finished, in {block:?}"
        );
    }

    #[test]
    fn a_missing_number_keeps_its_row_and_its_note_inside_its_own_block() {
        let streams = vec![
            stream(
                "S1",
                vec![
                    entry(999, Status::Missing, ""),
                    entry(344, Status::Open, "First"),
                ],
            ),
            stream("S2", vec![entry(350, Status::Open, "Second")]),
        ];
        assert_eq!(
            plan_glyphs(&streams, 80),
            concat!(
                "S1\n",
                "  ? #999  (no such issue)\n",
                "  → #344  First\n",
                "\n",
                "  #999 is not in timmattison/tools.\n",
                "\n",
                "S2\n",
                "  → #350  Second\n",
                "\n",
                "Take one from each stream:\n",
                "  S1  → #344  si 344\n",
                "  S2  → #350  si 350",
            )
        );
    }

    #[test]
    fn a_pair_whose_two_states_differ_earns_a_note() {
        let streams = vec![stream(
            "S1",
            vec![
                paired(344, Status::Done, "First", 341, Status::Open),
                entry(330, Status::Open, "Second"),
            ],
        )];
        assert_eq!(
            plan_glyphs(&streams, 80),
            concat!(
                "S1\n",
                "  ✓ #344 (#341)  First\n",
                "  → #330         Second\n",
                "\n",
                "  #344 is closed and #341 is open.\n",
                "\n",
                "Take one from each stream:\n",
                "  S1  → #330  si 330",
            )
        );
    }

    #[test]
    fn a_dropped_pull_request_over_an_open_issue_says_the_work_was_not_done() {
        let streams = vec![stream(
            "S1",
            vec![
                paired(342, Status::Dropped, "First", 328, Status::Open),
                entry(330, Status::Open, "Second"),
            ],
        )];
        let block = plan_glyphs(&streams, 80);
        assert!(
            block.contains("  #342 is closed without the work being done and #328 is open.\n"),
            "the note writes the word of each state, in {block:?}"
        );
    }

    #[test]
    fn the_notes_of_a_block_stand_in_one_order() {
        // The number the repository does not have comes first, because it is
        // the one that turns a green run red. The pair that disagrees comes
        // next, and the work done out of order stands last.
        let streams = vec![stream(
            "S1",
            vec![
                entry(999, Status::Missing, ""),
                paired(344, Status::Done, "First", 341, Status::Open),
                entry(330, Status::Open, "Second"),
                entry(350, Status::Done, "Third"),
            ],
        )];
        let block = plan_glyphs(&streams, 80);
        assert!(
            block.contains(concat!(
                "  #999 is not in timmattison/tools.\n",
                "  #344 is closed and #341 is open.\n",
                "  #350 is already closed, out of order.\n",
            )),
            "the three notes stand in one order, in {block:?}"
        );
    }

    #[test]
    fn a_pair_writes_both_numbers_and_a_lone_number_lines_up_under_it() {
        let streams = vec![stream(
            "S1",
            vec![
                paired(344, Status::Open, "First", 341, Status::Open),
                entry(330, Status::Open, "Second"),
            ],
        )];
        let block = plan_glyphs(&streams, 80);
        let rows: Vec<&str> = block.lines().skip(1).take(2).collect();
        assert_eq!(
            rows,
            vec!["  → #344 (#341)  First", "  · #330         Second"]
        );
    }

    #[test]
    fn each_block_lines_up_under_itself_and_not_under_its_neighbour() {
        let streams = vec![
            stream(
                "S1",
                vec![paired(344, Status::Open, "First", 341, Status::Open)],
            ),
            stream("S2", vec![entry(350, Status::Open, "Second")]),
        ];
        let block = plan_glyphs(&streams, 80);
        assert!(
            block.contains("  → #344 (#341)  First\n"),
            "the wide block holds its own column, in {block:?}"
        );
        assert!(
            block.contains("  → #350  Second\n"),
            "the narrow block is not padded to the column of the wide one, in {block:?}"
        );
    }

    #[test]
    fn the_summary_lines_up_under_itself() {
        let streams = vec![
            stream(
                "S1 a long stream label",
                vec![entry(344, Status::Open, "First")],
            ),
            stream("S2 ic", vec![entry(350, Status::Open, "Second")]),
        ];
        let block = plan_glyphs(&streams, 80);
        assert!(
            block.ends_with(concat!(
                "  S1 a long stream label  → #344  si 344\n",
                "  S2 ic                   → #350  si 350",
            )),
            "the short label is padded to the width of the long one, in {block:?}"
        );
    }

    #[test]
    fn a_long_stream_label_is_cut_to_the_window() {
        let streams = vec![stream(
            "S1 a very long stream label that goes on",
            vec![entry(350, Status::Open, "Third thing")],
        )];
        let block = plan_glyphs(&streams, 30);
        assert_eq!(
            block,
            concat!(
                "S1 a very long stream label t…\n",
                "  → #350  Third thing\n",
                "\n",
                "Take one from each stream:\n",
                "  S1 a very l…  → #350  si 350",
            )
        );
        for line in block.lines() {
            assert!(
                UnicodeWidthStr::width(line) <= 30,
                "no line is wider than the window, in {line:?}"
            );
        }
    }

    #[test]
    fn a_summary_line_names_its_stream_at_any_width() {
        // The tail of the closed stream is wider than this window on its own,
        // so cutting the label to what the tail leaves cuts it to nothing. A
        // line that names no stream answers for no stream, and the heading
        // over these lines promises one answer for each. So the label keeps
        // its floor and the line runs past the edge instead.
        let streams = vec![
            stream("Stream 1", vec![entry(330, Status::Open, "Second thing")]),
            stream("Stream 2", vec![entry(344, Status::Done, "First thing")]),
        ];
        let block = plan_glyphs(&streams, 20);
        let summary: Vec<&str> = block
            .lines()
            .skip_while(|line| *line != SUMMARY_HEADING)
            .skip(1)
            .collect();
        assert_eq!(
            summary,
            vec![
                "  Stream 1  → #330  si 330",
                "  Stream 2  every issue is closed",
            ]
        );
    }

    #[test]
    fn a_row_of_a_plan_is_cut_to_the_columns_the_indent_leaves() {
        let streams = vec![stream(
            "S1",
            vec![entry(
                277,
                Status::Open,
                "A title that is far too long for the window it has to fit in",
            )],
        )];
        let block = plan_glyphs(&streams, 20);
        let row = block.lines().nth(1).expect("the block holds a row");
        assert_eq!(row, "  → #277  A title t…");
        assert_eq!(
            UnicodeWidthStr::width(row),
            20,
            "the row fills the width it was given"
        );
    }

    #[test]
    fn a_plan_of_no_streams_renders_nothing_and_does_not_panic() {
        assert_eq!(plan_glyphs(&[], 80), "");
    }

    #[test]
    fn the_plan_carries_color_and_strips_back_to_the_glyphs() {
        let painted = testcolor::with_forced_ansi(|| {
            render_plan(&a_plan(), REPO, 80, &StartCommand::new(None))
        });
        assert!(
            painted.contains('\u{1b}'),
            "the plan is painted, in {painted:?}"
        );
        assert!(
            testcolor::strip_ansi(&painted).starts_with("S1 gitscratch\n  ✓ #344  First thing\n"),
            "the paint comes back out, in {painted:?}"
        );
    }

    /// The report of the picture `text`, with what GitHub says about each of
    /// the numbers `answers` names.
    ///
    /// The test builds the report out of a real picture, so it says what a
    /// reader typed and what GitHub answered. A number `answers` does not name
    /// is a number the repository does not have.
    fn picture(text: &str, answers: &[(u64, Status, &str)]) -> Report {
        let graph = crate::graph::read(text)
            .expect("the text draws a graph")
            .expect("the picture reads");
        let states = States::of(
            answers
                .iter()
                .map(|&(number, status, title)| entry(number, status, title))
                .collect(),
        );
        Report::of_graph(&graph, &states)
    }

    /// Paint the picture, and take the color back out. See [`glyphs`].
    fn graph_glyphs(report: &Report, width: usize) -> String {
        testcolor::strip_ansi(&testcolor::with_forced_ansi(|| {
            render_graph(report, REPO, width, &StartCommand::new(None))
        }))
    }

    /// The row of the step `number` in a painted block.
    ///
    /// A row writes its own number after the mark, and the numbers it waits
    /// for at the end. So the search reads the head of the line, and it never
    /// finds a row that names the number in its last column.
    fn row_of(block: &str, number: u64) -> String {
        let number = format!("#{number}");
        block
            .lines()
            .find(|line| line.split_whitespace().nth(1) == Some(number.as_str()))
            .expect("the block holds a row for the number")
            .to_string()
    }

    #[test]
    fn a_blocked_row_names_every_step_it_waits_for() {
        // Both steps before `#249` are open. A row that named the first of
        // them would send somebody to `#247` and hide `#248`, so the row names
        // both, in the order the picture holds them.
        let block = graph_glyphs(&picture(PASTE, ALL_OPEN), 80);
        let row = row_of(&block, 249);
        assert!(
            row.ends_with("waits for #247, #248"),
            "the row names each step it waits for, in {row:?}"
        );
        assert!(
            row.contains("Paint the gallery"),
            "the row keeps its title beside the work it waits for, in {row:?}"
        );
    }

    #[test]
    fn a_picture_whose_every_step_is_finished_names_no_command() {
        // Nothing of this picture is left to do, and the answer says so of the
        // picture the reader pasted rather than of a chain they did not write.
        let block = graph_glyphs(
            &picture(
                PASTE,
                &[
                    (242, Status::Done, "Read the picture"),
                    (247, Status::Done, "Answer the picture"),
                    (246, Status::Done, "Read the table"),
                    (248, Status::Dropped, "Answer the table"),
                    (249, Status::Done, "Paint the gallery"),
                ],
            ),
            80,
        );
        assert!(
            block.ends_with("Every issue in the graph is closed. Nothing to start."),
            "the answer names the shape the reader pasted, in {block:?}"
        );
        assert!(
            !block.contains("Start #"),
            "a picture with nothing left to do names no command, in {block:?}"
        );
    }

    /// The numbers of the rows of a painted block, in the order they print.
    ///
    /// The rows are the lines over the blank line, and a row writes its own
    /// number after the mark.
    fn rows_of(block: &str) -> Vec<String> {
        block
            .lines()
            .take_while(|line| !line.is_empty())
            .filter_map(|line| line.split_whitespace().nth(1))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_row_for_each_step_of_a_picture_and_one_answer_for_each_stream() {
        assert_eq!(
            graph_glyphs(&picture(PASTE, ALL_OPEN), 80),
            concat!(
                "→ #242  Read the picture\n",
                "· #247  Answer the picture  waits for #242\n",
                "→ #246  Read the table\n",
                "· #248  Answer the table    waits for #246\n",
                "· #249  Paint the gallery   waits for #247, #248\n",
                "\n",
                "Start #242 next with 'si 242'\n",
                "Start #246 next with 'si 246'",
            )
        );
    }

    #[test]
    fn a_step_that_is_finished_is_no_reason_for_the_step_after_it_to_wait() {
        // The top stream is finished, so the bottom stream is the only one to
        // start and the join waits for the one step of it that is left.
        let block = graph_glyphs(
            &picture(
                PASTE,
                &[
                    (242, Status::Done, "Read the picture"),
                    (247, Status::Done, "Answer the picture"),
                    (246, Status::Open, "Read the table"),
                    (248, Status::Open, "Answer the table"),
                    (249, Status::Open, "Paint the gallery"),
                ],
            ),
            80,
        );
        let row = row_of(&block, 249);
        assert!(
            row.ends_with("waits for #248"),
            "the row names the work that is left and not the work that is done, in {row:?}"
        );
        assert!(
            block.ends_with("Start #246 next with 'si 246'"),
            "the stream that is finished starts nothing, in {block:?}"
        );
        assert!(
            !block.contains("Start #242"),
            "a finished step is no answer, in {block:?}"
        );
    }

    #[test]
    fn a_ready_row_and_a_finished_row_end_at_their_titles() {
        // A row that waits for nothing writes no last column, and it ends at
        // its title rather than in the spaces that would stand before one.
        let block = graph_glyphs(
            &picture(
                PASTE,
                &[
                    (242, Status::Done, "Read the picture"),
                    (247, Status::Done, "Answer the picture"),
                    (246, Status::Open, "Read the table"),
                    (248, Status::Open, "Answer the table"),
                    (249, Status::Open, "Paint the gallery"),
                ],
            ),
            80,
        );
        assert_eq!(row_of(&block, 242), "✓ #242  Read the picture");
        assert_eq!(row_of(&block, 246), "→ #246  Read the table");
        for line in block.lines() {
            assert!(
                !line.ends_with(' '),
                "no line of the block ends in a space, in {line:?}"
            );
        }
    }

    #[test]
    fn the_rows_of_a_picture_keep_each_stream_together() {
        // The rows stand in the order the graph holds them, which is a
        // topological order with a tie going to the text. So the top stream of
        // the paste prints before the bottom one, and a reader reads the
        // streams the way they drew them.
        let block = graph_glyphs(&picture(PASTE, ALL_OPEN), 80);
        assert_eq!(
            rows_of(&block),
            vec!["#242", "#247", "#246", "#248", "#249"]
        );
    }

    #[test]
    fn a_number_the_repository_does_not_have_earns_a_note_and_blocks_the_step_after_it() {
        // GitHub answered for every number but `#247`. Nothing is known about
        // a missing step, so it is not finished and `#249` waits for it. The
        // note under the rows says why nobody can start that work.
        let block = graph_glyphs(
            &picture(
                PASTE,
                &[
                    (242, Status::Done, "Read the picture"),
                    (246, Status::Done, "Read the table"),
                    (248, Status::Done, "Answer the table"),
                    (249, Status::Open, "Paint the gallery"),
                ],
            ),
            80,
        );
        assert_eq!(row_of(&block, 247), "? #247  (no such issue)");
        assert!(
            block.contains("#247 is not in timmattison/tools.\n"),
            "the note names the number the repository does not have, in {block:?}"
        );
        let row = row_of(&block, 249);
        assert!(
            row.ends_with("waits for #247"),
            "the row of the step behind it says so, in {row:?}"
        );
    }

    #[test]
    fn a_picture_with_nothing_open_and_a_missing_number_is_not_called_closed() {
        // Nothing is open and one number is not an issue at all, so the
        // picture is not finished. Saying it is would be a guess about the
        // number nobody could read.
        let block = graph_glyphs(
            &picture(
                PASTE,
                &[
                    (242, Status::Done, "Read the picture"),
                    (247, Status::Done, "Answer the picture"),
                    (246, Status::Done, "Read the table"),
                    (248, Status::Done, "Answer the table"),
                ],
            ),
            80,
        );
        assert!(
            block.ends_with("No issue in the graph is open."),
            "the answer does not claim the picture is finished, in {block:?}"
        );
        assert!(
            block.contains("#249 is not in timmattison/tools.\n"),
            "the note names the number the repository does not have, in {block:?}"
        );
    }

    #[test]
    fn work_closed_out_of_order_in_a_picture_earns_the_note_a_chain_earns() {
        // `#242` is open and every other step is done, so `#247` is closed over
        // the step before it and `#249` is closed over a step two hops back.
        // A picture asks that question of every step the wires reach, and it
        // writes the answer in the words a chain writes.
        let block = graph_glyphs(
            &picture(
                PASTE,
                &[
                    (242, Status::Open, "Read the picture"),
                    (247, Status::Done, "Answer the picture"),
                    (246, Status::Done, "Read the table"),
                    (248, Status::Done, "Answer the table"),
                    (249, Status::Done, "Paint the gallery"),
                ],
            ),
            80,
        );
        assert!(
            block.contains("#247 and #249 are already closed, out of order.\n"),
            "the note names each step somebody closed early, in {block:?}"
        );
        assert!(
            block.ends_with("Start #242 next with 'si 242'"),
            "the answer still stands last, in {block:?}"
        );
    }

    #[test]
    fn a_wide_title_of_a_picture_is_cut_by_columns_and_not_by_characters() {
        // A Japanese character takes two columns and one character. A row that
        // counted characters would run two columns past the window for each
        // one of them, and the row would wrap.
        let block = graph_glyphs(
            &picture(
                PASTE,
                &[
                    (242, Status::Open, "Read the picture"),
                    (247, Status::Open, "Answer the picture"),
                    (246, Status::Open, "Read the table"),
                    (248, Status::Open, "Answer the table"),
                    (249, Status::Open, "日本語のタイトルはとても長い"),
                ],
            ),
            40,
        );
        let row = row_of(&block, 249);
        assert!(
            row.contains("日本語の…"),
            "the title is cut at a column and at a character, in {row:?}"
        );
        for line in block.lines() {
            assert!(
                UnicodeWidthStr::width(line) <= 40,
                "no line is wider than the window, in {line:?}"
            );
        }
    }

    #[test]
    fn a_picture_with_an_open_step_and_nothing_ready_says_why() {
        // GitHub answered for neither of the two steps that start the streams,
        // so nothing is known about them and neither of them is finished. The
        // three steps that are open each wait for one of them. A silent
        // "nothing to start" reads as "the plan is done", which is the
        // opposite of the truth.
        let block = graph_glyphs(
            &picture(
                PASTE,
                &[
                    (247, Status::Open, "Answer the picture"),
                    (248, Status::Open, "Answer the table"),
                    (249, Status::Open, "Paint the gallery"),
                ],
            ),
            80,
        );
        assert!(
            block.ends_with(concat!(
                "No issue in the graph is ready. ",
                "Every open issue waits for work that is not finished.",
            )),
            "the answer says why nobody starts anything, in {block:?}"
        );
    }

    #[test]
    fn the_work_each_row_waits_for_stands_in_one_column() {
        // The titles of a block are of different lengths, and a column that
        // opened after each title would step left and right down the block.
        // The reader then reads the column as prose and not as a column.
        let block = graph_glyphs(&picture(PASTE, ALL_OPEN), 80);
        let columns: Vec<usize> = block
            .lines()
            .filter_map(|line| line.split_once(WAITS_FOR))
            .map(|(head, _)| UnicodeWidthStr::width(head))
            .collect();
        assert_eq!(columns.len(), 3, "three rows wait for work, in {block:?}");
        assert!(
            columns.iter().all(|column| *column == columns[0]),
            "the column opens at one place in every row, in {block:?}"
        );
    }

    #[test]
    fn a_row_of_a_picture_never_wraps_whatever_the_window_holds() {
        // The last column takes its columns out of the window first, and the
        // title is cut to what is left. A row that wrapped would cost two
        // lines, and the second of them would carry no number.
        let report = picture(PASTE, ALL_OPEN);
        for width in [80, 40, 30] {
            let block = graph_glyphs(&report, width);
            for line in block.lines() {
                assert!(
                    UnicodeWidthStr::width(line) <= width,
                    "no line is wider than the window of {width}, in {line:?}"
                );
            }
        }

        assert!(
            row_of(&graph_glyphs(&report, 80), 249).contains("Paint the gallery"),
            "a window wide enough holds the whole title beside the column"
        );
        assert!(
            row_of(&graph_glyphs(&report, 40), 249).contains("Paint the…"),
            "a narrower window cuts the title and keeps the column"
        );
        assert_eq!(
            row_of(&graph_glyphs(&report, 30), 249),
            "· #249  waits for #247, #248",
            "a window with no columns for a title keeps the number and the column"
        );
    }

    #[test]
    fn the_answer_of_a_picture_names_a_command_for_every_step_that_is_ready() {
        // Two streams that join are two people who work at the same time, and
        // an answer that names one issue loses the reason somebody drew the
        // picture. Nothing of this paste is finished, so both streams start.
        let block = graph_glyphs(&picture(PASTE, ALL_OPEN), 80);
        assert!(
            block.ends_with(concat!(
                "Start #242 next with 'si 242'\n",
                "Start #246 next with 'si 246'",
            )),
            "the answer names one command for each stream, in {block:?}"
        );
    }
}
