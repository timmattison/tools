//! Tests of the progress display that `Console` draws on standard error.
//!
//! A display is a matter of bytes on a terminal, so each test starts this test
//! binary again as a child, gives the child a pseudo-terminal, and reads what
//! the child wrote there. The child is the test `progress_child`. It does
//! nothing unless the environment names a scenario. The parent then runs a
//! small terminal model, [`screen`], over the bytes and asserts on the rows
//! that a person would see.
//!
//! No test here waits for a clock. The delay of the display is a parameter, so
//! a test sets it to zero to see the display at once, or to one hour to see
//! that nothing shows before the verdict.

#![cfg(unix)]

use std::process::{Command, Output};

use anyhow::anyhow;
use gitscratch::testing::pty::Pty;
use gitscratch::testing::{child_ran, CHILD_RAN};
use gitscratch::{Conflicts, Console, Phase, ReplayObserver, ReplayProgress};

/// The environment variable that names the scenario of the child.
const SCENARIO: &str = "GITSCRATCH_PROGRESS_SCENARIO";

/// The environment variable that holds the delay of the display, in
/// milliseconds.
const DELAY_MS: &str = "GITSCRATCH_PROGRESS_DELAY_MS";

/// The environment variable that is `1` when the child runs with `-q`.
const QUIET: &str = "GITSCRATCH_PROGRESS_QUIET";

/// The sentence of the replay that the child gives to the display.
const ACTION: &str = "replaying HEAD onto main";

/// The verdict that the child says.
const VERDICT: &str = "no conflicts would occur";

/// The note that the child says.
const NOTE: &str = "note: uncommitted work is not replayed";

/// The failure that the child answers with.
const FAILURE: &str = "the branch does not resolve";

/// The width of the terminal of every run here.
const COLUMNS: u16 = 80;

/// The escape sequence that clears one row.
const CLEAR_ROW: &str = "\u{1b}[2K";

/// The delay of one hour, in milliseconds. No run here lasts that long.
const ONE_HOUR_MS: &str = "3600000";

/// The child half. It does nothing unless the environment names a scenario.
///
/// The body gives the display the three kinds of report, and then ends as the
/// scenario says. The reports come before the end, so a display that shows at
/// all has something on the screen when the verdict arrives.
#[test]
fn progress_child() {
    let Ok(scenario) = std::env::var(SCENARIO) else {
        return;
    };
    let delay_ms: u64 = std::env::var(DELAY_MS)
        .expect("the parent must give the delay")
        .parse()
        .expect("the delay must be a number");
    let quiet = std::env::var(QUIET).is_ok_and(|value| value == "1");

    // The exit code carries no meaning here: the parent reads the bytes.
    let _ = Console::answer_with_progress_delay(
        "grind",
        quiet,
        std::time::Duration::from_millis(delay_ms),
        |console| {
            let progress = console.progress(ACTION);
            progress.report(ReplayProgress::Phase(Phase::CreateScratch));
            progress.report(ReplayProgress::Phase(Phase::Replay));
            progress.report(ReplayProgress::Commit {
                position: 3,
                total: 10,
            });
            match scenario.as_str() {
                "verdict" => {
                    console.verdict(VERDICT);
                    Ok(Conflicts::nothing_replayed())
                }
                "note" => {
                    console.note(NOTE);
                    console.verdict(VERDICT);
                    Ok(Conflicts::nothing_replayed())
                }
                "failure" => Err(anyhow!(FAILURE)),
                other => panic!("unknown scenario {other}"),
            }
        },
    );
    child_ran();
}

/// The command that runs the child half under `scenario`.
fn child(scenario: &str, delay_ms: &str, quiet: bool) -> Command {
    let mut command = childenv::fixture_command(
        std::env::current_exe().expect("path of the running test binary"),
    );
    command
        .args(["progress_child", "--exact", "--nocapture"])
        .env(SCENARIO, scenario)
        .env(DELAY_MS, delay_ms)
        .env(QUIET, if quiet { "1" } else { "0" })
        .env("COLUMNS", COLUMNS.to_string());
    command
}

/// Run the child half on a terminal that carries both output streams, and
/// check that the child half ran.
fn on_a_terminal(scenario: &str, delay_ms: &str, quiet: bool) -> Output {
    let output = Pty::open(COLUMNS).run_with_output_on_terminal(child(scenario, delay_ms, quiet));
    assert_ran(&output);
    output
}

/// Assert that the child half ran its own body, so a stale filter cannot pass.
fn assert_ran(output: &Output) {
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains(CHILD_RAN),
        "the child half must run its own body\nstatus: {}\nstdout: {text:?}\nstderr: {:?}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
}

