//! The filter that crops the screen and hides rows and columns.
//!
//! The filter runs the `termstate` emulator at the size of the screen. After
//! each output event, it compares the cells that the output must show with the
//! cells that it already wrote, and it writes only the changed cells, as escape
//! sequences. Thus each player and each exporter shows the crop.
//!
//! The output shows the screen that `termstate` models. A sequence that
//! `termstate` does not model (for example CHA, VPA, the insert mode, the
//! origin mode, or a second screen buffer for mode 1049) has no effect in the
//! output.
//!
//! The options, with their keys in the project file:
//!
//! - `rows` and `columns`: the crop, as `{ "start": 1, "end": 24 }`. The
//!   count starts at 0. The start is in the crop, and the end is not. A
//!   missing key keeps all rows or all columns.
//! - `hide_rows` and `hide_columns`: rows and columns of the full screen that
//!   do not show, also when they are not next to each other. An index outside
//!   the crop has no effect.
//!
//! The visible rows are the rows of the crop that are not hidden. The visible
//! columns are the same for columns. An unknown key fails the render. A start
//! that is not before its end, an end after the edge of the screen, a hidden
//! index that is not on the screen, no visible row, and no visible column each
//! fail the render, with a message that names the option. A screen of width 0
//! or height 0, or larger than the limits of the preview, fails the render.
//! Thus a bad header cannot cause a panic or a very large allocation.
//!
//! When all rows and all columns are visible, the filter answers the events
//! with no change, at the same size.
//!
//! The region `All` gives an output of the count of visible columns by the
//! count of visible rows. The visible cells move together: a hidden row or
//! column goes out, and the cells after it move up or left. The region `Range`
//! is not supported yet.
//!
//! For each output event, the filter first writes the sequences that do not
//! change a cell, in their order: each OSC, each DECSCUSR, mode 12 (the blink
//! of the cursor), and BEL. Then it writes the paint of the screen after the
//! event. It removes each other byte of the event, and it removes an output
//! event whose new data is empty. An input event stays as it is.

mod writer;

use serde::Deserialize;

use crate::pipeline::{FilterRegistry, Screen, ScreenView};
use crate::types::{Event, FilterKind, FilterOptions};

/// The kind of the viewport filter.
pub const VIEWPORT: &str = "viewport";

/// Register the viewport filter under `VIEWPORT`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_screen(FilterKind::new(VIEWPORT), filter);
}

/// A crop on one axis: the start is in the crop, and the end is not.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct Crop {
    start: u16,
    end: u16,
}

/// The options in the project file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Options {
    rows: Option<Crop>,
    columns: Option<Crop>,
    #[serde(default)]
    hide_rows: Vec<u16>,
    #[serde(default)]
    hide_columns: Vec<u16>,
}

/// The viewport filter.
fn filter(
    events: &[Event],
    view: ScreenView<'_>,
    _options: &FilterOptions,
) -> Result<Screen, String> {
    Ok(Screen {
        events: events.to_vec(),
        size: view.size,
    })
}

#[cfg(test)]
mod tests;
