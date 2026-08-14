//! `occ` — "old Claude Code": list the Claude Code sessions running on this
//! machine, oldest release first.

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;
use comfy_table::{Cell, ContentArrangement, Table};
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
fn render(sessions: &[SessionReport]) -> Table {
    // The releases at the two ends of the report drive the colouring, so a
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
            None => ABSENT.dimmed().to_string(),
            Some(version) if Some(version) == oldest && oldest != newest => {
                version.as_str().red().bold().to_string()
            }
            Some(version) if Some(version) == newest => version.as_str().green().to_string(),
            Some(version) => version.as_str().yellow().to_string(),
        };
        let directory = row
            .directory
            .as_ref()
            .map_or_else(|| ABSENT.dimmed().to_string(), |d| d.display().to_string());

        table.add_row([
            Cell::new(row.pid.to_string()),
            Cell::new(release),
            Cell::new(format_uptime(row.uptime_secs)),
            Cell::new(render_session(&row.session)),
            Cell::new(directory),
        ]);
    }
    table
}

/// Renders the session cell, marking anything that was inferred.
///
/// An inferred id is dimmed and an unresolved one says why, so a reader is never
/// shown a firm-looking id that `occ` only guessed at.
fn render_session(session: &Session) -> String {
    match session {
        Session::Named(id) => id.to_string(),
        Session::Matched(id) => id.to_string().dimmed().to_string(),
        Session::Likely { id, of } => format!("{id} (newest of {of})").dimmed().to_string(),
        Session::Ambiguous { candidates, peers } => {
            format!("? {candidates} of {peers}").dimmed().to_string()
        }
        Session::Unknown => ABSENT.dimmed().to_string(),
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

    /// The width a line occupies on screen, which is what a reader sees.
    ///
    /// Escape sequences move no cursor and occupy no column, so they do not
    /// count. A table whose rows disagree here has a ragged right edge.
    fn visible_width(line: &str) -> usize {
        let mut width = 0;
        let mut characters = line.chars();
        while let Some(character) = characters.next() {
            if character != '\u{1b}' {
                width += 1;
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
        width
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
        table.force_no_tty().enforce_styling().set_width(120);
        let rendered = table.to_string();

        let widths: BTreeSet<usize> = rendered.lines().map(visible_width).collect();
        assert_eq!(
            widths.len(),
            1,
            "every line must occupy the same width, found {widths:?} in:\n{rendered}"
        );
    }
}
