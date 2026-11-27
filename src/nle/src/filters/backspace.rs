//! The backspace removal filter: it removes typing errors and the backspaces
//! that erase them.

use std::collections::VecDeque;
use std::ops::Range;

use serde::Deserialize;
use termstate::{TerminalState, TerminalTheme};

use crate::pipeline::{FilterRegistry, Span, SpanContext};
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

/// Check that the terminal of `width` by `height` cells can hold a screen. It
/// has at least one cell, and not more than `MAX_CELLS`.
fn check_size(width: usize, height: usize) -> Result<(), String> {
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
    Ok(())
}

/// The kind of the backspace removal filter, as a `FilterKind`.
#[must_use]
pub fn kind() -> FilterKind {
    FilterKind::new(KIND)
}

/// Add the backspace removal filter to `registry`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_span(kind(), remove_backspacing);
}

/// Remove each typed key and the backspace that erases it, when the screen
/// after the backspace is the screen before the typed key. The filter changes
/// only input events and the output events that follow them. It removes the
/// time of each removed run, so the viewer sees no pause for the typo.
///
/// The filter takes no option. It replays the output events before
/// the region into the screen first.
fn remove_backspacing(
    span: Span,
    context: SpanContext<'_>,
    options: &FilterOptions,
) -> Result<Span, String> {
    reject_options(options)?;
    let (width, height) = (
        usize::from(context.size.width),
        usize::from(context.size.height),
    );
    check_size(width, height)?;
    let mut screen = TerminalState::new(width, height, probe_theme());
    for event in context
        .before
        .iter()
        .filter(|event| event.event_type == EventType::Output)
    {
        feed(&mut screen, event)?;
    }
    let removed = mark_removed(&span.events, &mut screen)?;
    Ok(cut_removed(span, removed))
}

/// Walk `events` and mark the events that the filter removes. `screen` is the
/// screen at the start of the events.
fn mark_removed(events: &[Event], screen: &mut TerminalState) -> Result<Vec<bool>, String> {
    let mut walk = Walk::new(events.len());
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
        let key = key_of(&event.data);
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
}

impl Walk {
    /// A walk over `count` events, with no event removed and no typed key.
    fn new(count: usize) -> Self {
        Self {
            removed: vec![false; count],
            stack: VecDeque::new(),
        }
    }

    /// Put the typed key at event `first` on the stack. `before` is the screen
    /// before the key.
    fn push(&mut self, first: usize, before: TerminalState) {
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

    /// The rule of an erase key that no typed key explains. If the key changed
    /// nothing on the screen, `unit` goes and the stack stays. Else the key
    /// changed text that no typed key wrote, so the unit stays and the stack
    /// ends.
    fn erase_without_typed_key(&mut self, unit: Range<usize>, changed_nothing: bool) {
        if changed_nothing {
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
/// each theme draws them the same. With `TerminalTheme::auto()`, black is the
/// background, so a space on `CSI 40 m` would equal a blank cell, and the
/// filter would remove a black cell that a viewer sees in another theme.
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

/// Drop the events that `removed` marks, and take their time from the rest.
///
/// For each run of removed events, the cut is the time from the first event of
/// the run to the next kept event, or to the end of the span. Each kept event
/// moves earlier by the cuts before it. The length is the old length less all
/// cuts, but never below the last kept event.
fn cut_removed(span: Span, removed: Vec<bool>) -> Span {
    let mut kept = Vec::new();
    let mut cut = 0.0;
    let mut run_start: Option<f64> = None;
    for (event, gone) in span.events.into_iter().zip(removed) {
        if gone {
            run_start.get_or_insert(event.time);
        } else {
            if let Some(start) = run_start.take() {
                cut += (event.time - start).max(0.0);
            }
            kept.push(Event {
                time: (event.time - cut).max(0.0),
                ..event
            });
        }
    }
    if let Some(start) = run_start {
        cut += (span.length - start).max(0.0);
    }
    let last = kept.iter().map(|event| event.time).fold(0.0, f64::max);
    Span {
        events: kept,
        length: (span.length - cut).max(last).max(0.0),
    }
}

#[cfg(test)]
mod tests;
