//! The backspace removal filter. It removes a typed key and the key that
//! erases it, so the viewer sees the text that the typist meant.
//!
//! # The idea
//!
//! The filter removes a typed key and its erase only when the screen after the
//! erase is the screen before the typed key. Then the screen at each later
//! event is the same with or without them. The filter keeps a model of the
//! screen, a `TerminalState`, to make that comparison. It acts only on erase
//! keys in input events. Thus output that writes `\b` with no erase key before
//! it stays, and so does the output of a program whose screen does not come
//! back after an erase key.
//!
//! # The units and the keys
//!
//! The filter walks the events in list order. It feeds each output event before
//! the first input event to the screen and keeps it. Each input event starts a
//! unit: the input event and the output events up to the next input event.
//! The data of the input event gives the key of the unit:
//!
//! | Key | Data |
//! |---|---|
//! | typed | one printable character |
//! | erase one | DEL (`\x7f`) or BS (`\x08`) |
//! | erase word | Ctrl+W (`\x17`) |
//! | erase line | Ctrl+U (`\x15`) |
//! | other | all other data: Enter, Tab, arrows, a paste, two keys in one event |
//!
//! # The rules
//!
//! The filter keeps a stack of the typed keys of the current line. Each entry
//! holds the screen before its key. The newest entry is at the back.
//!
//! - A typed key goes on the stack.
//! - An erase-one key whose screen equals the screen before the newest typed
//!   key removes the events from that typed key to the end of the erase. The
//!   entry leaves the stack.
//! - An erase-word or erase-line key looks for the entries whose screen equals
//!   the screen now. If one entry matches, the events from that key to the end
//!   of the erase go, and the stack keeps the keys before it. If many entries
//!   match, as with a prompt that echoes nothing, the filter cannot tell how many
//!   keys the erase removed. The unit stays and the stack ends.
//! - An erase key that no typed key explains goes with its output when its
//!   unit has output and it changed nothing on the screen, for example a
//!   Backspace at the start of a line that rings the bell. Else the unit stays
//!   and the stack ends. A unit with no output stays, because type-ahead can
//!   put the echo of its key after the next key.
//! - An other key stays and ends the stack. The filter follows one line.
//! - The last unit of a range stays and ends the stack when the first event
//!   after the range is an output event. The end of the range cuts the output
//!   of that unit, so the filter cannot see all of it.
//!
//! # The times
//!
//! The filter marks the removed runs. The pipeline removes their time by the
//! steps of `CutFilterFn`, so the recording looks as if the typist made no
//! error. The rule that the pipeline obeys: the cut of a run is the time from
//! its first event to the next kept event, or to the end of the region.
//! Each kept event moves earlier by the cuts before it. The length of the
//! region becomes the old length less all cuts, but never below the last
//! kept event.
//!
//! The screen starts empty. The filter feeds the output events before the
//! region into it, so a filter on a range sees the screen that the viewer
//! sees. The screen uses its own theme, `probe_theme`, in which no two colors
//! are equal.
//!
//! # The checks
//!
//! The filter takes no option, and a key in the options fails the render.
//! The terminal must have at least one column and one row, and at most
//! `MAX_CELLS` cells. Else the render fails and the message gives the size.
//!
//! # The limits
//!
//! - The comparison is as exact as the emulator of `termstate`. The emulator
//!   does not model each sequence, for example the insert mode (`CSI 4 h`) or
//!   a second screen buffer for mode 1049. A typo near such output thus
//!   usually stays.
//! - A prompt that echoes nothing keeps a Ctrl+W or a Ctrl+U, because many
//!   entries match.
//! - Type-ahead during a slow echo can put the echo of a key after the next
//!   key. The filter then removes a key and keeps its echo. The screen at the
//!   end is correct, but the viewer sees one moment that the typist did not
//!   type. Two cases are known:
//!   - A typed key and a Backspace with no output in their units go together,
//!     as at a prompt that echoes nothing. Under double type-ahead, their
//!     echoes stay, and the typo shows with no key.
//!   - Under single type-ahead, the unit of a Backspace can hold an unrelated
//!     output that changes nothing on the screen, for example a bell. The
//!     Backspace then goes with that output, and its late echo stays. Thus an
//!     earlier typed character goes with no Backspace.
//! - The stack holds `SNAPSHOT_BUDGET / cells` keys, at least 1 and at most
//!   `MAX_DEPTH`. A line of more typed keys than that depth keeps its oldest
//!   keys.

