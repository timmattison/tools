//! The size of the terminal window a tool draws in, and whether to paint color
//! into it.
//!
//! A tool that draws rows into a terminal must know how many columns and how
//! many rows it has. Three inputs answer that, and no one of them answers it
//! alone. The first input is the size the tool measures through the terminal
//! itself. The second input is the size a wrapper such as `viddy(1)` exports in
//! `COLUMNS` and in `LINES`. The third input tells the tool whether its own
//! standard output is a terminal.
//!
//! The three inputs disagree, and the third input decides which of the other
//! two is correct. A tool that writes to a terminal takes the measured size,
//! because that size is the size of the window at this moment. A tool that a
//! wrapper runs writes into a pipe, and a pipe measures nothing. The wrapper
//! measures the terminal for the tool and exports the two counts, so the tool
//! must take them instead. But the wrapper keeps rows of the terminal for its
//! own header and its own status bar, so the exported counts are larger than
//! the area the tool gets. This crate removes that difference.
//!
//! A wrapper causes a second problem. The `colored` crate paints no color into
//! a pipe, because a pipe usually goes to a file. Here the pipe goes to the
//! wrapper, and the wrapper draws the bytes it reads on a terminal. So the tool
//! must turn the color on again, and [`should_force_colors`] tells it when.
//!
//! # Why the answers stand in one crate
//!
//! `gsw` wrote all of this first, and it wrote it in its own binary crate. A
//! binary crate builds a program and it builds no library, so no other tool of
//! this workspace can read one line of it. `wn` draws in the same terminal,
//! under the same wrappers, so `wn` needs the same answers. The only way to
//! reach them was to write them a second time.
//!
//! A second copy is worse than the one it replaces. The two copies agree on the
//! plain input, and they part company at the edges. The edges hold the whole of
//! the correctness: the margin of one column that keeps a full-width row off the
//! next line, the rows a wrapper keeps for its chrome, the wrapper that reports
//! a height of one row, and the offset a user gives to remove more columns than
//! that. Each of those is one number, and a tool that takes the number of
//! another tool clips the bottom of its own output.

use std::io::IsTerminal;

/// Decide the effective terminal width gsw should render for.
///
/// Always leaves one cell of margin against the detected column count:
/// - Direct TTY: rendering a row exactly `cols` cells wide collides with
///   DECAWM auto-wrap quirks and right-edge chrome (scrollbars, padding)
///   on many terminals, pushing the last glyph onto the next line. The
///   margin keeps the rightmost cell empty.
/// - Watch-like wrapper (stdout not a TTY but `COLUMNS` set, e.g. viddy):
///   `COLUMNS` reports the full terminal width but the wrapper renders
///   into a content area one column narrower (its scroll indicator).
/// - Fallback (no signal): treat the implicit 80-column default the same
///   way for consistency.
///
/// `width_offset` always stacks on top, and the result is at least 1.
#[must_use]
pub fn effective_terminal_width(
    tty_width: Option<usize>,
    columns_env: Option<usize>,
    stdout_is_tty: bool,
    width_offset: usize,
) -> usize {
    let detected = match (stdout_is_tty, columns_env) {
        (false, Some(cols)) => cols,
        _ => tty_width.unwrap_or(DEFAULT_TERMINAL_WIDTH),
    };
    detected
        .saturating_sub(1)
        .saturating_sub(width_offset)
        .max(1)
}

/// Rows a watch-like wrapper paints for its own chrome (header, status/help
/// bar, surrounding padding) before and after our output. The wrapper exports
/// the *full* terminal height via `LINES` but only hands the command a smaller
/// content area, so we reserve these rows or the bottom of our frame — the
/// file list — gets clipped below the fold.
///
/// Measured empirically for viddy 1.3.0 (gsw's primary wrapper, per Cargo.toml):
/// a 30-row terminal shows exactly 26 lines of command output, i.e. 4 rows of
/// chrome, and this holds constant across terminal heights (20→16, 40→36).
/// `watch(1)` uses fewer (~2); reserving the larger value only leaves a couple
/// of harmless blank rows there, whereas reserving too few clips real content.
pub const WRAPPER_CHROME_ROWS: usize = 4;

