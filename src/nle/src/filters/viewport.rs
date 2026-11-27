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

use termstate::{Cell, TerminalState, TerminalTheme};

use self::writer::{Glyph, Look, Writer};
use crate::ansi::{Scanner, Sequence, Step};
use crate::pipeline::{FilterRegistry, Screen, ScreenSize, ScreenView};
use crate::preview::{MAX_COLUMNS, MAX_ROWS};
use crate::types::{Event, EventType, FilterKind, FilterOptions};

/// The character BEL, which passes through.
const BEL: char = '\x07';

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
///
/// It checks the options and the screen, as the module doc says. When every
/// row and every column is visible, it answers the events as they are. Else it
/// runs `Replay` over the events. A range is not supported yet.
fn filter(
    events: &[Event],
    view: ScreenView<'_>,
    options: &FilterOptions,
) -> Result<Screen, String> {
    let value = serde_json::to_value(options).map_err(|error| error.to_string())?;
    let options: Options = serde_json::from_value(value).map_err(|error| error.to_string())?;
    let ScreenSize { width, height } = view.size;
    check_screen(view.size)?;
    let (rows_axis, columns_axis) = (Axis::ROWS, Axis::COLUMNS);
    let rows_crop = rows_axis.check_crop(options.rows, height)?;
    let columns_crop = columns_axis.check_crop(options.columns, width)?;
    rows_axis.check_hidden(&options.hide_rows, height)?;
    columns_axis.check_hidden(&options.hide_columns, width)?;
    let rows = rows_axis.visible(rows_crop, &options.hide_rows)?;
    let columns = columns_axis.visible(columns_crop, &options.hide_columns)?;
    if rows.len() == usize::from(height) && columns.len() == usize::from(width) {
        return Ok(Screen {
            events: events.to_vec(),
            size: view.size,
        });
    }
    if !view.before.is_empty() || !view.region.open {
        return Err("a range is not supported yet".to_string());
    }
    let size = ScreenSize {
        width: u16::try_from(columns.len()).map_err(|error| error.to_string())?,
        height: u16::try_from(rows.len()).map_err(|error| error.to_string())?,
    };
    let events = Replay::new(view.size, rows, columns).run(events)?;
    Ok(Screen { events, size })
}

/// Check that the screen has a cell and fits the limits of the preview.
fn check_screen(size: ScreenSize) -> Result<(), String> {
    let ScreenSize { width, height } = size;
    if width == 0 || height == 0 {
        return Err(format!("the screen of {width}x{height} has no cells"));
    }
    if usize::from(width) > MAX_COLUMNS || usize::from(height) > MAX_ROWS {
        return Err(format!(
            "the screen of {width}x{height} is larger than {MAX_COLUMNS}x{MAX_ROWS}, the limit of the preview"
        ));
    }
    Ok(())
}

/// The names of one axis of the screen, for the messages of the checks.
struct Axis {
    /// The key of the crop, and the plural of the unit: `rows`.
    plural: &'static str,
    /// The unit: `row`.
    unit: &'static str,
    /// The key of the hidden indexes: `hide_rows`.
    hide_key: &'static str,
}

impl Axis {
    const ROWS: Axis = Axis {
        plural: "rows",
        unit: "row",
        hide_key: "hide_rows",
    };
    const COLUMNS: Axis = Axis {
        plural: "columns",
        unit: "column",
        hide_key: "hide_columns",
    };

    /// The crop to use: `crop`, or all of the `len` indexes when there is no
    /// crop.
    ///
    /// # Errors
    ///
    /// The crop does not start before its end, or it ends after the screen.
    fn check_crop(&self, crop: Option<Crop>, len: u16) -> Result<Crop, String> {
        let plural = self.plural;
        match crop {
            Some(Crop { start, end }) if start >= end => Err(format!(
                "{plural}: the start {start} is not before the end {end}"
            )),
            Some(Crop { end, .. }) if end > len => Err(format!(
                "{plural}: the end {end} is after the last {} of the screen ({len} {plural})",
                self.unit
            )),
            Some(crop) => Ok(crop),
            None => Ok(Crop { start: 0, end: len }),
        }
    }

    /// Check that each hidden index is on an axis of `len` cells.
    ///
    /// # Errors
    ///
    /// A hidden index is not below `len`.
    fn check_hidden(&self, hidden: &[u16], len: u16) -> Result<(), String> {
        match hidden.iter().find(|index| **index >= len) {
            Some(index) => Err(format!(
                "{}: the {} {index} is not on the screen ({len} {})",
                self.hide_key, self.unit, self.plural
            )),
            None => Ok(()),
        }
    }

