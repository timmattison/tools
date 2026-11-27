//! The keystroke HUD filter: it shows the keys that the presenter presses in a
//! box on the screen. Issue #161.
//!
//! The filter reads the input events of its region. It shows the keys in a box
//! with rounded corners. Fast typing goes into one box. A key that the user
//! holds shows once, with a count. The box fades in and out in steps of 24-bit
//! color.
//!
//! The filter draws with output events. Each one saves the cursor, draws the
//! box, and restores the cursor. The filter replays the events through the
//! emulator of `termstate`. Thus it knows the cells under the box. It puts
//! them back when the box goes away, and it keeps the box in place when the
//! output scrolls. Thus `beta play`, `beta export web`, and `beta export video`
//! show the HUD with no change.
//!
//! The options, with their defaults:
//! - `position`: `top-right`. The corner or the edge of the box.
//! - `duration`: 1.5. The seconds that a box stays after its last key.
//! - `fade`: 0.2. The seconds of each fade. 0 turns the fade off.
//! - `theme`: `auto`. The theme of the export. The fade starts from its
//!   background.
//! - `text`: `#ffffff`. The color of typed text.
//! - `accent`: `#ffc83c`. The color of the labels of special keys.
//! - `background`: `#303030`. The color of the inside of the box.
//! - `border`: `#808080`. The color of the border.
//!
//! A redaction of input events (issue #156) must run before this filter, in the
//! list of filters of the project. Then the HUD does not show the hidden keys.
//! A redaction after this filter does not change the boxes that the HUD drew.
//!
//! The limits:
//! - A modifier key alone (Ctrl, Alt, Shift) shows nothing. A terminal sends
//!   no byte for it. The kitty keyboard protocol sends a code, and the HUD
//!   skips it.
//! - The HUD never makes a region longer. A box that is still on the screen at
//!   the end of the region goes away there.
//! - `termstate` gives each character one cell. A wide character (for example
//!   `日`) in a label moves the right border by one cell in a terminal or in
//!   xterm.js.
//! - A cell color of exactly one of the 18 colors of the private replay theme
//!   comes back as its theme color.

mod keys;
mod options;
mod paint;
mod timeline;

pub use options::{
    HudOptions, Position, Rgb, ACCENT_OPTION, BACKGROUND_OPTION, BORDER_OPTION, DURATION_OPTION,
    FADE_OPTION, POSITION_OPTION, TEXT_OPTION, THEME_OPTION,
};

use crate::pipeline::{FilterRegistry, Screen, Span};
use crate::types::{AppliedFilter, Event, EventType, FilterKind, FilterOptions, FilterRegion};
use paint::{layout, Painter};
use timeline::{Press, Timeline, Timing};

/// The kind of the keystroke HUD filter.
pub const KIND: &str = "keystroke-hud";

/// The kind of the keystroke HUD filter, as a `FilterKind`.
#[must_use]
pub fn kind() -> FilterKind {
    FilterKind::new(KIND)
}

/// A keystroke HUD filter for `region` with `options`.
#[must_use]
pub fn applied(region: FilterRegion, options: &HudOptions) -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region,
        options: options.to_options(),
    }
}

/// The most cells of a screen that the HUD replays. A larger size fails the
/// render, so that a header with a bad size does not take all the memory.
pub const MAX_CELLS: usize = 1_000_000;

/// Add the keystroke HUD filter to `registry`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_screen(kind(), draw);
}

/// The screen filter of the HUD. It answers the events of `span`, with the
/// output events of the box added in time order.
///
/// # Errors
/// Returns a message when an option is not valid, or when the screen has more
/// than `MAX_CELLS` cells.
fn draw(screen: &Screen<'_>, span: Span, options: &FilterOptions) -> Result<Vec<Event>, String> {
    let options = HudOptions::from_options(options)?;
    let size = screen.size;
    let (columns, rows) = (size.columns, size.rows);
    if usize::from(columns) * usize::from(rows) > MAX_CELLS {
        return Err(format!(
            "the terminal is {columns} x {rows} cells, which is more than the {MAX_CELLS} cells that the keystroke HUD replays"
        ));
    }
    let Some(mut painter) = Painter::new(size) else {
        return Ok(span.events);
    };
    for event in screen.before {
        if event.event_type == EventType::Output {
            painter.feed(&event.data);
        }
    }

    let mut events = span.events;
    // The pipeline gives no order inside a region.
    events.sort_by(|a, b| a.time.total_cmp(&b.time));
    let presses = events
        .iter()
        .filter(|event| event.event_type == EventType::Input)
        .flat_map(|event| {
            keys::parse(&event.data).into_iter().map(|key| Press {
                time: event.time,
                key,
            })
        })
        .collect();
    let timeline = Timeline::new(
        presses,
        Timing {
            duration: options.duration,
            fade: options.fade,
            limit: span.length,
        },
    );
    let frames = timeline.frames();

    let mut result = Vec::with_capacity(events.len());
    let paint = |painter: &mut Painter, result: &mut Vec<Event>, time: f64| {
        let overlay = timeline
            .at(time)
            .and_then(|shown| layout(size, &options, &shown));
        if let Some(data) = painter.paint(overlay.as_ref()) {
            result.push(Event {
                time,
                event_type: EventType::Output,
                data,
            });
        }
    };
    let mut next_frame = 0;
    for event in events {
        // A frame at the time of an event comes after it.
        while let Some(&time) = frames.get(next_frame).filter(|time| **time < event.time) {
            paint(&mut painter, &mut result, time);
            next_frame += 1;
        }
        let time = event.time;
        let is_output = event.event_type == EventType::Output;
        if is_output {
            painter.feed(&event.data);
        }
        result.push(event);
        if is_output {
            paint(&mut painter, &mut result, time);
        }
    }
    for &time in frames.iter().skip(next_frame) {
        paint(&mut painter, &mut result, time);
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
