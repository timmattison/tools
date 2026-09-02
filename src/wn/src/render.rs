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
//! # A plan is one block for each stream
//!
//! A plan of parallel work holds many streams, and [`render_plan`] paints one
//! block for each of them. A block carries no answer of its own: the summary
//! under the last block names the issue to start in every stream, so the
//! reader reads the answers together and picks the stream they want.

use colored::{ColoredString, Colorize};
use textfit::{pad_right, truncate_to_budget};
use unicode_width::UnicodeWidthStr;

use crate::chain::IssueNumber;
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

/// The answer of a stream where every step is finished.
const EVERY_ISSUE_CLOSED: &str = "every issue is closed";

/// The answer of a stream where nothing is open and something could not be
/// read.
const NO_ISSUE_OPEN: &str = "no issue is open";

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
        .map(|(position, entry)| row(entry, report.next() == Some(position), number_width, width))
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

/// One row: the mark, the number, and as much of the title as the width
/// holds.
///
/// A row that has no columns left for a title ends at the number, rather than
/// in the spaces that would have stood before one.
///
/// The number a row writes is [`Entry::label`], so a step of a plan that names
/// a pull request and the issue it closes writes both. The width of the column
/// comes out of the same call, and the two can never part company.
fn row(entry: &Entry, is_next: bool, number_width: usize, width: usize) -> String {
    let style = style(entry.status, is_next);
    let number = entry.label();
    let mark = style.mark.to_string();

    let spent = MARK_WIDTH + 1 + number_width + COLUMN_GAP;
    let title = if entry.status == Status::Missing {
        MISSING_TITLE
    } else {
        entry.title.as_str()
    };
    let title = truncate_to_budget(title, width.saturating_sub(spent));

    let mark = (style.paint_mark)(&mark);
    if title.is_empty() {
        return format!("{mark} {}", (style.paint_text)(&number));
    }
    let number = pad_right(&number, number_width);
    format!(
        "{mark} {}{}{}",
        (style.paint_text)(&number),
        " ".repeat(COLUMN_GAP),
        (style.paint_text)(&title)
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
    format!(
        "Start {} next with '{}'",
        entry.number.to_string().bold(),
        format!("{} {}", start.as_str(), entry.number.get())
            .cyan()
            .bold()
    )
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
                report.next() == Some(position),
                number_width,
                row_width,
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
    /// `mark_width`. This is what the label of a summary line has to give way
    /// to.
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
/// would take the answer away.
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

    let budget = width.saturating_sub(PLAN_INDENT + COLUMN_GAP + tail_width);
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

/// Write a list of numbers the way a sentence reads one.
fn list(numbers: &[IssueNumber]) -> String {
    let written: Vec<String> = numbers.iter().map(ToString::to_string).collect();
    match written.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::report::Closes;

    /// The repository the test states come from.
    const REPO: &str = "timmattison/tools";

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
}
