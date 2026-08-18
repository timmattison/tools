//! Test-only support for the ANSI escape codes the `colored` crate writes.
//!
//! The crate has two jobs. It decides whether the `colored` crate writes escape
//! codes at all, through one lock and one entrance. And it takes the codes back
//! out of text that already carries them, through [`strip_ansi`].
//!
//! # Why the codes come and go
//!
//! The `colored` crate decides at format time whether to write escape codes,
//! and one input to that decision is whether file descriptor 1 is a terminal.
//! A test that compares rendered text against plain text thus passes when the
//! output of the run goes to a file and fails when it goes to a terminal.
//! `cargo test` hands the test binary the terminal of whoever started it, so
//! such a test passes in a redirected run and fails under a hand-typed
//! `git commit`. A test that asserts on rendered text must therefore compare
//! visible glyphs rather than bytes, and it does that through [`strip_ansi`].
//!
//! Some tests want the opposite. They read a color out of the codes, so they
//! need the codes to be there whatever the run writes to. Those tests turn the
//! codes on with [`with_forced_ansi`].
//!
//! # Why this is a crate
//!
//! More than one crate in this workspace renders colored text and asserts on
//! what it rendered. Each of them needs the same two answers. One crate that
//! each of them takes as a dev-dependency keeps the answers identical, which
//! matters most for [`strip_ansi`]: two hand-written strippers agree on the
//! common sequences and part company on the rare ones, and the test that
//! notices is the one that fails once a year.
//!
//! # The lock is per process, and so is the override
//!
//! `cargo test` builds one test binary for each crate and runs the tests of one
//! binary on many threads at the same time. The override of the `colored` crate
//! is process-global, so two threads of one binary fight over it. One test that
//! turns the override on changes what a different test sees, and one test that
//! turns it off again removes the escapes a different test is in the middle of
//! reading. The private mutex in this crate serializes them.
//!
//! Each test binary links its own copy of this crate and thus gets its own
//! mutex. That is correct rather than a gap. A separate binary is a separate
//! process with a separate `colored` global, and two processes have nothing to
//! serialize.

use std::sync::{Mutex, PoisonError};

/// The one lock on the global override of the `colored` crate.
///
/// It is private on purpose. A caller that could lock it directly could also
/// hold it across an unrelated body, or forget to put the override back, which
/// is the exact failure this crate removes. [`with_forced_ansi`] holds it for
/// one body and gives it up at the end of that body.
static OVERRIDE: Mutex<()> = Mutex::new(());

/// Run `body` with the `colored` crate forced to write ANSI escape codes, and
/// give the process-global override back to its earlier state at the end.
///
/// The lock is held for the whole call, so two tests that both want real ANSI
/// bytes run one after the other. The override goes back even when `body`
/// panics, so one failed test leaves the next test a clean process.
///
/// A poisoned mutex is recovered, not propagated. Poison here records that some
/// earlier test panicked while it held the lock. The data behind the lock is a
/// unit value with no invariant to break, so the panic tells this crate
/// nothing, and a propagated poison turns one real failure into a cascade of
/// unrelated ones.
pub fn with_forced_ansi<T>(body: impl FnOnce() -> T) -> T {
    let _lock = OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner);
    force(body)
}

/// The forcing half of [`with_forced_ansi`], without the lock.
///
/// This split exists for the tests of this crate. `OVERRIDE` is a plain
/// `Mutex`, which is not reentrant, so a test that holds the lock to read the
/// override before and after the body cannot also call [`with_forced_ansi`] —
/// that call would wait for a lock the same thread already holds, and the test
/// would deadlock. Such a test calls `force` instead and holds the lock itself.
/// No other caller has a reason to use this function.
fn force<T>(body: impl FnOnce() -> T) -> T {
    #[allow(
        clippy::disallowed_methods,
        reason = "this is the helper the ban points every other caller at; the one call that sets the override lives here"
    )]
    colored::control::set_override(true);
    // Build the guard before the body runs. A call to
    // `colored::control::unset_override()` after `body()` runs only when `body`
    // returns; a panic in `body` unwinds past it and leaves the override on for
    // every later test. A guard restores the override on both paths, because
    // unwinding drops it.
    let _restore = Restore;
    body()
}

