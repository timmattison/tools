//! The filter that crops the screen and hides rows and columns.
//!
//! The filter runs the `termstate` emulator at the size of the screen. After
//! each output event, it compares the cells that the output must show with the
//! cells that it already wrote, and it writes only the changed cells, as escape
//! sequences. Thus each player and each exporter shows the crop.
//!
//! The output shows the screen that `termstate` models. A sequence that
//! `termstate` does not model (for example the insert mode, the origin mode,
//! or a second screen buffer for mode 1049) has no effect in the output.
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
//! column goes out, and the cells after it move up or left.
//!
//! The region `Range` keeps the size of the screen, and each cell keeps its
//! place. During the range, each cell that is not visible is a blank: a space
//! with the default colors and no attribute. The cursor shows only when its
//! row and its column are visible. The filter feeds the output events before
//! the range to the emulator, so it knows the screen at the start. It does not
//! feed the input events. Then it works as follows:
//!
//! - At the start of the range (time 0), it writes the mask. When the emulator
//!   uses the line-drawing character set, it first writes `ESC ( B`, because
//!   the filter writes each line-drawing character as its Unicode
//!   box-drawing character. The first paint has a CUP, because the cursor of
//!   the output is not known. It also has a full SGR, because the pen of the
//!   output is not known.
//! - At the end of a range that is not open (time `view.region.length`), it
//!   writes one output event that gives the full screen back and sets the
//!   state of the terminal to the state of the emulator, in this order: the
//!   cells that differ, the scrolling region (DECSTBM), the pen (one SGR),
//!   the saved cursor position (a CUP, then `ESC 7`), the line-drawing
//!   character set (`ESC ( 0`, when the emulator uses it), the cursor (a CUP,
//!   then mode 25), and last the start of the escape sequence that is open at
//!   the end of the range. Thus an event after the range that finishes the
//!   sequence finishes it in the real terminal too. A range that is open has
//!   no restore.
//!
//! An escape sequence that the start of a range splits needs no such step.
//! The first sequence that the filter writes in the range starts with ESC,
//! and an ESC stops the open sequence in the real terminal. No character goes
//! before it, because the cursor of the output is not known at the start.
//!
//! A `ESC 7` in a real terminal also saves the pen. The restore saves the
//! position only, as `termstate` does.
//!
//! For each output event of the region, the filter first writes the
//! sequences that do not change a cell, in their order: each OSC, each
//! DECSCUSR, mode 12 (the blink of the cursor), and BEL. Then it writes the
//! paint of the screen after the event. A `Scanner` reads the events before
//! the range too, so a sequence that the start of the range splits is read
//! correctly. The filter removes each other byte of the event, and it removes
//! an output event whose new data is empty. An input event stays as it is.

mod writer;

use serde::Deserialize;

use termstate::{Cell, TerminalState, TerminalTheme};

use self::writer::{Glyph, Look, Writer, HIDE_CURSOR, SHOW_CURSOR};
use crate::ansi::{Scanner, Sequence, Step};
use crate::pipeline::{FilterRegistry, Screen, ScreenSize, ScreenView};
use crate::preview::{MAX_COLUMNS, MAX_ROWS};
use crate::types::{Event, EventType, FilterKind, FilterOptions};

/// The character BEL, which passes through.
const BEL: char = '\x07';
/// The character ESC, which starts an escape sequence.
const ESC: char = '\x1b';
/// The sequence that selects the ASCII character set.
const ASCII_SET: &str = "\x1b(B";
/// The sequence that selects the line-drawing character set.
const LINE_DRAWING_SET: &str = "\x1b(0";
/// The sequence that saves the cursor position.
const SAVE_CURSOR: &str = "\x1b7";

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
/// runs `Replay` over the events. On the region `All` the answer has the size
/// of the visible cells. On a range it has the size of the screen, and the
/// replay starts from the emulator after `view.before`. The view says which
/// path to take with `whole_timeline`.
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
    if !view.whole_timeline {
        let restore_at = (!view.region.open).then_some(view.region.length);
        let replay = Replay::masked(view.size, rows, columns, view.before)?;
        return Ok(Screen {
            events: replay.run(events, restore_at)?,
            size: view.size,
        });
    }
    let size = ScreenSize {
        width: u16::try_from(columns.len()).map_err(|error| error.to_string())?,
        height: u16::try_from(rows.len()).map_err(|error| error.to_string())?,
    };
    let events = Replay::moved(view.size, rows, columns).run(events, None)?;
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

/// How the visible cells go to the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// The region `All`: the visible cells move together, and the output has
    /// the size of the visible cells.
    Moved,
    /// The region `Range`: each cell keeps its place, and a cell that is not
    /// visible is blank.
    Masked,
}

