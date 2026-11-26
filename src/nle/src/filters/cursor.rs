//! The filter that sets how the cursor looks.
//!
//! The filter rewrites the escape sequences that set the cursor, in the data
//! of the output events. The options, with their keys in the project file:
//!
//! - `visibility`: `honor` (the default) obeys the recording.
//!   `always-visible` and `always-hidden` set the cursor and drop each
//!   DECTCEM change (`CSI ? 25 h` and `CSI ? 25 l`) of the recording.
//!   `smart` obeys the recording, but hides the cursor during a burst of
//!   output.
//! - `burst_gap`: for `smart` only, in seconds, above zero. The default is
//!   `DEFAULT_BURST_GAP`.
//! - `shape`: `block`, `underline`, or `bar`.
//! - `blink`: `true` or `false`.
//! - `color`: `#rrggbb`.
//!
//! An option that the options do not name obeys the recording. An unknown
//! key or value fails the render.

use serde::Deserialize;

use crate::ansi::{Scanner, Sequence, Step};
use crate::pipeline::{FilterRegistry, RegionView};
use crate::types::{Event, EventType, FilterKind, FilterOptions, Seconds};

/// The kind of the filter that sets how the cursor looks.
pub const CURSOR: &str = "cursor";

/// The burst gap of the visibility `smart` when the options do not name
/// one, in seconds.
pub const DEFAULT_BURST_GAP: f64 = 0.1;

/// Register the cursor filter under `CURSOR`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_in_place(FilterKind::new(CURSOR), filter);
}

/// The DECTCEM sequence that shows the cursor.
const SHOW: &str = "\x1b[?25h";
/// The DECTCEM sequence that hides the cursor.
const HIDE: &str = "\x1b[?25l";
/// The DEC private mode of DECTCEM, the visibility of the cursor.
const DECTCEM: u32 = 25;
/// The DEC private mode that starts (`h`) and stops (`l`) the blink of the
/// cursor.
const BLINK_MODE: u32 = 12;
/// The largest DECSCUSR value that a terminal knows. A terminal ignores a
/// larger one.
const LAST_STYLE: u16 = 6;

/// The cursor filter.
///
/// It sets the cursor at the start of its region, and then it reads the
/// output events with one `Scanner`. It drops each sequence of the recording
/// that sets a part of the cursor that the options set. Each such sequence
/// is rewritten in the event where it completes. Each other byte stays in
/// its event. At the end of a region that is not open, it sets the state of
/// the recording back: the state that the last sequence of the region gave,
/// or else the default of a terminal.
///
/// The filter sees only its region. A sequence that the start of the region
/// splits is not seen, and the state of the recording before the region is
/// taken to be the default of a terminal.
fn filter(
    events: &[Event],
    region: RegionView,
    options: &FilterOptions,
) -> Result<Vec<Event>, String> {
    let cursor = Cursor::parse(options)?;
    if cursor.owns_nothing() {
        return Ok(events.to_vec());
    }
    Ok(Rewriter::new(&cursor).run(events, region))
}

/// The visibility in the project file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Visibility {
    #[default]
    Honor,
    AlwaysVisible,
    AlwaysHidden,
    Smart,
}

/// A shape of the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Shape {
    Block,
    Underline,
    Bar,
}

/// The shape and the blink of a DECSCUSR value (`CSI Ps SP q`). By the
/// VT520 manual, 0 and 1 are a blinking block, 2 a steady block, 3 and 4 an
/// underline, 5 and 6 a bar. An odd value blinks.
fn style_parts(style: u16) -> (Shape, bool) {
    match style {
        0 | 1 => (Shape::Block, true),
        2 => (Shape::Block, false),
        3 => (Shape::Underline, true),
        4 => (Shape::Underline, false),
        5 => (Shape::Bar, true),
        _ => (Shape::Bar, false),
    }
}

/// The DECSCUSR value of `shape` with `blink`.
fn style_value(shape: Shape, blink: bool) -> u16 {
    let steady = u16::from(!blink);
    match shape {
        Shape::Block => 1 + steady,
        Shape::Underline => 3 + steady,
        Shape::Bar => 5 + steady,
    }
}