/// Puts the global override of the `colored` crate back when it goes out of
/// scope.
///
/// A test body panics as its normal way to fail, so the restore must survive a
/// panic. Only a `Drop` implementation does that.
struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
        #[allow(
            clippy::disallowed_methods,
            reason = "this is the helper the ban points every other caller at; the one call that puts the override back lives here"
        )]
        colored::control::unset_override();
    }
}

/// The start of a 24-bit foreground SGR sequence. The red, green, and blue
/// values follow it.
pub const TRUECOLOR_FG: &str = "\x1b[38;2;";

/// The largest red channel of any 24-bit foreground sequence in `text`.
///
/// A fade scales all three channels by one factor, so one channel reports the
/// brightness of the whole row.
///
/// The scan finds every [`TRUECOLOR_FG`] prefix in `text` and reads the digits
/// that follow it. A malformed sequence contributes nothing and breaks nothing:
/// a prefix with no digits after it, and a red value above 255, both fail to
/// parse and the scan drops them. The cursor moves past the prefix on every
/// turn, even when the digits are absent, so the scan always ends.
///
/// # Panics
///
/// Panics when `text` carries no 24-bit foreground sequence. A caller paints a
/// truecolor row on purpose, so an absent sequence is a failure of the test
/// rather than an empty result.
#[must_use]
pub fn max_red_channel(text: &str) -> u8 {
    let bytes = text.as_bytes();
    let needle = TRUECOLOR_FG.as_bytes();
    let mut best: Option<u8> = None;
    let mut at = 0;
    while let Some(found) = bytes[at..].windows(needle.len()).position(|w| w == needle) {
        let start = at + found + needle.len();
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        let digits = std::str::from_utf8(&bytes[start..end]).expect("ASCII digits are valid UTF-8");
        if let Ok(red) = digits.parse::<u8>() {
            best = Some(best.map_or(red, |seen: u8| seen.max(red)));
        }
        at = end;
    }
    best.expect("a truecolor row must carry a 24-bit foreground sequence")
}

