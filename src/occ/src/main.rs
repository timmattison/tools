//! `occ` — "old Claude Code": list the Claude Code sessions running on this
//! machine, oldest release first.

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};
use occ::process::Role;
use occ::report::SessionReport;
use occ::session::Session;
use occ::{build, classify, format_uptime, gather_processes, ProjectTranscripts};

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
    let transcripts = ProjectTranscripts::for_home(&home);
    let sessions = build(&facts, &transcripts);

    // Counted so the footer can say what was left out. A tool that quietly drops
    // processes it cannot read would report a clean machine that is not clean.
    let mut support = 0_usize;
    let mut unreadable = 0_usize;
    for fact in &facts {
        match classify(fact) {
            Role::Support(_) => support += 1,
            Role::Unreadable => unreadable += 1,
            _ => {}
        }
    }

    if sessions.is_empty() {
        println!("No Claude Code sessions are running.");
    } else {
        println!("{}", render(&sessions));
    }
    print_footer(&sessions, support, unreadable);
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
            render_session(&row.session),
            directory,
        ]);
    }
    table
}

/// The cell shown in place of a value that could not be read.
fn absent() -> Cell {
    Cell::new(ABSENT).add_attribute(Attribute::Dim)
}

/// Renders the session cell, marking anything that was inferred.
///
/// An inferred id is dimmed and an unresolved one says why, so a reader is never
/// shown a firm-looking id that `occ` only guessed at.
fn render_session(session: &Session) -> Cell {
    match session {
        Session::Named(id) => Cell::new(id),
        Session::Matched(id) => Cell::new(id).add_attribute(Attribute::Dim),
        Session::Likely { id, of } => {
            Cell::new(format!("{id} (newest of {of})")).add_attribute(Attribute::Dim)
        }
        Session::Ambiguous { candidates, peers } => {
            Cell::new(format!("? {candidates} of {peers}")).add_attribute(Attribute::Dim)
        }
        Session::Unknown => absent(),
    }
}

/// Chooses the singular or the plural form for `count`.
fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

/// Prints the counts and the legend below the table.
fn print_footer(sessions: &[SessionReport], support: usize, unreadable: usize) {
    let inferred = sessions
        .iter()
        .filter(|row| matches!(row.session, Session::Matched(_) | Session::Likely { .. }))
        .count();
    let unresolved = sessions
        .iter()
        .filter(|row| matches!(row.session, Session::Ambiguous { .. } | Session::Unknown))
        .count();

    println!();
    println!(
        "{} running {}.",
        sessions.len().to_string().bold(),
        plural(sessions.len(), "session", "sessions")
    );

    if inferred > 0 || unresolved > 0 {
        println!(
            "{}",
            format!(
                "A session id is dimmed when it was matched by directory and release rather than \
                 read from the command line ({inferred} here). \"? N of M\" means N transcripts \
                 fit M competing sessions, so none can be named ({unresolved} unresolved)."
            )
            .dimmed()
        );
    }
    if support > 0 {
        println!(
            "{}",
            format!(
                "{support} Claude Code support {} not shown.",
                plural(support, "process", "processes")
            )
            .dimmed()
        );
    }
    if unreadable > 0 {
        println!(
            "{}",
            format!(
                "{unreadable} Claude Code {} {} to another account and cannot be read.",
                plural(unreadable, "process", "processes"),
                plural(unreadable, "belongs", "belong")
            )
            .dimmed()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::render;
    use occ::{ClaudeVersion, Session, SessionId, SessionReport};
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::{Mutex, PoisonError};

    /// Whether escape sequences are emitted is one setting for the whole
    /// process, so a test that forces them on holds this lock while it runs.
    static COLOR: Mutex<()> = Mutex::new(());

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

    fn row(pid: u32, release: &str, session: Session) -> SessionReport {
        SessionReport {
            pid,
            version: ClaudeVersion::parse(release),
            directory: Some(PathBuf::from("/work")),
            session,
            uptime_secs: 3_600,
        }
    }

    #[test]
    fn the_colored_table_keeps_a_straight_right_edge() {
        // Each row here is colored differently: the oldest release red and
        // bold, the middle one yellow, the newest green, one session id plain
        // and another dimmed. The escape sequences that carry those colors are
        // all of different lengths, and no reader sees any of them.
        let rows = [
            row(
                1,
                "2.1.196",
                Session::Named(session_id("d3b0d921-f0a1-41fc-b309-c11aa30c1173")),
            ),
            row(
                2,
                "2.1.200",
                Session::Matched(session_id("ed84c8c7-0117-4670-936c-98e0f0d2c80b")),
            ),
            row(3, "2.1.204", Session::Unknown),
        ];

        let _held = COLOR.lock().unwrap_or_else(PoisonError::into_inner);
        colored::control::set_override(true);
        let mut table = render(&rows);
        colored::control::unset_override();
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
}