/// A color of the cursor: `#` and six hexadecimal digits.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
struct Color(String);

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let mut chars = text.chars();
        let valid = chars.next() == Some('#')
            && chars.clone().count() == 6
            && chars.all(|c| c.is_ascii_hexdigit());
        if valid {
            Ok(Self(text))
        } else {
            Err(format!("the color \"{text}\" is not of the form #rrggbb"))
        }
    }
}

/// The options as the project file holds them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorFile {
    #[serde(default)]
    visibility: Visibility,
    burst_gap: Option<Seconds>,
    shape: Option<Shape>,
    blink: Option<bool>,
    color: Option<Color>,
}

/// What the filter does with the visibility of the cursor.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Show {
    /// Obey the recording.
    Honor,
    /// Show (`true`) or hide (`false`) the cursor at all times.
    Always(bool),
    /// Obey the recording, but hide the cursor during a burst: two or more
    /// output events, each less than `burst_gap` seconds after the one
    /// before it, with no input event between them.
    Smart { burst_gap: f64 },
}

/// The options of the filter, after their checks.
#[derive(Debug, Clone, PartialEq)]
struct Cursor {
    show: Show,
    shape: Option<Shape>,
    blink: Option<bool>,
    color: Option<String>,
}

impl TryFrom<CursorFile> for Cursor {
    type Error = String;

    fn try_from(file: CursorFile) -> Result<Self, String> {
        let show = match (file.visibility, file.burst_gap) {
            (Visibility::Smart, gap) => {
                let burst_gap = gap.map_or(DEFAULT_BURST_GAP, Seconds::get);
                if burst_gap <= 0.0 {
                    return Err(format!(
                        "burst_gap must be above zero, and it is {burst_gap} s"
                    ));
                }
                Show::Smart { burst_gap }
            }
            (_, Some(_)) => {
                return Err("burst_gap applies only when visibility is smart".to_string());
            }
            (Visibility::Honor, None) => Show::Honor,
            (Visibility::AlwaysVisible, None) => Show::Always(true),
            (Visibility::AlwaysHidden, None) => Show::Always(false),
        };
        Ok(Self {
            show,
            shape: file.shape,
            blink: file.blink,
            color: file.color.map(|color| color.0),
        })
    }
}

impl Cursor {
    /// True when the options set no part of the cursor.
    fn owns_nothing(&self) -> bool {
        self.show == Show::Honor && !self.owns_style() && self.color.is_none()
    }

    /// True when the options set the shape or the blink, thus DECSCUSR.
    fn owns_style(&self) -> bool {
        self.shape.is_some() || self.blink.is_some()
    }

    /// Read and check the options of the filter.
    fn parse(options: &FilterOptions) -> Result<Self, String> {
        let value = serde_json::to_value(options).map_err(|error| error.to_string())?;
        let file: CursorFile = serde_json::from_value(value).map_err(|error| error.to_string())?;
        Self::try_from(file)
    }
}

/// The state of the cursor that a stream of output sets.
#[derive(Debug, Clone, PartialEq)]
struct CursorState {
    visible: bool,
    /// The last DECSCUSR value.
    style: u16,
    /// The last set (`true`) or reset (`false`) of mode 12, when one came.
    blink_mode: Option<bool>,
}

impl Default for CursorState {
    /// The state of a terminal after a reset.
    fn default() -> Self {
        Self {
            visible: true,
            style: 0,
            blink_mode: None,
        }
    }
}

/// The state that the filter brings the stream to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The state that the options ask for.
    Wanted,
    /// The state of the recording.
    Recording,
}

/// The rewrite of the events of one region.
struct Rewriter<'a> {
    cursor: &'a Cursor,
    scanner: Scanner,
    /// The state that the recording sets.
    recording: CursorState,
    /// The visibility that the answer set last, or `None` when it is not
    /// known.
    shown_visible: Option<bool>,
    /// The DECSCUSR value that the answer set last, or `None` when it is not
    /// known.
    shown_style: Option<u16>,
    /// The answer.
    out: Vec<Event>,
    /// Where each piece of the open sequence starts: the index of its event
    /// in `out`, and the byte offset in the data of that event. Each piece
    /// runs to the end of that data.
    pieces: Vec<(usize, usize)>,
    /// Where the last character went, as a piece.
    last: (usize, usize),
}

impl<'a> Rewriter<'a> {
    fn new(cursor: &'a Cursor) -> Self {
        Self {
            cursor,
            scanner: Scanner::new(),
            recording: CursorState::default(),
            shown_visible: None,
            shown_style: None,
            out: Vec::new(),
            pieces: Vec::new(),
            last: (0, 0),
        }
    }