    /// The indexes of `crop` that `hidden` does not name, in order.
    ///
    /// # Errors
    ///
    /// No index is visible.
    fn visible(&self, crop: Crop, hidden: &[u16]) -> Result<Vec<usize>, String> {
        let visible: Vec<usize> = (crop.start..crop.end)
            .filter(|index| !hidden.contains(index))
            .map(usize::from)
            .collect();
        if visible.is_empty() {
            return Err(format!("no {} is visible", self.unit));
        }
        Ok(visible)
    }
}

/// What the emulator at the full size shows at one moment: the visible cells
/// in row order, and the cursor, mapped into the visible cells.
struct Replay {
    terminal: TerminalState,
    rows: Vec<usize>,
    columns: Vec<usize>,
    writer: Writer,
}

impl Replay {
    /// A replay of a screen of `size`, with the visible `rows` and `columns`.
    /// The output is a blank terminal of the size of the visible cells.
    fn new(size: ScreenSize, rows: Vec<usize>, columns: Vec<usize>) -> Self {
        Self {
            terminal: TerminalState::new(
                usize::from(size.width),
                usize::from(size.height),
                TerminalTheme::auto(),
            ),
            writer: Writer::blank(columns.len(), rows.len()),
            rows,
            columns,
        }
    }

    /// The paint of the screen that the emulator shows now.
    fn paint(&mut self) -> String {
        let grid = self.terminal.get_grid();
        let target: Vec<Glyph> = self
            .rows
            .iter()
            .flat_map(|row| self.columns.iter().map(move |column| (*row, *column)))
            .map(|(row, column)| {
                grid.get(row)
                    .and_then(|cells| cells.get(column))
                    .map_or(Glyph::BLANK, glyph_of)
            })
            .collect();
        let (cursor_x, cursor_y) = self.terminal.get_cursor_position();
        let cursor = if self.terminal.is_cursor_visible() {
            self.columns
                .iter()
                .position(|column| *column == cursor_x)
                .zip(self.rows.iter().position(|row| *row == cursor_y))
        } else {
            None
        };
        self.writer.paint(&target, cursor)
    }

    /// The events of the region `All` after the filter. The first event is the
    /// paint of the start, at time 0.
    ///
    /// # Errors
    ///
    /// The emulator refuses the data of an event.
    fn run(mut self, events: &[Event]) -> Result<Vec<Event>, String> {
        let mut answer = Vec::new();
        let start = self.paint();
        if !start.is_empty() {
            answer.push(Event {
                time: 0.0,
                event_type: EventType::Output,
                data: start,
            });
        }
        let mut scanner = Scanner::new();
        for event in events {
            if event.event_type != EventType::Output {
                answer.push(event.clone());
                continue;
            }
            let mut data: String = event
                .data
                .chars()
                .map(|c| pass_through(&scanner.feed(c), c))
                .collect();
            self.terminal
                .process_output(&event.data)
                .map_err(|error| error.to_string())?;
            data.push_str(&self.paint());
            if !data.is_empty() {
                answer.push(Event {
                    data,
                    ..event.clone()
                });
            }
        }
        Ok(answer)
    }
}

/// The glyph that an emulator cell shows.
fn glyph_of(cell: &Cell) -> Glyph {
    Glyph {
        ch: cell.ch,
        look: Look {
            fg: cell.fg_source,
            bg: cell.bg_source,
            bold: cell.bold,
            italic: cell.italic,
            underline: cell.underline,
        },
    }
}

/// The part of the stream that `step`, the step of `c`, passes through: an
/// OSC, a DECSCUSR, mode 12, and BEL. Each other step gives nothing.
fn pass_through(step: &Step, c: char) -> String {
    match step {
        Step::Text if c == BEL => BEL.to_string(),
        Step::Done(Sequence::Osc { text, terminator }) => format!("\x1b]{text}{terminator}"),
        Step::Done(Sequence::Csi {
            params,
            intermediates,
            final_char: 'q',
            ..
        }) if intermediates == " " => format!("\x1b[{params} q"),
        Step::Done(Sequence::Csi {
            params,
            intermediates,
            final_char: final_char @ ('h' | 'l'),
            ..
        }) if intermediates.is_empty() => params
            .strip_prefix('?')
            .map(|modes| {
                modes
                    .split(';')
                    .filter(|mode| *mode == "12")
                    .map(|_| format!("\x1b[?12{final_char}"))
                    .collect()
            })
            .unwrap_or_default(),
        Step::EscapeBreak(inner) => pass_through(inner, c),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests;