/// What the emulator at the full size shows at one moment, and the output
/// terminal that shows the visible cells of it.
struct Replay {
    terminal: TerminalState,
    reader: Reader,
    layout: Layout,
    rows: Vec<usize>,
    columns: Vec<usize>,
    /// For each row of the screen, whether it is in `rows`.
    row_shown: Vec<bool>,
    /// For each column of the screen, whether it is in `columns`.
    column_shown: Vec<bool>,
    writer: Writer,
}

impl Replay {
    /// A replay for the region `All` of a screen of `size`, with the visible
    /// `rows` and `columns`. The output is a blank terminal of the size of the
    /// visible cells.
    fn moved(size: ScreenSize, rows: Vec<usize>, columns: Vec<usize>) -> Self {
        Self {
            terminal: new_terminal(size),
            reader: Reader::default(),
            layout: Layout::Moved,
            writer: Writer::blank(columns.len(), rows.len()),
            row_shown: shown(&rows, usize::from(size.height)),
            column_shown: shown(&columns, usize::from(size.width)),
            rows,
            columns,
        }
    }

    /// A replay for the region `Range` of a screen of `size`, with the visible
    /// `rows` and `columns`. It feeds the output events of `before` to the
    /// emulator, and each character of them to the reader. The output is
    /// what the emulator shows then.
    ///
    /// # Errors
    ///
    /// The emulator refuses the data of an event.
    fn masked(
        size: ScreenSize,
        rows: Vec<usize>,
        columns: Vec<usize>,
        before: &[Event],
    ) -> Result<Self, String> {
        let mut terminal = new_terminal(size);
        let mut reader = Reader::default();
        for event in before.iter().filter(|e| e.event_type == EventType::Output) {
            terminal
                .process_output(&event.data)
                .map_err(|error| error.to_string())?;
            for c in event.data.chars() {
                reader.read(c);
            }
        }
        let writer = Writer::from_screen(
            usize::from(size.width),
            full_target(&terminal),
            terminal.is_cursor_visible(),
        );
        Ok(Self {
            terminal,
            reader,
            layout: Layout::Masked,
            row_shown: shown(&rows, usize::from(size.height)),
            column_shown: shown(&columns, usize::from(size.width)),
            rows,
            columns,
            writer,
        })
    }

    /// The cells that the output must show now, in row order, and the place of
    /// its cursor, or `None` when the cursor is hidden.
    fn target(&self) -> (Vec<Glyph>, Option<(usize, usize)>) {
        let grid = self.terminal.get_grid();
        let (cursor_x, cursor_y) = self.terminal.get_cursor_position();
        let shown = self.terminal.is_cursor_visible();
        match self.layout {
            Layout::Moved => {
                let target = self
                    .rows
                    .iter()
                    .flat_map(|row| self.columns.iter().map(move |column| (*row, *column)))
                    .map(|(row, column)| {
                        grid.get(row)
                            .and_then(|cells| cells.get(column))
                            .map_or(Glyph::BLANK, glyph_of)
                    })
                    .collect();
                let cursor = shown
                    .then(|| {
                        self.columns
                            .iter()
                            .position(|column| *column == cursor_x)
                            .zip(self.rows.iter().position(|row| *row == cursor_y))
                    })
                    .flatten();
                (target, cursor)
            }
            Layout::Masked => {
                let target = grid
                    .iter()
                    .enumerate()
                    .flat_map(|(row, cells)| {
                        let row_shown = self.row_shown.get(row).copied().unwrap_or(false);
                        cells.iter().enumerate().map(move |(column, cell)| {
                            let column_shown =
                                self.column_shown.get(column).copied().unwrap_or(false);
                            if row_shown && column_shown {
                                glyph_of(cell)
                            } else {
                                Glyph::BLANK
                            }
                        })
                    })
                    .collect();
                let visible = self.row_shown.get(cursor_y).copied().unwrap_or(false)
                    && self.column_shown.get(cursor_x).copied().unwrap_or(false);
                let cursor = (shown && visible).then_some((cursor_x, cursor_y));
                (target, cursor)
            }
        }
    }

    /// The paint of the screen that the emulator shows now.
    fn paint(&mut self) -> String {
        let (target, cursor) = self.target();
        self.writer.paint(&target, cursor)
    }

    /// The data of the first event, at time 0. A range first sets the
    /// character set that the paint needs, then it paints the mask.
    fn start(&mut self) -> String {
        let mut data = String::new();
        if self.layout == Layout::Masked && self.terminal.uses_line_drawing() {
            data.push_str(ASCII_SET);
        }
        data.push_str(&self.paint());
        data
    }