    /// The answer of the filter for `events`.
    fn run(mut self, events: &[Event], region: RegionView) -> Vec<Event> {
        // An event at the start of such a region sets nothing that a frame
        // shows.
        if events.is_empty() && (region.open || region.length <= 0.0) {
            return Vec::new();
        }
        let start = self.emissions(Target::Wanted);
        for event in events {
            match event.event_type {
                EventType::Input => self.out.push(event.clone()),
                EventType::Output => self.output(event),
            }
        }

        if let Some(&(index, offset)) = self.pieces.first() {
            // A sequence is open at the end, so nothing could go after it.
            // Put the rest before it.
            let mut rest = self.emissions(Target::Wanted);
            if !region.open {
                rest.push_str(&self.emissions(Target::Recording));
            }
            if let Some(event) = self.out.get_mut(index) {
                event.data.insert_str(offset, &rest);
            }
        } else if !region.open {
            let restore = self.emissions(Target::Recording);
            if !restore.is_empty() {
                self.out.push(output_event(region.length, restore));
            }
        }
        if !start.is_empty() {
            self.out.insert(0, output_event(0.0, start));
        }
        self.out
    }

    /// Rewrite one output event into `out`.
    fn output(&mut self, event: &Event) {
        let index = self.out.len();
        self.out.push(output_event(event.time, String::new()));
        for c in event.data.chars() {
            let step = self.scanner.feed(c);
            self.take(step, c, index);
        }
    }

    /// Put `c` into the event at `index` of `out`, by its `step`.
    fn take(&mut self, step: Step, c: char, index: usize) {
        match step {
            Step::Text => self.push(index, c),
            Step::Pending => {
                self.mark(index);
                self.push(index, c);
            }
            Step::Done(sequence) => {
                self.mark(index);
                self.push(index, c);
                self.finish(&sequence, index);
            }
            Step::Ignored => {
                self.push(index, c);
                self.pieces.clear();
            }
            Step::Aborted => {
                self.pieces.clear();
                self.push(index, c);
            }
            Step::Restarted => {
                self.pieces.clear();
                self.mark(index);
                self.push(index, c);
            }
            Step::EscapeBreak(inner) => {
                // The ESC before `c` starts the new sequence.
                self.pieces.clear();
                self.pieces.push(self.last);
                self.take(*inner, c, index);
            }
        }
    }

    /// The data of the event at `index` of `out`.
    fn data(&mut self, index: usize) -> Option<&mut String> {
        self.out.get_mut(index).map(|event| &mut event.data)
    }

    /// Append `c` to the event at `index`.
    fn push(&mut self, index: usize, c: char) {
        if let Some(event) = self.out.get_mut(index) {
            self.last = (index, event.data.len());
            event.data.push(c);
        }
    }

    /// Start a piece of the open sequence in the event at `index`, when the
    /// sequence has no piece there yet.
    fn mark(&mut self, index: usize) {
        if self.pieces.last().map(|piece| piece.0) != Some(index) {
            let offset = self.data(index).map_or(0, |data| data.len());
            self.pieces.push((index, offset));
        }
    }

    /// The sequence that completed in the event at `index`: rewrite it when
    /// it sets a part of the cursor that the filter owns, and then bring the
    /// stream to the wanted state.
    fn finish(&mut self, sequence: &Sequence, index: usize) {
        let pieces = std::mem::take(&mut self.pieces);
        if let Some(replacement) = self.rewrite(sequence) {
            for (piece, offset) in pieces {
                if let Some(data) = self.data(piece) {
                    data.truncate(offset);
                }
            }
            if let Some(data) = self.data(index) {
                data.push_str(&replacement);
            }
        }
        let wanted = self.emissions(Target::Wanted);
        if let Some(data) = self.data(index) {
            data.push_str(&wanted);
        }
    }

