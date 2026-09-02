//! Painting a [`Report`] into the block of text the reader sees.
//!
//! The block has two parts. Every issue of the chain gets one row, in the
//! order the chain wrote it, so the reader can check the plan they typed
//! against the plan GitHub holds. Under the rows stands the answer, and the
//! answer names the command that starts the work.
//!
//! # One row never wraps
//!
//! A title is the one piece of a row with no bound on its length, and a row
//! that wraps costs two lines where the second one carries no number. So the
//! title is cut to the columns the row has left, through
//! [`textfit::truncate_to_budget`], which gives an empty title rather than a
//! marker that is itself one column too wide.

use colored::{ColoredString, Colorize};
use textfit::{pad_right, truncate_to_budget};
use unicode_width::UnicodeWidthStr;

use crate::chain::IssueNumber;
use crate::report::{Entry, Report, Status};

/// The command that starts work on an issue. The answer names it, because the
/// answer is only useful if the next thing to type is on the screen.
const START_COMMAND: &str = "si";

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

/// Paint the chain, the notes it earns, and the answer.
///
/// `repo` names the repository the states came from, and appears only in the
/// note about a number that repository does not have. `width` is the columns
/// the block has to fit in.
#[must_use]
pub fn render(report: &Report, repo: &str, width: usize) -> String {
    let entries = report.entries();
    if entries.is_empty() {
        return String::new();
    }

    let number_width = entries
        .iter()
        .map(|entry| UnicodeWidthStr::width(entry.number.to_string().as_str()))
        .max()
        .unwrap_or(0);

    let mut lines: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(position, entry)| row(entry, report.next() == Some(position), number_width, width))
        .collect();

    lines.push(String::new());
    lines.extend(notes(report, repo));
    lines.push(answer(report));
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

/// One row: the mark, the number, and as much of the title as the window
/// holds.
///
/// A row that has no columns left for a title ends at the number, rather than
/// in the spaces that would have stood before one.
fn row(entry: &Entry, is_next: bool, number_width: usize, width: usize) -> String {
    let style = style(entry.status, is_next);
    let number = entry.number.to_string();
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

/// The answer: the issue to start and the command that starts it.
fn answer(report: &Report) -> String {
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
        format!("{START_COMMAND} {}", entry.number.get())
            .cyan()
            .bold()
    )
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

    /// The repository the test states come from.
    const REPO: &str = "timmattison/tools";

    fn entry(number: u64, status: Status, title: &str) -> Entry {
        Entry {
            number: IssueNumber::new(number).expect("the test number is an issue number"),
            title: title.to_string(),
            status,
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
        testcolor::strip_ansi(&testcolor::with_forced_ansi(|| render(report, REPO, width)))
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
        let report = Report::build(vec![entry(
            277,
            Status::Open,
            "A title that is far too long for the window it has to fit in",
        )]);
        let block = glyphs(&report, 20);
        let row = block.lines().next().expect("the block holds a row");
        assert_eq!(row, "→ #277  A title tha…");
        assert_eq!(UnicodeWidthStr::width(row), 20, "the row fills the width");
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
            "the row fits the window, in {row:?}"
        );
        assert_eq!(row, "→ #277  日本…");
    }

    #[test]
    fn the_rows_carry_color_and_strip_back_to_the_glyphs() {
        let painted = testcolor::with_forced_ansi(|| render(&a_chain(), REPO, 80));
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
}