/// Width assumed when no terminal-size signal is available at all (stdout is
/// piped and the wrapper didn't export `COLUMNS`). The classic 80-column
/// default; the one-cell DECAWM safety margin still applies on top.
pub const DEFAULT_TERMINAL_WIDTH: usize = 80;

/// Height assumed when no terminal-size signal is available at all (stdout is
/// piped and the wrapper didn't export `LINES`). Matches the classic VT100
/// default and the width fallback's spirit.
pub const DEFAULT_TERMINAL_HEIGHT: usize = 24;

/// Decide how many terminal rows gsw should fit its output within.
///
/// Mirrors [`effective_terminal_width`]: when stdout is captured by a
/// watch-like wrapper (not a TTY) that exports `LINES`, trust that height —
/// minus [`WRAPPER_CHROME_ROWS`] for the wrapper's own header — because
/// `termsize::stdout_size` can't see through the pipe. With a direct TTY, use the
/// queried height. With no signal at all, fall back to
/// [`DEFAULT_TERMINAL_HEIGHT`].
#[must_use]
pub fn effective_terminal_height(
    tty_height: Option<usize>,
    lines_env: Option<usize>,
    stdout_is_tty: bool,
) -> usize {
    match (stdout_is_tty, lines_env) {
        (false, Some(lines)) => lines.saturating_sub(WRAPPER_CHROME_ROWS).max(1),
        _ => tty_height.unwrap_or(DEFAULT_TERMINAL_HEIGHT),
    }
}

/// Should `colored::control::set_override(true)` be called?
///
/// True only when output is captured by a watch-like wrapper (stdout is not
/// a TTY *and* `COLUMNS` is set in env), and the user has not asked to
/// suppress colors via `NO_COLOR`. The wrapper renders the captured bytes
/// inside its own TTY-backed UI, so colors should pass through.
#[must_use]
pub fn should_force_colors(
    stdout_is_tty: bool,
    columns_env_present: bool,
    no_color_env: bool,
) -> bool {
    !stdout_is_tty && columns_env_present && !no_color_env
}

/// The variable through which a wrapper states the width of its terminal.
const WIDTH_VARIABLE: &str = "COLUMNS";

/// The variable that refuses color with any value, as <https://no-color.org>
/// asks.
const NO_COLOR_VARIABLE: &str = "NO_COLOR";

/// The variable that refuses color when it holds the string `0`.
const CLICOLOR_VARIABLE: &str = "CLICOLOR";

/// Whether this process must turn on the color of the `colored` crate itself.
/// The function reads the standard output and the environment of this process.
///
/// `colored` already decides color for a terminal and for the usual variables.
/// `CLICOLOR_FORCE` turns color on, also into a pipe. `NO_COLOR` turns it off.
/// With neither variable, color is on only when standard output is a terminal.
/// This function adds one rule to those, the rule of a wrapper, through
/// [`should_force_colors`].
///
/// A wrapper such as `viddy(1)` gives the tool a pipe and states the width of
/// its terminal in `COLUMNS`. The wrapper shows the bytes that it reads on that
/// terminal. `colored` sees only the pipe and writes no code, so the tool must
/// turn color on for that shape. This function answers `true` for it.
///
/// `COLUMNS` states a width only when it holds a number above zero that fits in
/// a `u16`. That is the rule by which `termbar::TerminalWidth` takes a stated
/// width, so the color and the layout of one run read one width. An empty
/// value, a value that is no number, `0`, and a value above `65535` state no
/// width. A tool that reads its width through `termbar` then measures its
/// terminal, and this function leaves the pipe plain. This crate has no
/// dependencies, so the parse is a second statement of the rule of `termbar`,
/// and the unit tests of this crate hold it.
///
/// Two variables refuse the rule of a wrapper. Each one is a choice that the
/// user made, and the rule of a wrapper extends only the case in which the user
/// made none. `NO_COLOR` set to any value refuses it, as it refuses color
/// everywhere else. `CLICOLOR` set to `0` refuses it too. `colored` reads
/// `CLICOLOR` as off exactly when its value is the string `0`.
///
/// # The call that uses the answer
///
/// Call `colored::control::set_override(true)` when this function answers
/// `true`, and call nothing when it answers `false`. Never give the answer to
/// `set_override` directly. On a terminal the answer is `false`, and
/// `set_override(false)` turns off the color that `colored` gives a terminal by
/// itself. It also wins over `CLICOLOR_FORCE`.
///
/// The call stays in the tool, with
/// `#[allow(clippy::disallowed_methods, reason = "...")]` at the call site. The
/// workspace bans `set_override`, because a test that calls it changes the
/// color of each test in its process. A tool that decides its own color at
/// startup is the one legitimate caller, and the allow says so in the file of
/// that tool. Only the decision stands here.
///
/// ```ignore
/// if termwindow::should_force_colors_here() {
///     #[allow(clippy::disallowed_methods, reason = "...")]
///     colored::control::set_override(true);
/// }
/// ```
#[must_use]
pub fn should_force_colors_here() -> bool {
    should_force_colors(
        std::io::stdout().is_terminal(),
        states_a_width(std::env::var(WIDTH_VARIABLE).ok().as_deref()),
        refuses_color(
            std::env::var_os(NO_COLOR_VARIABLE).is_some(),
            std::env::var(CLICOLOR_VARIABLE).ok().as_deref(),
        ),
    )
}

