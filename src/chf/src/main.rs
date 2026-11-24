//! The `chf` binary.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use chf::app::{App, Effect, Version};
use chf::index::{self, IndexEvent};
use chf::launch;
use chf::render::{self, Context};
use chf::search::Searcher;
use clap::Parser;
use claude_transcript::SessionId;

/// The exit status of a run that fails.
const EXIT_FAILURE: u8 = 1;

/// The exit status of a run that has no terminal or no home directory.
const EXIT_USAGE: u8 = 2;

/// The exit status of a run that the person cancelled.
const EXIT_CANCELLED: u8 = 130;

/// The most index events that one turn of the loop takes.
const MAX_INDEX_EVENTS_PER_TURN: usize = 5_000;

/// How long one turn of the loop waits for a key.
const KEY_WAIT: Duration = Duration::from_millis(50);

/// The command line of `chf`.
#[derive(Parser)]
#[command(
    name = "chf",
    version = buildinfo::version_string!(),
    about = "Find a Claude Code session by the words in its messages, and resume it",
    long_about = "Find a Claude Code session by the words in its messages, and resume it.\n\n\
Type words to search. Up, Down, Ctrl-P and Ctrl-N move the selection. \
PageUp and PageDown move one page. Shift-Up and Shift-Down scroll the preview. \
Ctrl-U clears the query. Enter resumes the session. Esc and Ctrl-C quit.\n\n\
For a session that Claude Code compacted, chf asks a question. Press c to resume \
with ch (the compacted version). Press e to resume with che (the expanded version)."
)]
struct Cli {}

/// What the loop decides when it ends.
enum Outcome {
    /// The person quit.
    Quit,
    /// The person chose this session, in this version, to resume.
    Launch { id: SessionId, version: Version },
    /// The terminal failed with this error.
    DrawFailed(std::io::Error),
}

/// Takes the index events that wait, at most [`MAX_INDEX_EVENTS_PER_TURN`].
fn waiting_index_events(events: &Receiver<IndexEvent>) -> Vec<IndexEvent> {
    let mut batch = Vec::new();
    while batch.len() < MAX_INDEX_EVENTS_PER_TURN {
        match events.try_recv() {
            Ok(event) => batch.push(event),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }
    batch
}

/// Runs the picker until the person quits or chooses a session.
fn run_picker(home: &Path, events: &Receiver<IndexEvent>, searcher: &Searcher) -> Outcome {
    // The session erases the screen in the write that enters it, restores the
    // terminal on every way out of this function, and installs a panic hook
    // that restores it too.
    let mut session = match altscreen::RatatuiSession::enter(altscreen::Options::default()) {
        Ok(session) => session,
        Err(error) => return Outcome::DrawFailed(error),
    };
    let terminal = session.terminal();
    let mut app = App::new();
    loop {
        let batch = waiting_index_events(events);
        let mut effects = if batch.is_empty() {
            Vec::new()
        } else {
            app.on_index(batch)
        };
        while let Some(hits) = searcher.try_recv() {
            app.on_hits(hits);
        }
        let context = Context {
            home: Some(home),
            tz: &chrono::Local,
        };
        match terminal.draw(|frame| {
            let drawn = render::draw(frame, &app, &context);
            app.on_drawn(drawn);
        }) {
            Ok(_) => {}
            Err(error) => return Outcome::DrawFailed(error),
        }
        match crossterm::event::poll(KEY_WAIT) {
            Ok(true) => loop {
                if let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read() {
                    effects.extend(app.on_key(key));
                }
                if !matches!(crossterm::event::poll(Duration::ZERO), Ok(true)) {
                    break;
                }
            },
            Ok(false) => {}
            Err(error) => return Outcome::DrawFailed(error),
        }
        for effect in effects {
            match effect {
                Effect::Search(job) => searcher.submit(job),
                Effect::Quit => return Outcome::Quit,
                Effect::Launch { id, version } => return Outcome::Launch { id, version },
            }
        }
    }
}

/// Resumes a session through the shell. It returns only when that fails.
fn resume(id: &SessionId, version: Version) -> ExitCode {
    let shell = std::env::var_os("SHELL");
    if launch::shell_of(shell.as_deref()).is_none() {
        eprintln!("chf: $SHELL is not bash or zsh, so chf cannot run the crap shell function.");
        eprintln!("Paste this command into your shell:");
        eprintln!("{}", launch::paste_command(id, version));
        return ExitCode::from(EXIT_FAILURE);
    }
    let shell = shell.unwrap_or_default();
    let error = launch::exec(Path::new(&shell), &launch::script(id, version));
    eprintln!("chf: cannot start {}: {error}", Path::new(&shell).display());
    ExitCode::from(EXIT_FAILURE)
}

fn main() -> ExitCode {
    let _cli = Cli::parse();
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!(
            "chf: standard input and standard output must be a terminal. chf draws a full-screen picker."
        );
        return ExitCode::from(EXIT_USAGE);
    }
    let Some(home) = dirs::home_dir() else {
        eprintln!("chf: cannot find the home directory.");
        return ExitCode::from(EXIT_USAGE);
    };
    let projects = claude_transcript::projects_dir(&home);
    let cache = dirs::cache_dir()
        .unwrap_or_else(|| home.join(".cache"))
        .join("chf");
    let events = index::spawn(projects, cache);
    let searcher = Searcher::spawn();
    match run_picker(&home, &events, &searcher) {
        Outcome::Quit => ExitCode::from(EXIT_CANCELLED),
        Outcome::DrawFailed(error) => {
            eprintln!("chf: cannot draw the screen: {error}");
            ExitCode::from(EXIT_FAILURE)
        }
        Outcome::Launch { id, version } => resume(&id, version),
    }
}