use std::collections::VecDeque;
use std::ops::Range;

use serde::Deserialize;
use termstate::{TerminalState, TerminalTheme};

use crate::pipeline::{FilterRegistry, SpanContext};
use crate::types::{Event, EventType, FilterKind, FilterOptions};

/// The kind of the backspace removal filter.
pub const KIND: &str = "backspace-removal";

/// The data of the Backspace key of most terminals (DEL).
const DEL: &str = "\x7f";

/// The data of the Ctrl+H key (BS). Many terminals send it for Backspace.
const BS: &str = "\x08";

/// The data of the Ctrl+W key. It erases the word before the cursor.
const CTRL_W: &str = "\x17";

/// The data of the Ctrl+U key. It erases the line before the cursor.
const CTRL_U: &str = "\x15";

/// The most cells that the screen of the filter may have. A larger terminal
/// fails the render, because the filter holds many copies of the screen.
const MAX_CELLS: usize = 1 << 20;

/// The most typed keys that the stack holds, on a small terminal.
const MAX_DEPTH: usize = 256;

/// The most cells that the screens of the stack hold together. A large
/// terminal gets a smaller depth.
const SNAPSHOT_BUDGET: usize = 1 << 22;

/// What the data of an input event is, for this filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    /// One printable character.
    Typed,
    /// The Backspace key, as DEL or as BS.
    EraseChar,
    /// The Ctrl+W key.
    EraseWord,
    /// The Ctrl+U key.
    EraseLine,
    /// Any other input. It ends the line that the filter follows.
    Other,
}

/// The key that `data`, the data of an input event, is.
fn key_of(data: &str) -> Key {
    if data == DEL || data == BS {
        return Key::EraseChar;
    }
    if data == CTRL_W {
        return Key::EraseWord;
    }
    if data == CTRL_U {
        return Key::EraseLine;
    }
    let mut chars = data.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_control() => Key::Typed,
        _ => Key::Other,
    }
}

/// A typed key of the current line.
struct TypedKey {
    /// The index of its input event in the list of events.
    first: usize,
    /// The screen before the key.
    before: TerminalState,
}

/// The options of the filter. It has none, so a key is an error.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoOptions {}

/// Check that `options` holds no key. The message of the error names the key.
fn reject_options(options: &FilterOptions) -> Result<(), String> {
    let value = serde_json::to_value(options).map_err(|error| error.to_string())?;
    serde_json::from_value::<NoOptions>(value)
        .map(|NoOptions {}| ())
        .map_err(|error| error.to_string())
}

/// Check that the terminal of `width` by `height` cells can hold a screen, and
/// give its number of cells. It has at least one cell, and not more than
/// `MAX_CELLS`.
fn check_size(width: usize, height: usize) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err(format!(
            "the terminal is {width} by {height} cells, and it needs at least 1 by 1"
        ));
    }
    let cells = width.saturating_mul(height);
    if cells > MAX_CELLS {
        return Err(format!(
            "the terminal is {width} by {height} cells, and the limit is {MAX_CELLS} cells"
        ));
    }
    Ok(cells)
}

/// The depth of the stack for a terminal of `cells` cells. The screens of the
/// stack together hold at most `SNAPSHOT_BUDGET` cells, but the stack holds at
/// least one key and at most `MAX_DEPTH` keys.
fn depth_for(cells: usize) -> usize {
    (SNAPSHOT_BUDGET / cells.max(1)).clamp(1, MAX_DEPTH)
}