/// Whether `value`, the value of `COLUMNS`, states a width.
///
/// A width is a number above zero that fits in a `u16`. The `TIOCGWINSZ`
/// ioctl answers in that type, so a larger value is a width that no terminal
/// reports. No character of a line prints into no column, so zero states no
/// width either. A value that is not valid text arrives here as `None`,
/// because the read takes the variable as text. That is the rule of
/// `termbar::TerminalWidth`, whose `stated_of` and `columns_of` hold it.
fn states_a_width(value: Option<&str>) -> bool {
    value.is_some()
}

/// Whether the user refused color, from whether `NO_COLOR` is set and from the
/// value of `CLICOLOR`.
fn refuses_color(no_color_set: bool, clicolor: Option<&str>) -> bool {
    no_color_set || clicolor == Some("0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_uses_columns_minus_one_when_stdout_not_tty() {
        // viddy case: pipes captured, COLUMNS exported.
        assert_eq!(effective_terminal_width(None, Some(120), false, 0), 119);
    }

    #[test]
    fn height_uses_lines_env_minus_wrapper_chrome_when_stdout_not_tty() {
        // viddy/watch case: stdout piped, LINES exported. We budget to the
        // wrapper's height minus its title chrome so the bottom file list
        // isn't clipped below the wrapper's header.
        assert_eq!(
            effective_terminal_height(None, Some(40), false),
            40 - WRAPPER_CHROME_ROWS,
        );
    }

    #[test]
    fn height_uses_tty_height_when_stdout_is_tty() {
        // Interactive: trust the ioctl-reported height and ignore any stale
        // inherited LINES value.
        assert_eq!(effective_terminal_height(Some(50), Some(9999), true), 50);
    }

    #[test]
    fn height_falls_back_to_default_when_no_signal() {
        // Piped with no LINES exported: nothing to go on, so assume the
        // classic 24-row terminal.
        assert_eq!(
            effective_terminal_height(None, None, false),
            DEFAULT_TERMINAL_HEIGHT,
        );
    }

    #[test]
    fn height_never_collapses_to_zero_under_tiny_wrapper() {
        // A pathologically short wrapper height must still leave at least one
        // row rather than underflowing to zero.
        assert_eq!(effective_terminal_height(None, Some(1), false), 1);
    }

    #[test]
    fn width_leaves_safety_margin_when_stdout_is_tty() {
        // Direct TTY: terminal_size reports the full column count, but if
        // gsw renders a row exactly that many cells wide, terminals with
        // auto-wrap (DECAWM) or right-edge chrome (scrollbars, padding)
        // push the rightmost glyph onto the next line — the user sees the
        // last character of the age column wrap. Leave one cell of margin,
        // matching the viddy path so direct and viddy renderings agree.
        assert_eq!(effective_terminal_width(Some(200), None, true, 0), 199);
    }

    #[test]
    fn width_uses_tty_width_when_stdout_is_tty() {
        // Interactive: trust the ioctl-reported width, not the env — but
        // still subtract the one-cell safety margin.
        assert_eq!(effective_terminal_width(Some(200), None, true, 0), 199);
    }

    #[test]
    fn width_ignores_columns_when_stdout_is_tty() {
        // If a shell leaked COLUMNS into our env but we have a real TTY,
        // the TTY measurement wins.
        assert_eq!(effective_terminal_width(Some(200), Some(120), true, 0), 199);
    }

    #[test]
    fn width_falls_back_to_eighty_minus_margin_when_no_signal() {
        // Piped to a plain file with no COLUMNS in env: nothing to go on,
        // so fall back to the 80-column default. The safety margin still
        // applies so the fallback matches the detected paths.
        assert_eq!(effective_terminal_width(None, None, false, 0), 79);
    }

    #[test]
    fn width_offset_stacks_on_top_of_detection() {
        // 200 (TTY) - 1 (safety margin) - 3 (offset) = 196
        assert_eq!(effective_terminal_width(Some(200), None, true, 3), 196);
        // 120 (COLUMNS) - 1 (safety margin) - 2 (offset) = 117
        assert_eq!(effective_terminal_width(None, Some(120), false, 2), 117);
    }

    #[test]
    fn width_never_drops_below_one() {
        // A pathologically large offset should clamp to 1, not underflow.
        assert_eq!(effective_terminal_width(Some(10), None, true, 999), 1);
    }

    #[test]
    fn force_colors_when_piped_to_wrapper_with_columns_env() {
        assert!(should_force_colors(false, true, false));
    }

    #[test]
    fn no_force_colors_when_interactive() {
        // TTY → let colored auto-detect (it will say yes anyway).
        assert!(!should_force_colors(true, true, false));
        assert!(!should_force_colors(true, false, false));
    }

    #[test]
    fn no_force_colors_when_piped_without_columns_env() {
        // Plain pipe to file: respect the colored crate's default (off).
        assert!(!should_force_colors(false, false, false));
    }

    #[test]
    fn no_force_colors_when_no_color_env_set() {
        // Honor https://no-color.org even when under viddy.
        assert!(!should_force_colors(false, true, true));
    }

    #[test]
    fn a_number_above_zero_in_columns_states_a_width() {
        assert!(states_a_width(Some("80")));
    }

    #[test]
    fn the_widest_width_a_terminal_reports_states_a_width() {
        // The TIOCGWINSZ ioctl answers in a u16, and this is its largest value.
        assert!(states_a_width(Some("65535")));
    }

    #[test]
    fn no_columns_states_no_width() {
        assert!(!states_a_width(None));
    }

    #[test]
    fn an_empty_columns_states_no_width() {
        // `COLUMNS=` is set, and it holds no number. A tool that reads its
        // width through termbar measures the terminal for it.
        assert!(!states_a_width(Some("")));
    }

    #[test]
    fn a_columns_that_is_no_number_states_no_width() {
        assert!(!states_a_width(Some("abc")));
    }

    #[test]
    fn a_columns_of_zero_states_no_width() {
        // No character of a line prints into no column.
        assert!(!states_a_width(Some("0")));
    }

    #[test]
    fn a_columns_wider_than_a_u16_states_no_width() {
        // No terminal reports a width that the TIOCGWINSZ ioctl cannot hold.
        assert!(!states_a_width(Some("70000")));
    }

    #[test]
    fn no_color_with_any_value_refuses_color() {
        assert!(refuses_color(true, None));
        assert!(refuses_color(true, Some("1")));
    }

    #[test]
    fn clicolor_zero_refuses_color() {
        assert!(refuses_color(false, Some("0")));
    }

    #[test]
    fn clicolor_other_than_zero_refuses_nothing() {
        // colored reads CLICOLOR as off exactly when it holds the string 0.
        assert!(!refuses_color(false, Some("1")));
        assert!(!refuses_color(false, Some("")));
    }

    #[test]
    fn no_variable_refuses_nothing() {
        assert!(!refuses_color(false, None));
    }
}
