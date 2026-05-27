//! Watch mode: event loop, terminal lifecycle, and the pure helpers that drive
//! refresh decisions.
//!
//! The watch loop owns all rendering on a single thread and is fed by a
//! `std::sync::mpsc` channel. Every *decision* (which terminal dimensions to
//! render for, and — in later phases — which filesystem events matter and how
//! fast to tick) lives in a pure, terminal-free function here so it can be
//! unit-tested without a pty.

use std::io::{self, Write};
use std::sync::mpsc::{self, Sender};
use std::thread;

use anyhow::Result;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event as CtEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};

use crate::{
    build_output, effective_terminal_height, effective_terminal_width, RenderConfig,
    DEFAULT_TERMINAL_HEIGHT,
};

/// Which rendering mode `gsw` is running in. The mode — not ambient env
/// detection — decides where terminal dimensions come from (see
/// [`resolve_dimensions`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// Single render and exit. Honors the viddy-aware `COLUMNS`/`LINES` env
    /// logic so `gsw | …` and `viddy gsw` keep working unchanged.
    OneShot,
    /// Long-lived watch loop that owns the whole pane.
    Watch,
}

/// Resolved terminal dimensions to render within.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Dimensions {
    pub width: usize,
    pub height: usize,
}

/// Every raw signal available for resolving terminal dimensions, regardless of
/// mode. The resolver picks which of these to trust based on the mode.
#[derive(Clone, Copy)]
pub(crate) struct SizeInputs {
    /// Width queried from `terminal_size` (the ioctl), when a TTY is present.
    pub tty_width: Option<usize>,
    /// Height queried from `terminal_size` (the ioctl), when a TTY is present.
    pub tty_height: Option<usize>,
    /// `COLUMNS` env var, exported by watch-like wrappers (viddy).
    pub columns_env: Option<usize>,
    /// `LINES` env var, exported by watch-like wrappers (viddy).
    pub lines_env: Option<usize>,
    /// Whether stdout is a direct TTY.
    pub stdout_is_tty: bool,
    /// User-requested columns to subtract from the detected width.
    pub width_offset: usize,
}

/// Resolve the terminal dimensions `gsw` should render for, keyed off the mode.
///
/// - [`Mode::OneShot`] preserves the existing viddy-aware behavior: width and
///   height come from the `COLUMNS`/`LINES` env vars when stdout is captured by
///   a wrapper, reserving rows for the wrapper's chrome. This keeps `gsw | …`
///   and `viddy gsw` byte-identical to before.
/// - [`Mode::Watch`] owns the entire pane, so it takes width and height
///   straight from `terminal_size`, ignores `COLUMNS`/`LINES`, and reserves
///   **no** wrapper chrome rows. The one-cell width safety margin (DECAWM) and
///   the user's `width_offset` still apply.
pub(crate) fn resolve_dimensions(mode: Mode, inputs: &SizeInputs) -> Dimensions {
    match mode {
        Mode::OneShot => Dimensions {
            width: effective_terminal_width(
                inputs.tty_width,
                inputs.columns_env,
                inputs.stdout_is_tty,
                inputs.width_offset,
            ),
            height: effective_terminal_height(
                inputs.tty_height,
                inputs.lines_env,
                inputs.stdout_is_tty,
            ),
        },
        Mode::Watch => Dimensions {
            // Watch owns the whole pane: ignore COLUMNS/LINES, take the size
            // from terminal_size, and reserve no wrapper chrome. The one-cell
            // DECAWM safety margin and the user's width_offset still apply to
            // width, matching the one-shot path's right-edge behavior.
            width: inputs
                .tty_width
                .unwrap_or(80)
                .saturating_sub(1)
                .saturating_sub(inputs.width_offset)
                .max(1),
            height: inputs.tty_height.unwrap_or(DEFAULT_TERMINAL_HEIGHT).max(1),
        },
    }
}

/// Events the watch loop reacts to. The main thread owns all rendering and
/// blocks on a single channel carrying these. Phase 1 produces only terminal
/// events; later phases add filesystem-change and timer-tick variants.
enum Event {
    /// The terminal was resized — repaint at the new dimensions.
    Resize,
    /// The user asked to quit (`q` or Ctrl-C).
    Quit,
}

/// Run the live watch loop: take over the alternate screen, render once, then
/// repaint on terminal resize until the user quits with `q` or Ctrl-C.
///
/// The [`TerminalGuard`] restores the main screen and cursor on every exit
/// path (normal return, error, or panic), so the terminal can never be left
/// wedged. In this phase the only event producer is the crossterm reader
/// thread; filesystem watching and the decay timer arrive in later phases.
pub(crate) fn run(repo: &gix::Repository, cfg: &RenderConfig) -> Result<()> {
    let _guard = TerminalGuard::enter()?;

    let mut displayed = String::new();
    render_now(repo, cfg, &mut displayed)?;

    let (tx, rx) = mpsc::channel();
    spawn_event_reader(tx);

    // `recv` errors only once every sender has hung up (the reader thread
    // ended), which we treat as a clean shutdown just like an explicit Quit.
    while let Ok(event) = rx.recv() {
        match event {
            Event::Quit => break,
            Event::Resize => render_now(repo, cfg, &mut displayed)?,
        }
    }

    Ok(())
}

