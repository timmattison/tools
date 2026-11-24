//! `occ` — "old Claude Code": list the Claude Code sessions running on this
//! machine, oldest release first.

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};
use occ::report::{Report, SessionReport};
use occ::session::SessionId;
use occ::{build, format_uptime, gather_processes, SessionRegistry};

/// Shown when a value could not be read.
const ABSENT: &str = "—";

#[derive(Parser)]
#[command(
    name = "occ",
    version = version_string!(),
    about = "Old Claude Code: list running Claude Code sessions, oldest release first"
)]
struct Cli {}

fn main() -> Result<()> {
    let Cli {} = Cli::parse();

    let facts = gather_processes();
    let home = dirs::home_dir().context("cannot find the home directory")?;
    let registry = SessionRegistry::for_home(&home);
    let report = build(&facts, &registry);

    if report.sessions.is_empty() {
        println!("No Claude Code sessions are running.");
    } else {
        println!("{}", render(&report.sessions));
    }
    print_footer(&report);
    Ok(())
}

/// Builds the table of sessions.
///
/// Every color here is set on the [`Cell`], never written into the text of the
/// cell. The table measures the text to find its column widths, and an escape
/// sequence written into that text is measured with it: a red and bold release
/// would take three more columns than a yellow one, and the right edge of the
/// table would bend by three columns on that row. A color the table applies
/// itself lands after the measurement and moves nothing.
fn render(sessions: &[SessionReport]) -> Table {
    // The releases at the two ends of the report drive the coloring, so a
    // reader can see at a glance which sessions have fallen behind.
    let oldest = sessions.first().and_then(|row| row.version.as_ref());
    let newest = sessions
        .iter()
        .filter_map(|row| row.version.as_ref())
        .next_back();

    let mut table = Table::new();
    table
        .load_preset(comfy_table::presets::UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(["PID", "RELEASE", "OPEN FOR", "SESSION", "DIRECTORY"]);

    for row in sessions {
        let release = match row.version.as_ref() {
            None => absent(),
            Some(version) if Some(version) == oldest && oldest != newest => Cell::new(version)
                .fg(Color::Red)
                .add_attribute(Attribute::Bold),
            Some(version) if Some(version) == newest => Cell::new(version).fg(Color::Green),
            Some(version) => Cell::new(version).fg(Color::Yellow),
        };
        let directory = row
            .directory
            .as_ref()
            .map_or_else(absent, |path| Cell::new(path.display()));

        table.add_row([
            Cell::new(row.pid),
            release,
            Cell::new(format_uptime(row.uptime_secs)),
            render_session(row.session.as_ref()),
            directory,
        ]);
    }
    table
}

/// The cell shown in place of a value that could not be read.
fn absent() -> Cell {
    Cell::new(ABSENT).add_attribute(Attribute::Dim)
}

/// Renders the session cell.
///
/// Every id here was recorded by the session itself, so none of them is
/// qualified. A session that recorded nothing is left blank rather than guessed
/// at: a guess is wrong more often than it is right, and nothing in a table of
/// identifiers would say which one it was.
fn render_session(session: Option<&SessionId>) -> Cell {
    session.map_or_else(absent, Cell::new)
}

/// Chooses the singular or the plural form for `count`.
fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

/// Builds the lines shown below the table.
///
/// The footer says what the table left out. A tool that quietly drops processes
/// it cannot read would report a clean machine that is not clean, so every
/// count [`build`] collected appears here. A count of zero adds no line, so a
/// machine that hides nothing says nothing.
///
/// The lines are built as text instead of printed, because what a reader sees
/// is then a value, and a value can be read by a test.
fn footer_lines(report: &Report) -> Vec<String> {
    let Report {
        ref sessions,
        support,
        unreadable,
    } = *report;
    let unnamed = sessions.iter().filter(|row| row.session.is_none()).count();

    let mut lines = vec![format!(
        "{} running {}.",
        sessions.len().to_string().bold(),
        plural(sessions.len(), "session", "sessions")
    )];

    if unnamed > 0 {
        lines.push(
            format!(
                "{unnamed} of these sessions wrote no record in ~/.claude/sessions and {} named here.",
                plural(unnamed, "is not", "are not")
            )
            .dimmed()
            .to_string(),
        );
    }
    if support > 0 {
        lines.push(
            format!(
                "{support} Claude Code support {} not shown.",
                plural(support, "process", "processes")
            )
            .dimmed()
            .to_string(),
        );
    }
    if unreadable > 0 {
        lines.push(
            format!(
                "{unreadable} Claude Code {} {} to another account and cannot be read.",
                plural(unreadable, "process", "processes"),
                plural(unreadable, "belongs", "belong")
            )
            .dimmed()
            .to_string(),
        );
    }
    lines
}

/// Prints the counts below the table.
///
/// The blank line above the counts holds them apart from the table. It belongs
/// to the printing and not to the text, so [`footer_lines`] does not carry it.
fn print_footer(report: &Report) {
    println!();
    for line in footer_lines(report) {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::{footer_lines, render, render_session, ABSENT};
    use occ::{ClaudeVersion, Report, SessionId, SessionReport};
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    const SESSION_A: &str = "d3b0d921-f0a1-41fc-b309-c11aa30c1173";
    const SESSION_B: &str = "ed84c8c7-0117-4670-936c-98e0f0d2c80b";

    /// The text a reader sees, with every escape sequence removed.
    ///
    /// An escape sequence moves no cursor and occupies no column, so it is not
    /// part of the picture the table draws.
    fn visible(text: &str) -> String {
        let mut seen = String::new();
        let mut characters = text.chars();
        while let Some(character) = characters.next() {
            if character != '\u{1b}' {
                seen.push(character);
                continue;
            }
            // Skip the whole sequence: the `[` that opens it, its parameters,
            // and the letter that ends it.
            if characters.next() == Some('[') {
                for parameter in characters.by_ref() {
                    if !matches!(parameter, '0'..='9' | ';' | ':' | '?') {
                        break;
                    }
                }
            }
        }
        seen
    }

    fn session_id(text: &str) -> SessionId {
        SessionId::parse(text).expect("test id should parse")
    }

    fn row(pid: u32, release: &str, session: Option<SessionId>) -> SessionReport {
        SessionReport {
            pid,
            version: ClaudeVersion::parse(release),
            directory: Some(PathBuf::from("/work")),
            session,
            uptime_secs: 3_600,
        }
    }

    /// A report of `sessions` that leaves nothing out.
    fn report(sessions: Vec<SessionReport>) -> Report {
        Report {
            sessions,
            support: 0,
            unreadable: 0,
        }
    }

    /// The footer as a reader sees it, with every escape sequence removed.
    ///
    /// The footer colors itself, and whether it does so depends on where the
    /// output goes. What it says does not, so every assertion reads the text.
    fn footer(report: &Report) -> Vec<String> {
        footer_lines(report)
            .iter()
            .map(|line| visible(line))
            .collect()
    }

    #[test]
    fn the_colored_table_keeps_a_straight_right_edge() {
        // Each row here is colored differently: the oldest release red and
        // bold, the middle one yellow, the newest green, and a dimmed cell for
        // the session that recorded none. The escape sequences that carry those
        // colors are all of different lengths, and no reader sees any of them.
        let rows = [
            row(1, "2.1.196", Some(session_id(SESSION_A))),
            row(2, "2.1.200", Some(session_id(SESSION_B))),
            row(3, "2.1.204", None),
        ];

        let mut table = render(&rows);
        table.force_no_tty().set_width(120);

        let plain = table.to_string();
        let colored = table.enforce_styling().to_string();

        // The colors are there to be seen. Without this the table could satisfy
        // every rule below by having no color at all.
        assert_ne!(colored, plain, "the table must still color its cells");

        let widths: BTreeSet<usize> = colored
            .lines()
            .map(|line| visible(line).chars().count())
            .collect();
        assert_eq!(
            widths.len(),
            1,
            "every line must occupy the same width, found {widths:?} in:\n{colored}"
        );
        assert_eq!(
            visible(&colored),
            plain,
            "coloring a cell must not move anything"
        );
    }

    #[test]
    fn the_table_shows_the_id_a_session_recorded() {
        let rows = [
            row(1, "2.1.196", Some(session_id(SESSION_A))),
            row(2, "2.1.204", None),
        ];

        let mut table = render(&rows);
        table.force_no_tty().set_width(120);
        let drawn = visible(&table.to_string());

        assert!(
            drawn.contains(SESSION_A),
            "the table must show the recorded id, found:\n{drawn}"
        );
        assert!(
            drawn.contains(ABSENT),
            "the table must mark the session that recorded nothing, found:\n{drawn}"
        );
    }

    #[test]
    fn a_named_session_is_shown_by_its_id() {
        let session = session_id(SESSION_A);
        assert_eq!(render_session(Some(&session)).content(), SESSION_A);
    }

    #[test]
    fn a_session_that_recorded_nothing_is_shown_as_absent() {
        // A guess in this cell is wrong more often than it is right, and
        // nothing in a column of identifiers would say which one it was.
        assert_eq!(render_session(None).content(), ABSENT);
    }

    #[test]
    fn the_footer_counts_the_running_sessions() {
        assert_eq!(
            footer(&report(vec![row(
                1,
                "2.1.204",
                Some(session_id(SESSION_A))
            )])),
            ["1 running session."]
        );
        assert_eq!(
            footer(&report(vec![
                row(1, "2.1.204", Some(session_id(SESSION_A))),
                row(2, "2.1.205", Some(session_id(SESSION_B))),
            ])),
            ["2 running sessions."]
        );
    }

    #[test]
    fn the_footer_counts_the_sessions_that_wrote_no_record() {
        // The table leaves those cells blank, so the footer is the only place
        // that says how many of the sessions it could not name.
        assert_eq!(
            footer(&report(vec![
                row(1, "2.1.204", Some(session_id(SESSION_A))),
                row(2, "2.1.205", None),
            ])),
            [
                "2 running sessions.",
                "1 of these sessions wrote no record in ~/.claude/sessions and is not named here."
            ]
        );
        assert_eq!(
            footer(&report(vec![
                row(1, "2.1.204", None),
                row(2, "2.1.205", None),
            ])),
            [
                "2 running sessions.",
                "2 of these sessions wrote no record in ~/.claude/sessions and are not named here."
            ]
        );
    }

    #[test]
    fn the_footer_counts_the_support_processes_it_leaves_out() {
        let one = Report {
            sessions: vec![row(1, "2.1.204", Some(session_id(SESSION_A)))],
            support: 1,
            unreadable: 0,
        };
        assert_eq!(
            footer(&one),
            [
                "1 running session.",
                "1 Claude Code support process not shown."
            ]
        );

        let several = Report { support: 3, ..one };
        assert_eq!(
            footer(&several),
            [
                "1 running session.",
                "3 Claude Code support processes not shown."
            ]
        );
    }

    #[test]
    fn the_footer_counts_the_processes_it_cannot_read() {
        // A machine reported as clean while sixty processes of another account
        // run on it is the failure this line prevents.
        let one = Report {
            sessions: vec![row(1, "2.1.204", Some(session_id(SESSION_A)))],
            support: 0,
            unreadable: 1,
        };
        assert_eq!(
            footer(&one),
            [
                "1 running session.",
                "1 Claude Code process belongs to another account and cannot be read."
            ]
        );

        let several = Report {
            unreadable: 2,
            ..one
        };
        assert_eq!(
            footer(&several),
            [
                "1 running session.",
                "2 Claude Code processes belong to another account and cannot be read."
            ]
        );
    }

    #[test]
    fn the_footer_says_nothing_about_what_it_left_out_when_it_left_nothing_out() {
        // Every count is zero and every session is named, so the count of
        // sessions is the whole footer.
        assert_eq!(
            footer(&report(vec![
                row(1, "2.1.204", Some(session_id(SESSION_A))),
                row(2, "2.1.205", Some(session_id(SESSION_B))),
            ])),
            ["2 running sessions."]
        );
    }
}