    /// Follow the state that `sequence` sets in the recording. Returns the
    /// text that replaces the sequence, or `None` when it stays.
    fn rewrite(&mut self, sequence: &Sequence) -> Option<String> {
        match sequence {
            Sequence::Csi {
                params,
                intermediates,
                final_char: final_char @ ('h' | 'l'),
                controls,
            } if intermediates.is_empty() => {
                let modes = params.strip_prefix('?')?;
                self.dec_modes(modes, *final_char, controls)
            }
            // DECSTR, the soft reset, shows the cursor.
            Sequence::Csi {
                params,
                intermediates,
                final_char: 'p',
                ..
            } if params.is_empty() && intermediates == "!" => {
                self.recording.visible = true;
                self.shown_visible = Some(true);
                None
            }
            // DECSCUSR.
            Sequence::Csi {
                params,
                intermediates,
                final_char: 'q',
                controls,
            } if intermediates == " " && self.cursor.owns_style() => {
                // A terminal reads the first parameter. A value that it does
                // not know changes nothing, but the filter drops it too.
                let first = params.split(';').next().unwrap_or_default();
                let value: u64 = if first.is_empty() {
                    0
                } else {
                    first.parse().ok()?
                };
                if let Some(style) = u16::try_from(value)
                    .ok()
                    .filter(|style| *style <= LAST_STYLE)
                {
                    self.recording.style = style;
                }
                Some(controls.clone())
            }
            // RIS, the full reset.
            Sequence::Escape('c') => {
                self.recording = CursorState::default();
                self.shown_visible = Some(true);
                self.shown_style = Some(0);
                None
            }
            _ => None,
        }
    }

    /// Drop the modes of a DEC private mode set (`h`) or reset (`l`) that the
    /// filter owns. Returns the rest of the sequence after the controls in
    /// it, or `None` when the filter owns no mode of it.
    fn dec_modes(&mut self, modes: &str, final_char: char, controls: &str) -> Option<String> {
        let set = final_char == 'h';
        let mut kept = Vec::new();
        let mut dropped = false;
        for mode in modes.split(';') {
            match mode.parse() {
                Ok(DECTCEM) if self.cursor.show != Show::Honor => {
                    self.recording.visible = set;
                    dropped = true;
                }
                Ok(BLINK_MODE) if self.cursor.blink.is_some() => {
                    self.recording.blink_mode = Some(set);
                    dropped = true;
                }
                _ => kept.push(mode),
            }
        }
        if !dropped {
            return None;
        }
        let mut text = controls.to_string();
        if !kept.is_empty() {
            text.push_str(&format!("\x1b[?{}{final_char}", kept.join(";")));
        }
        Some(text)
    }

    /// The sequences that bring the answer to `target`, for each part that
    /// the filter owns. The caller puts them where no sequence is open.
    fn emissions(&mut self, target: Target) -> String {
        let mut text = String::new();
        if let Some(visible) = self.wanted_visible(target) {
            if self.shown_visible != Some(visible) {
                text.push_str(if visible { SHOW } else { HIDE });
                self.shown_visible = Some(visible);
            }
        }
        if let Some(style) = self.wanted_style(target) {
            if self.shown_style != Some(style) {
                text.push_str(&format!("\x1b[{style} q"));
                self.shown_style = Some(style);
            }
        }
        if target == Target::Recording && self.cursor.blink.is_some() {
            if let Some(set) = self.recording.blink_mode {
                let final_char = if set { 'h' } else { 'l' };
                text.push_str(&format!("\x1b[?{BLINK_MODE}{final_char}"));
            }
        }
        text
    }

    /// The DECSCUSR value that `target` asks for, or `None` when the filter
    /// does not own it.
    fn wanted_style(&self, target: Target) -> Option<u16> {
        if !self.cursor.owns_style() {
            return None;
        }
        match target {
            Target::Recording => Some(self.recording.style),
            Target::Wanted => {
                let (shape, blink) = style_parts(self.recording.style);
                Some(style_value(
                    self.cursor.shape.unwrap_or(shape),
                    self.cursor.blink.unwrap_or(blink),
                ))
            }
        }
    }

    /// The visibility that `target` asks for, or `None` when the filter does
    /// not own it.
    fn wanted_visible(&self, target: Target) -> Option<bool> {
        match (self.cursor.show, target) {
            (Show::Honor, _) => None,
            (_, Target::Recording) => Some(self.recording.visible),
            (Show::Always(visible), Target::Wanted) => Some(visible),
            (Show::Smart { .. }, Target::Wanted) => Some(self.recording.visible),
        }
    }
}

/// An output event at `time` with `data`.
fn output_event(time: f64, data: String) -> Event {
    Event {
        time,
        event_type: EventType::Output,
        data,
    }
}

#[cfg(test)]
mod tests;