/// Recompute the output for the current terminal size and paint it, unless it
/// is byte-identical to what is already on screen (suppression — cheap here,
/// load-bearing once filesystem and timer events arrive).
fn render_now(repo: &gix::Repository, cfg: &RenderConfig, displayed: &mut String) -> Result<()> {
    let dims = current_dimensions(cfg.width_offset);
    let output = build_output(repo, cfg, dims)?;
    if output == *displayed {
        return Ok(());
    }

    let mut out = io::stdout();
    // In raw mode a bare '\n' moves down without returning to column 0, which
    // would stair-step the output; translate to CRLF. Clear first so a shorter
    // render can't leave stale glyphs from a taller previous frame.
    let painted = output.replace('\n', "\r\n");
    execute!(out, MoveTo(0, 0), Clear(ClearType::All))?;
    write!(out, "{painted}")?;
    out.flush()?;

    *displayed = output;
    Ok(())
}

/// Query the live terminal size and resolve watch-mode dimensions from it.
fn current_dimensions(width_offset: usize) -> Dimensions {
    let tty = terminal_size::terminal_size().map(|(w, h)| (usize::from(w.0), usize::from(h.0)));
    resolve_dimensions(
        Mode::Watch,
        &SizeInputs {
            tty_width: tty.map(|(w, _)| w),
            tty_height: tty.map(|(_, h)| h),
            columns_env: None,
            lines_env: None,
            stdout_is_tty: true,
            width_offset,
        },
    )
}

/// Spawn the crossterm event-reader thread. It blocks on `event::read`,
/// translating `q`/Ctrl-C into [`Event::Quit`] and terminal resizes into
/// [`Event::Resize`], and exits when the receiver is gone or reading fails.
fn spawn_event_reader(tx: Sender<Event>) {
    thread::spawn(move || loop {
        match event::read() {
            Ok(CtEvent::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            })) => {
                // Ignore key-release events (kitty/Windows report them); only
                // a press should quit.
                if kind == KeyEventKind::Release {
                    continue;
                }
                let quit = matches!(code, KeyCode::Char('q'))
                    || (modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(code, KeyCode::Char('c')));
                if quit && tx.send(Event::Quit).is_err() {
                    break;
                }
            }
            Ok(CtEvent::Resize(_, _)) => {
                if tx.send(Event::Resize).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            // The terminal closed or reading failed: nothing more to read.
            Err(_) => break,
        }
    });
}

/// RAII guard for the alternate screen, hidden cursor, and raw mode. Restores
/// the main screen and cursor on drop *and* via a panic hook, so no exit path
/// — normal return, propagated error, or panic — can leave the terminal in a
/// wedged state. The panic hook restores *before* the default handler prints,
/// so the panic message lands on the main screen rather than the torn-down
/// alternate one.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));

        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Best-effort restore of the terminal to its pre-watch state. Idempotent and
/// failure-tolerant: both the panic hook and `Drop` may call it (a panic runs
/// the hook, then unwinding runs `Drop`), and a partially-entered terminal
/// must still be cleaned up, so every step is independently ignored on error.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WRAPPER_CHROME_ROWS;

    #[test]
    fn one_shot_uses_env_dimensions_watch_uses_terminal_size() {
        // Deliberately make terminal_size (200x50) disagree with the env
        // (COLUMNS=120, LINES=40) so the *source* each mode picks is
        // unambiguous from the resulting numbers.
        let inputs = SizeInputs {
            tty_width: Some(200),
            tty_height: Some(50),
            columns_env: Some(120),
            lines_env: Some(40),
            stdout_is_tty: false, // viddy-like capture for the one-shot case
            width_offset: 0,
        };

        // One-shot trusts the env: COLUMNS-1 for width, LINES minus wrapper
        // chrome for height.
        let one_shot = resolve_dimensions(Mode::OneShot, &inputs);
        assert_eq!(one_shot.width, 119, "one-shot width must come from COLUMNS");
        assert_eq!(
            one_shot.height,
            40 - WRAPPER_CHROME_ROWS,
            "one-shot height must come from LINES minus wrapper chrome",
        );

        // Watch ignores the env entirely and takes terminal_size directly,
        // reserving no chrome: 200-1 wide, full 50 tall.
        let watch_inputs = SizeInputs {
            stdout_is_tty: true,
            ..inputs
        };
        let watch = resolve_dimensions(Mode::Watch, &watch_inputs);
        assert_eq!(
            watch.width, 199,
            "watch width must come from terminal_size, not COLUMNS",
        );
        assert_eq!(
            watch.height, 50,
            "watch height must come from terminal_size with no chrome reserved",
        );
    }
}