/// The kind of the backspace removal filter, as a `FilterKind`.
#[must_use]
pub fn kind() -> FilterKind {
    FilterKind::new(KIND)
}

/// Add the backspace removal filter to `registry`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_cut(kind(), remove_backspacing);
}

/// Remove each typed key and the key that erases it, when the screen after
/// the erase is the screen before the typed key. The erase keys are Backspace
/// (DEL or BS), Ctrl+W, and Ctrl+U. The filter changes only input events and
/// the output events that follow them. It answers one mark for each event. The
/// pipeline removes the time of each removed run, so the viewer sees no pause
/// for the typo.
///
/// The filter takes no option. It replays the output events before the region
/// into the screen first. When the first event after the region is an output
/// event, the end of the region cuts the last unit, and that unit stays.
///
/// # Errors
/// Returns an error when `options` holds a key, when the terminal has no
/// column or no row or more than `MAX_CELLS` cells, or when the screen fails
/// to read an output event.
fn remove_backspacing(
    events: &[Event],
    context: SpanContext<'_>,
    options: &FilterOptions,
) -> Result<Vec<bool>, String> {
    reject_options(options)?;
    let (width, height) = (
        usize::from(context.size.width),
        usize::from(context.size.height),
    );
    let depth = depth_for(check_size(width, height)?);
    let mut screen = TerminalState::new(width, height, probe_theme());
    for event in context
        .before
        .iter()
        .filter(|event| event.event_type == EventType::Output)
    {
        feed(&mut screen, event)?;
    }
    let last_unit_is_cut = context
        .after
        .first()
        .is_some_and(|event| event.event_type == EventType::Output);
    mark_removed(events, &mut screen, depth, last_unit_is_cut)
}

/// Walk `events` and mark the events that the filter removes. `screen` is the
/// screen at the start of the events. The stack of typed keys holds at most
/// `depth` keys. When `last_unit_is_cut` is true, the end of the region cuts
/// the output of the last unit, so the last unit stays and ends the stack.
fn mark_removed(
    events: &[Event],
    screen: &mut TerminalState,
    depth: usize,
    last_unit_is_cut: bool,
) -> Result<Vec<bool>, String> {
    let mut walk = Walk::new(events.len(), depth);
    let mut index = 0;
    while let Some(event) = events.get(index) {
        if event.event_type == EventType::Output {
            feed(screen, event)?;
            index += 1;
            continue;
        }
        let unit_end = events
            .iter()
            .enumerate()
            .skip(index + 1)
            .find(|(_, later)| later.event_type == EventType::Input)
            .map_or(events.len(), |(position, _)| position);
        let cut = last_unit_is_cut && unit_end == events.len();
        let key = if cut { Key::Other } else { key_of(&event.data) };
        let before = (key != Key::Other).then(|| screen.clone());
        for output in events.get(index + 1..unit_end).unwrap_or_default() {
            feed(screen, output)?;
        }
        let unit = index..unit_end;
        match (key, before) {
            (Key::Typed, Some(before)) => walk.push(index, before),
            (Key::EraseChar, Some(before)) => walk.erase_char(unit, screen, &before),
            (Key::EraseWord | Key::EraseLine, Some(before)) => {
                walk.erase_many(unit, screen, &before);
            }
            _ => walk.stack.clear(),
        }
        index = unit_end;
    }
    Ok(walk.removed)
}

/// The state of the walk: the events marked as removed, and the typed keys of
/// the current line. Each rule of a key is a method.
struct Walk {
    /// For each event, if the filter removes it.
    removed: Vec<bool>,
    /// The typed keys of the current line. The newest is at the back.
    stack: VecDeque<TypedKey>,
    /// The most keys that the stack holds.
    depth: usize,
}

impl Walk {
    /// A walk over `count` events, with no event removed and no typed key. The
    /// stack holds at most `depth` keys.
    fn new(count: usize, depth: usize) -> Self {
        Self {
            removed: vec![false; count],
            stack: VecDeque::new(),
            depth,
        }
    }

