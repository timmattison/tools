//! The backspace removal filter: it removes typing errors and the backspaces
//! that erase them.

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

/// What the data of an input event is, for this filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    /// One printable character.
    Typed,
    /// The Backspace key, as DEL or as BS.
    EraseChar,
    /// The Ctrl+W key.
    EraseWord,
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
/// The filter does not parse options, and it does not replay the events
/// before the region.
fn remove_backspacing(
    span: Span,
    context: SpanContext<'_>,
    _options: &FilterOptions,
) -> Result<Span, String> {
    let mut screen = TerminalState::new(
        usize::from(context.size.width),
        usize::from(context.size.height),
        TerminalTheme::auto(),
    );
    let removed = mark_removed(&span.events, &mut screen)?;
    Ok(cut_removed(span, removed))
}

/// Walk `events` and mark the events that the filter removes. `screen` is the
/// screen at the start of the events.
fn mark_removed(events: &[Event], screen: &mut TerminalState) -> Result<Vec<bool>, String> {
    let mut removed = vec![false; events.len()];
    let mut stack: Vec<TypedKey> = Vec::new();
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
        match (key, before) {
            (Key::Typed, Some(before)) => stack.push(TypedKey {
                first: index,
                before,
            }),
            (Key::EraseChar, Some(before)) => match stack.last() {
                Some(newest) if *screen == newest.before => {
                    mark(&mut removed, newest.first, unit_end);
                    stack.pop();
                }
                _ => erase_without_typed_key(
                    &mut removed,
                    &mut stack,
                    index..unit_end,
                    screen == &before,
                ),
            },
            (Key::EraseWord, Some(before)) => {
                erase_many(&mut removed, &mut stack, index..unit_end, screen, &before);
            }
            _ => stack.clear(),
        }
        index = unit_end;
    }
    Ok(removed)
}

/// Mark the events from `from` up to `to` as removed.
fn mark(removed: &mut [bool], from: usize, to: usize) {
    if let Some(range) = removed.get_mut(from..to) {
        range.fill(true);
    }
}

/// The rule of an erase key that no typed key explains. If the key changed
/// nothing on the screen, `unit` goes and the stack stays. Else the key
/// changed text that no typed key wrote, so the unit stays and the stack ends.
fn erase_without_typed_key(
    removed: &mut [bool],
    stack: &mut Vec<TypedKey>,
    unit: std::ops::Range<usize>,
    changed_nothing: bool,
) {
    if changed_nothing {
        mark(removed, unit.start, unit.end);
    } else {
        stack.clear();
    }
}

/// The rule of an erase key that can remove many typed keys. It looks for the
/// typed keys whose screen before the key is the screen now.
///
/// - One match: the events from that typed key to the end of the unit go, and
///   the stack keeps the keys before it.
/// - More than one match: the filter cannot tell how many keys the erase
///   removed. The unit stays and the stack ends.
/// - No match: the rule of a key that no typed key explains.
fn erase_many(
    removed: &mut [bool],
    stack: &mut Vec<TypedKey>,
    unit: std::ops::Range<usize>,
    screen: &TerminalState,
    before: &TerminalState,
) {
    let mut matches = stack
        .iter()
        .enumerate()
        .filter(|(_, typed)| typed.before == *screen)
        .map(|(position, typed)| (position, typed.first));
    let (first_match, second_match) = (matches.next(), matches.next());
    match (first_match, second_match) {
        (Some((position, first)), None) => {
            mark(removed, first, unit.end);
            stack.truncate(position);
        }
        (Some(_), Some(_)) => stack.clear(),
        (None, _) => erase_without_typed_key(removed, stack, unit, screen == before),
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