/// The raw bytes of a run as text, for a search and for a message.
fn raw(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The lines that a terminal shows after it gets `bytes`.
///
/// A small model of a terminal. It keeps rows of characters and a cursor. A
/// newline goes to column 0 of the next row, and a carriage return goes to
/// column 0. The escape sequences `K` (erase in row), `A`, `B` (cursor up and
/// down) and `G` (cursor to a column) act. Every other escape sequence, such
/// as a color or the show and hide of the cursor, is discarded. Every other
/// control character is ignored. The rows come back with their trailing
/// spaces trimmed.
fn screen(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut rows: Vec<Vec<char>> = vec![Vec::new()];
    let mut row = 0;
    let mut column = 0;
    let mut chars = text.chars().peekable();

    while let Some(each) = chars.next() {
        match each {
            '\n' => {
                row += 1;
                column = 0;
                if rows.len() <= row {
                    rows.push(Vec::new());
                }
            }
            '\r' => column = 0,
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    let mut parameters = String::new();
                    let mut last = None;
                    for next in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            last = Some(next);
                            break;
                        }
                        parameters.push(next);
                    }
                    let number = parameters.parse::<usize>().ok();
                    match last {
                        Some('K') => match number.unwrap_or(0) {
                            0 => rows[row].truncate(column),
                            1 => {
                                for cell in rows[row].iter_mut().take(column + 1) {
                                    *cell = ' ';
                                }
                            }
                            _ => rows[row].clear(),
                        },
                        Some('A') => row = row.saturating_sub(number.unwrap_or(1)),
                        Some('B') => {
                            row += number.unwrap_or(1);
                            while rows.len() <= row {
                                rows.push(Vec::new());
                            }
                        }
                        Some('G') => column = number.unwrap_or(1).saturating_sub(1),
                        _ => {}
                    }
                }
                _ => {}
            },
            control if control.is_control() => {}
            printable => {
                let cells = &mut rows[row];
                while cells.len() < column {
                    cells.push(' ');
                }
                if column < cells.len() {
                    cells[column] = printable;
                } else {
                    cells.push(printable);
                }
                column += 1;
            }
        }
    }

    rows.into_iter()
        .map(|cells| cells.into_iter().collect::<String>().trim_end().to_string())
        .collect()
}

/// A carriage return and a clear of the row let the next text take the row.
#[test]
fn the_screen_model_lets_a_clear_and_a_carriage_return_overwrite_a_row() {
    let rows = screen(b"working 3/10\r\x1b[2Kdone\n");

    assert_eq!(rows, vec!["done".to_string(), String::new()]);
}

/// A color sequence takes no cell.
#[test]
fn the_screen_model_discards_a_color_sequence() {
    let rows = screen(b"\x1b[31mred\x1b[0m and \x1b[?25lplain");

    assert_eq!(rows, vec!["red and plain".to_string()]);
}

/// With a terminal on standard error, the bar shows, and the verdict line holds
/// no part of it.
///
/// The raw bytes hold `3/10 commits`, so the display drew. The model of the
/// terminal then holds a row that is exactly the verdict, so the display
/// cleared its row before the verdict took it.
#[test]
fn on_a_terminal_the_bar_shows_and_the_verdict_line_holds_no_part_of_it() {
    let output = on_a_terminal("verdict", "0", false);

    assert!(
        raw(&output).contains("3/10 commits"),
        "the bar must show\n{:?}",
        raw(&output)
    );
    assert!(
        screen(&output.stdout).iter().any(|row| row == VERDICT),
        "a row must hold the verdict and nothing else\n{:?}",
        screen(&output.stdout)
    );
}

/// With `-q`, the display writes no byte.
///
/// The delay is zero, so a display that `-q` did not hide would draw at once.
/// The raw bytes then hold the action, the word `commits`, a clear of a row,
/// or a block element of the bar.
#[test]
fn with_quiet_the_display_writes_no_byte() {
    let output = on_a_terminal("verdict", "0", true);

    let text = raw(&output);
    for forbidden in [ACTION, "commits", CLEAR_ROW, "█"] {
        assert!(
            !text.contains(forbidden),
            "-q must hide the display, but the bytes hold {forbidden:?}\n{text:?}"
        );
    }
}

/// A display that waits for its delay writes nothing before the verdict.
///
/// The delay is one hour, and the run ends in far less. The proof needs no
/// clock in the test: the run ends before the delay, so the display never
/// shows. The model of the terminal still holds the verdict.
#[test]
fn a_display_that_waits_for_its_delay_writes_nothing_before_the_verdict() {
    let output = on_a_terminal("verdict", ONE_HOUR_MS, false);

    let text = raw(&output);
    assert!(
        screen(&output.stdout).iter().any(|row| row == VERDICT),
        "a row must hold the verdict\n{text:?}"
    );
    for forbidden in [ACTION, "commits"] {
        assert!(
            !text.contains(forbidden),
            "the display must wait for its delay, but the bytes hold {forbidden:?}\n{text:?}"
        );
    }
}
