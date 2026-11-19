// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

//! The `cctop` binary. It wires the library to the terminal: one channel of
//! events, one loop that draws, and one thread for each job that can block.

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cctop::controller::{AppEvent, Controller, Effect};
use cctop::focus::FocusTools;
use cctop::jobs::{bar_lock_for_user, EventSink, Jobs, NotifyGate};
use cctop::liveness::SystemClock;
use cctop::paths::ClaudeDirs;
use cctop::runner::{Runner, SystemRunner};
use cctop::state::EpochMillis;
use cctop::sync::Sync;
use cctop::ui;
use cctop::wezterm::DeviceWriter;
use clap::Parser;

/// The exit code of a refused start.
const EXIT_USAGE: u8 = 2;

/// The exit code of a failure at run time.
const EXIT_FAILURE: u8 = 1;

/// The time between two ticks.
const TICK: Duration = Duration::from_secs(1);

/// Claude Code top: one row for each live Claude Code session.
#[derive(Parser)]
#[command(
    name = "cctop",
    version = buildinfo::version_string!(),
    about = "Shows the state of each Claude Code session, and brings the session that needs you to the front"
)]
struct Cli {
    /// Send no macOS notification when a session changes to needs-you.
    #[arg(long)]
    no_notify: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if !std::io::stdout().is_terminal() {
        eprintln!(
            "cctop: standard output is not a terminal. cctop draws a full-screen view and has no plain-text mode."
        );
        return ExitCode::from(EXIT_USAGE);
    }
    let dirs = match ClaudeDirs::from_process_env() {
        Ok(dirs) => dirs,
        Err(error) => {
            eprintln!("cctop: {error}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    match run(&dirs, !cli.no_notify) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cctop: {message}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

/// Start the watcher, load the sessions, and run the loop. The terminal is
/// back in its normal state when this function returns.
fn run(dirs: &ClaudeDirs, notify: bool) -> Result<(), String> {
    let state_dir = dirs.state_dir();
    std::fs::create_dir_all(&state_dir)
        .map_err(|error| format!("cannot create {}: {error}", state_dir.display()))?;
    let (tx, rx) = mpsc::channel::<AppEvent>();
    // The watcher starts first, so no change falls between the read and the watch.
    let watch_tx = tx.clone();
    let _watcher = cctop::store::watch(&state_dir, move |event| {
        let _ = watch_tx.send(AppEvent::Dir(event));
    })
    .map_err(|error| format!("cannot watch {}: {error}", state_dir.display()))?;
    let mut controller = Controller::new(Sync::new(dirs), Box::new(SystemClock::new()), notify);
    controller
        .start(EpochMillis::now())
        .map_err(|error| format!("cannot read {}: {error}", state_dir.display()))?;
    let input_tx = tx.clone();
    std::thread::spawn(move || {
        while let Ok(event) = crossterm::event::read() {
            if input_tx.send(AppEvent::Terminal(event)).is_err() {
                break;
            }
        }
    });
    let runner: Arc<dyn Runner> = Arc::new(SystemRunner);
    let tools = FocusTools {
        runner: Arc::clone(&runner),
        writer: Arc::new(DeviceWriter),
    };
    let gate = bar_lock_for_user().map_or(NotifyGate::Always, NotifyGate::UnlessHeld);
    let sink: EventSink = Arc::new(move |event| {
        let _ = tx.send(event);
    });
    let jobs = Jobs::new(tools, gate, sink);
    // The session clears the screen, restores the terminal on every exit, and
    // installs a panic hook that restores it too.
    let mut session = altscreen::RatatuiSession::enter(altscreen::Options::default())
        .map_err(|error| format!("cannot enter the alternate screen: {error}"))?;
    let terminal = session.terminal();
    let size = terminal
        .size()
        .map_err(|error| format!("cannot read the size of the terminal: {error}"))?;
    controller.set_terminal_height(size.height);
    let mut next_tick = Instant::now() + TICK;
    loop {
        terminal
            .draw(|frame| ui::draw(frame, controller.app(), EpochMillis::now()))
            .map_err(|error| format!("cannot draw: {error}"))?;
        let wait = next_tick.saturating_duration_since(Instant::now());
        let event = match rx.recv_timeout(wait) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                next_tick += TICK;
                if next_tick < Instant::now() {
                    next_tick = Instant::now() + TICK;
                }
                AppEvent::Tick
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        for effect in controller.handle(event, EpochMillis::now()) {
            match effect {
                Effect::Quit => return Ok(()),
                Effect::Focus(target) => jobs.spawn_focus(target),
                Effect::Notify(message) => jobs.spawn_notify(message),
            }
        }
    }
}