/// `text` with every ANSI escape sequence removed, leaving the visible glyphs.
///
/// This is what a test compares against expected output. Whether a frame
/// carries color at all is decided by the process-global state of the `colored`
/// crate and by whether the run writes to a terminal, so a test that compares
/// raw bytes asserts on something the test does not control.
///
/// The scan walks characters, not bytes, so a multi-byte glyph next to an
/// escape survives whole.
///
/// Two shapes of escape are recognized. A CSI sequence starts with `ESC [`, and
/// runs through its parameter and intermediate bytes to a final byte in the
/// range `0x40..=0x7E`. Every other `ESC x` pair is an Fe-style escape, whose
/// second character ends it. An escape that runs off the end of `text` takes
/// the rest of `text` with it, which is what a truncated frame deserves.
#[must_use]
pub fn strip_ansi(s: &str) -> String {
    #[derive(PartialEq)]
    enum State {
        Normal,
        AfterEsc,
        InCsi,
    }
    let mut out = String::with_capacity(s.len());
    let mut state = State::Normal;
    for c in s.chars() {
        match state {
            State::Normal => {
                if c == '\x1b' {
                    state = State::AfterEsc;
                } else {
                    out.push(c);
                }
            }
            State::AfterEsc => {
                if c == '[' {
                    // CSI introducer — consume parameters until the final byte.
                    state = State::InCsi;
                } else {
                    // Fe-style single-byte escape (e.g. ESC M, ESC =).
                    // The byte itself is the final byte; swallow it and resume.
                    state = State::Normal;
                }
            }
            State::InCsi => {
                // Parameter bytes: 0x30–0x3F. Intermediate bytes: 0x20–0x2F.
                // Final byte: 0x40–0x7E — terminates the sequence.
                if (0x40..=0x7E).contains(&(c as u32)) {
                    state = State::Normal;
                }
                // In all cases, keep consuming (don't push to output).
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{force, max_red_channel, strip_ansi, with_forced_ansi, OVERRIDE, TRUECOLOR_FG};

    use std::sync::PoisonError;

    use colored::control::SHOULD_COLORIZE;
    use colored::Colorize;

    /// The first byte of every ANSI escape sequence the `colored` crate writes.
    const ESCAPE: &str = "\u{1b}[";

    #[test]
    fn forcing_makes_colored_emit_escapes() {
        let painted = with_forced_ansi(|| "x".red().to_string());
        assert!(
            painted.contains(ESCAPE),
            "expected ANSI escapes in {painted:?}"
        );
    }

    #[test]
    fn the_override_goes_back_when_the_body_ends() {
        // Hold the lock for all three reads. No other test can change the
        // override between them, so the before and after values are comparable.
        // The test holds the lock itself and calls `force`, because the lock is
        // not reentrant.
        let _lock = OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner);

        let ambient = SHOULD_COLORIZE.should_colorize();
        let inside = force(|| SHOULD_COLORIZE.should_colorize());

        assert!(inside, "the body must run with the override on");
        assert_eq!(
            SHOULD_COLORIZE.should_colorize(),
            ambient,
            "the override must go back to what it was before the body"
        );
    }

    #[test]
    fn the_override_goes_back_when_the_body_panics() {
        // A test body panics as its normal way to fail. The override must go
        // back on that path too, or one failed test paints every later one.
        let _lock = OVERRIDE.lock().unwrap_or_else(PoisonError::into_inner);

        let ambient = SHOULD_COLORIZE.should_colorize();
        let panicked = std::panic::catch_unwind(|| force(|| panic!("the body fails")));

        assert!(panicked.is_err(), "the body must have panicked");
        assert_eq!(
            SHOULD_COLORIZE.should_colorize(),
            ambient,
            "the override must go back after a panicking body"
        );
    }

    #[test]
    fn stripping_removes_the_escapes_colored_writes() {
        // The round trip is the point: paint text, strip it, and the glyphs
        // that come back are the glyphs that went in. This is the pair of
        // calls every render test in this workspace makes.
        let painted = with_forced_ansi(|| format!("{} [{}]", "/repo".green().bold(), "main".red()));
        assert!(painted.contains(ESCAPE), "the fixture must carry escapes");
        assert_eq!(strip_ansi(&painted), "/repo [main]");
    }

    #[test]
    fn stripping_leaves_plain_text_alone() {
        assert_eq!(strip_ansi("> /repo [main]\n"), "> /repo [main]\n");
    }

    #[test]
    fn stripping_removes_a_csi_sequence_with_several_parameters() {
        assert_eq!(strip_ansi("\x1b[1;32mgreen\x1b[0m"), "green");
    }

    #[test]
    fn stripping_removes_an_fe_escape_whose_second_character_ends_it() {
        // ESC M and ESC = carry no parameters, so the character after ESC is
        // the whole sequence.
        assert_eq!(strip_ansi("a\x1bMb\x1b=c"), "abc");
    }

    #[test]
    fn stripping_swallows_an_escape_that_runs_off_the_end() {
        // A frame cut mid-sequence has no visible glyphs left to report.
        assert_eq!(strip_ansi("done\x1b[38;2;1"), "done");
        assert_eq!(strip_ansi("done\x1b"), "done");
    }

    #[test]
    fn stripping_keeps_multi_byte_glyphs_whole() {
        // The scan walks characters, so a 3-byte or 4-byte glyph beside an
        // escape survives. Byte-level scanning is what breaks these.
        let painted = with_forced_ansi(|| format!("{} {} {}", "日本語".green(), "🎉".red(), "café".blue()));
        assert_eq!(strip_ansi(&painted), "日本語 🎉 café");
    }

    #[test]
    fn the_largest_red_channel_wins() {
        let text = format!("{TRUECOLOR_FG}10;0;0mdim{TRUECOLOR_FG}200;0;0mbright");
        assert_eq!(max_red_channel(&text), 200);
    }

    #[test]
    fn a_malformed_truecolor_sequence_contributes_nothing() {
        // 300 is above 255 and fails to parse, and the prefix with no digits
        // after it parses to nothing. The well-formed sequence still reports.
        let text = format!("{TRUECOLOR_FG}300;0;0mx{TRUECOLOR_FG}{TRUECOLOR_FG}7;0;0my");
        assert_eq!(max_red_channel(&text), 7);
    }
}