    /// Put the typed key at event `first` on the stack. `before` is the screen
    /// before the key. If the stack is full, the oldest key leaves it.
    fn push(&mut self, first: usize, before: TerminalState) {
        if self.stack.len() >= self.depth {
            self.stack.pop_front();
        }
        self.stack.push_back(TypedKey { first, before });
    }

    /// Mark the events from `from` up to `to` as removed.
    fn mark(&mut self, from: usize, to: usize) {
        if let Some(range) = self.removed.get_mut(from..to) {
            range.fill(true);
        }
    }

    /// The rule of the Backspace key. `screen` is the screen after the unit,
    /// and `before` is the screen before it.
    ///
    /// If the screen is the screen before the newest typed key, the events from
    /// that key to the end of the unit go, and the key leaves the stack. Else
    /// the rule of a key that no typed key explains applies.
    fn erase_char(&mut self, unit: Range<usize>, screen: &TerminalState, before: &TerminalState) {
        match self.stack.back() {
            Some(newest) if *screen == newest.before => {
                let first = newest.first;
                self.mark(first, unit.end);
                self.stack.pop_back();
            }
            _ => self.erase_without_typed_key(unit, screen == before),
        }
    }

    /// The rule of an erase key that no typed key explains. `unit` is the input
    /// event of the key and the output events after it.
    ///
    /// If the unit has output and the key changed nothing on the screen, the
    /// unit goes and the stack stays. Else the unit stays and the stack ends:
    /// the key changed text that no typed key wrote, or its echo can come
    /// after the next key.
    fn erase_without_typed_key(&mut self, unit: Range<usize>, changed_nothing: bool) {
        let has_output = unit.len() > 1;
        if changed_nothing && has_output {
            self.mark(unit.start, unit.end);
        } else {
            self.stack.clear();
        }
    }

    /// The rule of an erase key that can remove many typed keys. It looks for
    /// the typed keys whose screen before the key is the screen now.
    ///
    /// - One match: the events from that typed key to the end of the unit go,
    ///   and the stack keeps the keys before it.
    /// - More than one match: the filter cannot tell how many keys the erase
    ///   removed. The unit stays and the stack ends.
    /// - No match: the rule of a key that no typed key explains.
    fn erase_many(&mut self, unit: Range<usize>, screen: &TerminalState, before: &TerminalState) {
        let mut matches = self
            .stack
            .iter()
            .enumerate()
            .filter(|(_, typed)| typed.before == *screen)
            .map(|(position, typed)| (position, typed.first));
        let (first_match, second_match) = (matches.next(), matches.next());
        match (first_match, second_match) {
            (Some((position, first)), None) => {
                self.mark(first, unit.end);
                self.stack.truncate(position);
            }
            (Some(_), Some(_)) => self.stack.clear(),
            (None, _) => self.erase_without_typed_key(unit, screen == before),
        }
    }
}

/// The theme of the screen that the filter compares.
///
/// The 18 colors differ from each other and from each color of the 256-color
/// palette. The color cube of that palette uses only the channel values 0, 95,
/// 135, 175, 215 and 255, and its gray ramp has equal channels. No color
/// `(n, 1, 2)` is in it. Thus two cells are equal only when a terminal in
/// each theme draws them the same.
fn probe_theme() -> TerminalTheme {
    let color = |n: u8| (n, 1, 2);
    TerminalTheme {
        background: color(1),
        foreground: color(2),
        black: color(3),
        red: color(4),
        green: color(5),
        yellow: color(6),
        blue: color(7),
        magenta: color(8),
        cyan: color(9),
        white: color(10),
        bright_black: color(11),
        bright_red: color(12),
        bright_green: color(13),
        bright_yellow: color(14),
        bright_blue: color(15),
        bright_magenta: color(16),
        bright_cyan: color(17),
        bright_white: color(18),
    }
}

/// Feed the data of the output event `event` to `screen`.
fn feed(screen: &mut TerminalState, event: &Event) -> Result<(), String> {
    screen
        .process_output(&event.data)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