    /// The data of the event that ends a range: the full screen, then the
    /// state of the terminal, in the order that the module doc gives.
    fn restore(&mut self) -> String {
        let mut data = String::new();
        self.writer
            .paint_cells(&full_target(&self.terminal), &mut data);
        let (top, bottom) = self.terminal.scroll_region();
        data.push_str(&format!(
            "\x1b[{};{}r",
            top.saturating_add(1),
            bottom.saturating_add(1)
        ));
        let pen = pen_of(&self.terminal);
        data.push_str(&writer::sgr(&pen));
        self.writer.set_pen(pen);
        let (saved_x, saved_y) = self.terminal.saved_cursor_position();
        data.push_str(&writer::cup(saved_x, saved_y));
        data.push_str(SAVE_CURSOR);
        if self.terminal.uses_line_drawing() {
            data.push_str(LINE_DRAWING_SET);
        }
        let (cursor_x, cursor_y) = self.terminal.get_cursor_position();
        data.push_str(&writer::cup(cursor_x, cursor_y));
        data.push_str(if self.terminal.is_cursor_visible() {
            SHOW_CURSOR
        } else {
            HIDE_CURSOR
        });
        // The real terminal reads the start of the open sequence again, so
        // the events after the range finish it.
        data.push_str(self.reader.open());
        data
    }

    /// The events of the region after the filter. The first event is the
    /// start, at time 0. When `restore_at` is a time, the last event is the
    /// restore at that time.
    ///
    /// # Errors
    ///
    /// The emulator refuses the data of an event.
    fn run(mut self, events: &[Event], restore_at: Option<f64>) -> Result<Vec<Event>, String> {
        let mut answer = Vec::new();
        let start = self.start();
        if !start.is_empty() {
            answer.push(Event {
                time: 0.0,
                event_type: EventType::Output,
                data: start,
            });
        }
        for event in events {
            if event.event_type != EventType::Output {
                answer.push(event.clone());
                continue;
            }
            let mut data: String = event.data.chars().map(|c| self.reader.read(c)).collect();
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
        if let Some(time) = restore_at {
            answer.push(Event {
                time,
                event_type: EventType::Output,
                data: self.restore(),
            });
        }
        Ok(answer)
    }
}

/// The reader of the escape sequences of the input. It keeps the text of the
/// sequence that is open, so the restore at the end of a range can write it
/// again.
#[derive(Debug, Default)]
struct Reader {
    scanner: Scanner,
    /// The characters of the open sequence, from its first character. A C0
    /// control other than ESC inside it is not kept, because the emulator ran
    /// it where it stood.
    open: String,
}

impl Reader {
    /// Read `c`. Return the part of the stream that passes through.
    fn read(&mut self, c: char) -> String {
        let step = self.scanner.feed(c);
        if !self.scanner.is_pending() {
            self.open.clear();
        } else {
            match step {
                Step::Restarted => {
                    self.open.clear();
                    self.open.push(c);
                }
                Step::EscapeBreak(_) => {
                    self.open.clear();
                    self.open.push(ESC);
                    self.open.push(c);
                }
                _ if c == ESC || !c.is_ascii_control() => self.open.push(c),
                _ => {}
            }
        }
        pass_through(&step, c)
    }

    /// The characters of the open sequence, or an empty string.
    fn open(&self) -> &str {
        &self.open
    }
}

/// For each of `len` indexes, whether `indexes` holds it.
fn shown(indexes: &[usize], len: usize) -> Vec<bool> {
    let mut shown = vec![false; len];
    for index in indexes {
        if let Some(slot) = shown.get_mut(*index) {
            *slot = true;
        }
    }
    shown
}

/// An emulator of a screen of `size`.
fn new_terminal(size: ScreenSize) -> TerminalState {
    TerminalState::new(
        usize::from(size.width),
        usize::from(size.height),
        TerminalTheme::auto(),
    )
}

/// The glyphs of the whole grid of `terminal`, in row order.
fn full_target(terminal: &TerminalState) -> Vec<Glyph> {
    terminal
        .get_grid()
        .iter()
        .flat_map(|cells| cells.iter().map(glyph_of))
        .collect()
}

/// The look that the pen of `terminal` gives the next printed character.
fn pen_of(terminal: &TerminalState) -> Look {
    Look {
        fg: terminal.current_fg_source,
        bg: terminal.current_bg_source,
        bold: terminal.bold,
        italic: terminal.italic,
        underline: terminal.underline,
        dim: terminal.dim,
        blink: terminal.blink,
        reverse: terminal.reverse,
        hidden: terminal.hidden,
        strikethrough: terminal.strikethrough,
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
            dim: cell.dim,
            blink: cell.blink,
            reverse: cell.reverse,
            hidden: cell.hidden,
            strikethrough: cell.strikethrough,
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
