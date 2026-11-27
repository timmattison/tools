//! The search and the rewrite of the event data.
//!
//! The work has four steps. First, the data of the events of one stream is
//! joined into one string. Second, a tokenizer cuts the string into runs of
//! text. Third, the text runs form segments, and the matcher searches each
//! segment. Fourth, each match is mapped back to pieces of the original
//! events, and each event is built again.

use std::ops::Range;

use unicode_width::UnicodeWidthStr;

use crate::filters::redaction::{Redaction, Replacement, ASTERISK, BLOCK, REDACT_GROUP};
use crate::types::{Event, EventType};

/// The error text for a range that is not a character boundary. It cannot
/// happen, because each cut falls on an ASCII byte or on an event boundary.
const BOUNDARY_ERROR: &str = "a match does not fall on a character boundary";

/// A run of text in the joined string. The segment 0 is the visible text.
/// Each other segment is the text of one OSC sequence, see [`osc_text`].
struct Run {
    segment: usize,
    range: Range<usize>,
}

/// A part of a segment that lies inside one event.
struct Chunk {
    /// Where the chunk starts in the text of its segment.
    segment_start: usize,
    /// The index of the event in the list of all events.
    event: usize,
    /// Where the chunk starts in the data of its event.
    event_start: usize,
    /// The length of the chunk in bytes.
    len: usize,
}

/// A segment of text: the text, and the chunks that it is made from.
#[derive(Default)]
struct Segment {
    text: String,
    chunks: Vec<Chunk>,
}

/// A change to the data of one event: the range of bytes and its new text.
struct Edit {
    range: Range<usize>,
    text: String,
}

/// Replace each match in the events that the redaction covers.
pub(super) fn rewrite(events: Vec<Event>, redaction: &Redaction) -> Result<Vec<Event>, String> {
    let mut edits: Vec<Vec<Edit>> = events.iter().map(|_| Vec::new()).collect();
    let mut types = vec![EventType::Output];
    if redaction.includes_input() {
        types.push(EventType::Input);
    }
    for event_type in types {
        let stream: Vec<usize> = events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.event_type == event_type)
            .map(|(index, _)| index)
            .collect();
        edit_stream(&events, &stream, &event_type, redaction, &mut edits)?;
    }
    events
        .into_iter()
        .zip(edits)
        .map(|(event, edits)| apply(event, edits))
        .collect()
}

/// Find the matches in one stream and add their edits.
fn edit_stream(
    events: &[Event],
    stream: &[usize],
    event_type: &EventType,
    redaction: &Redaction,
    edits: &mut [Vec<Edit>],
) -> Result<(), String> {
    let datas: Vec<&str> = stream
        .iter()
        .map(|&index| events.get(index).map_or("", |event| event.data.as_str()))
        .collect();
    let joined: String = datas.concat();
    let mut starts = Vec::with_capacity(datas.len() + 1);
    let mut total = 0;
    for data in &datas {
        starts.push(total);
        total += data.len();
    }
    starts.push(total);
    let segments = build_segments(&joined, &runs(&joined, event_type), &starts, stream)?;
    for segment in &segments {
        edit_segment(segment, events, redaction, edits)?;
    }
    Ok(())
}

/// The escape byte.
const ESC: u8 = 0x1B;

/// The bell byte. It ends an OSC sequence.
const BEL: u8 = 0x07;

/// The cancel byte. It ends an OSC sequence and a control string.
const CAN: u8 = 0x18;

/// The substitute byte. It ends an OSC sequence and a control string.
const SUB: u8 = 0x1A;

/// The segment of the visible text.
const VISIBLE: usize = 0;

/// The OSC command number of the clipboard. Its text is base64 data, which a
/// text search cannot read, and a match corrupts the data.
const CLIPBOARD_OSC: u32 = 52;

/// The OSC command number of the iTerm2 commands. Its text is `Key=value` or
/// `Key`. The shell integration of iTerm2 writes `RemoteHost=user@host` and
/// `CurrentDir=/Users/user` at each prompt, and these values are plain text.
const ITERM2_OSC: u32 = 1337;

/// The iTerm2 keys whose value holds base64 data, for example the image of
/// `File` and the variable of `SetUserVar`. The iTerm2 documentation of the
/// escape codes and of the badges names them. The value of each other key is
/// text.
const ITERM2_ENCODED_KEYS: [&[u8]; 10] = [
    b"Copy",
    b"File",
    b"FilePart",
    b"MultipartFile",
    b"OpenURL",
    b"ReportVariable",
    b"SetBackgroundImageFile",
    b"SetBadgeFormat",
    b"SetProfileProperty",
    b"SetUserVar",
];

/// Cut the joined string into runs of text. Escape sequences are not text.
/// The text of an OSC sequence is a run of its own segment. `event_type` is
/// the type of the stream: in the input stream, a string introducer is a key.
fn runs(joined: &str, event_type: &EventType) -> Vec<Run> {
    let bytes = joined.as_bytes();
    let mut runs = Vec::new();
    let mut at = 0;
    let mut segments = 1;
    while at < bytes.len() {
        if bytes.get(at) != Some(&ESC) {
            let end = bytes
                .get(at..)
                .and_then(|rest| rest.iter().position(|&byte| byte == ESC))
                .map_or(bytes.len(), |offset| at + offset);
            runs.push(Run {
                segment: VISIBLE,
                range: at..end,
            });
            at = end;
            continue;
        }
        let (payload, end) = escape_sequence(bytes, at, event_type);
        if let Some(range) = payload.filter(|range| !range.is_empty()) {
            runs.push(Run {
                segment: segments,
                range,
            });
            segments += 1;
        }
        at = end;
    }
    runs
}

/// Read the escape sequence that starts at `at`, where `bytes[at]` is ESC.
/// The answer is the range of the text when the sequence is an OSC sequence
/// with text, see [`osc_text`], and the end of the sequence. The end is always
/// above `at`.
///
/// An OSC sequence or a control string ends where a terminal ends it, see
/// [`string_end`]. In the input stream, `ESC ]`, `ESC P`, `ESC X`, `ESC ^`,
/// and `ESC _` are one key press of two bytes, and not an introducer.
fn escape_sequence(
    bytes: &[u8],
    at: usize,
    event_type: &EventType,
) -> (Option<Range<usize>>, usize) {
    let length = bytes.len();
    match bytes.get(at + 1) {
        Some(b'[') => {
            let mut end = at + 2;
            end = skip_while(bytes, end, |byte| (0x20..=0x3F).contains(&byte));
            if bytes
                .get(end)
                .is_some_and(|byte| (0x40..=0x7E).contains(byte))
            {
                end += 1;
            }
            (None, end)
        }
        Some(b']' | b'P' | b'X' | b'^' | b'_') if *event_type == EventType::Input => (None, at + 2),
        Some(b']') => {
            let start = at + 2;
            let (payload_end, end) = string_end(bytes, start, true);
            (osc_text(bytes, start..payload_end), end)
        }
        Some(b'P' | b'X' | b'^' | b'_') => (None, string_end(bytes, at + 2, false).1),
        Some(_) => {
            let mut end = skip_while(bytes, at + 1, |byte| (0x20..=0x2F).contains(&byte));
            if bytes
                .get(end)
                .is_some_and(|byte| (0x30..=0x7E).contains(byte))
            {
                end += 1;
            }
            (None, end)
        }
        None => (None, length),
    }
}

/// The range of the text in the OSC payload at `payload`. The payload is
/// `Ps ; Pt`, where `Ps` is the command number, and only `Pt` is text. Thus a
/// match never changes the kind of the sequence. A payload with no `;` after
/// its number has no text, for example `104`, which resets the colors. The
/// clipboard, [`CLIPBOARD_OSC`], has no text either. The text of an iTerm2
/// command, [`ITERM2_OSC`], is the value of its key, see [`iterm2_value`]. A
/// terminal reads leading zeros, thus `01337` is 1337.
fn osc_text(bytes: &[u8], payload: Range<usize>) -> Option<Range<usize>> {
    let content = bytes.get(payload.clone())?;
    let digits = content
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if content.get(digits) != Some(&b';') {
        return None;
    }
    let text = payload.start + digits + 1..payload.end;
    let number = content
        .get(..digits)
        .and_then(|number| std::str::from_utf8(number).ok())
        .and_then(|number| number.parse::<u32>().ok());
    match number {
        Some(CLIPBOARD_OSC) => None,
        Some(ITERM2_OSC) => iterm2_value(bytes, text),
        _ => Some(text),
    }
}

/// The range of the value in the iTerm2 text at `text`, which is `Key=value`
/// or `Key`. The key and its `=` are not text, thus a match never changes the
/// command. A text with no `=` has no value, for example `SetMark`. The keys
/// of [`ITERM2_ENCODED_KEYS`] have no text either. The comparison keeps the
/// case of the key, thus `file` is not `File`.
fn iterm2_value(bytes: &[u8], text: Range<usize>) -> Option<Range<usize>> {
    let content = bytes.get(text.clone())?;
    let key_length = content.iter().position(|&byte| byte == b'=')?;
    let key = content.get(..key_length)?;
    (!ITERM2_ENCODED_KEYS.contains(&key)).then_some(text.start + key_length + 1..text.end)
}

/// Find the end of an OSC payload or a control string that starts at `start`.
/// The answer is the end of the content and the end of the sequence. The
/// string ends at the first ESC, CAN, or SUB, and an OSC payload also ends at
/// BEL. `ESC \`, CAN, SUB, and BEL belong to the sequence. Another ESC does
/// not, because it starts the next sequence. The end of the stream ends the
/// string too.
fn string_end(bytes: &[u8], start: usize, bel_ends: bool) -> (usize, usize) {
    let length = bytes.len();
    let mut end = start;
    while end < length {
        match bytes.get(end) {
            Some(&ESC) if bytes.get(end + 1) == Some(&b'\\') => return (end, end + 2),
            Some(&ESC) => return (end, end),
            Some(&CAN | &SUB) => return (end, end + 1),
            Some(&BEL) if bel_ends => return (end, end + 1),
            _ => end += 1,
        }
    }
    (length, length)
}

/// The first index from `from` where `bytes` has no byte that `test` accepts.
fn skip_while(bytes: &[u8], from: usize, test: impl Fn(u8) -> bool) -> usize {
    let mut at = from;
    while bytes.get(at).is_some_and(|&byte| test(byte)) {
        at += 1;
    }
    at
}

/// Build the segments from the runs. A run that crosses an event boundary
/// becomes one chunk for each event.
fn build_segments(
    joined: &str,
    runs: &[Run],
    starts: &[usize],
    stream: &[usize],
) -> Result<Vec<Segment>, String> {
    let mut segments: Vec<Segment> = Vec::new();
    for run in runs {
        while segments.len() <= run.segment {
            segments.push(Segment::default());
        }
        let Some(segment) = segments.get_mut(run.segment) else {
            continue;
        };
        let mut at = run.range.start;
        while at < run.range.end {
            let position = starts
                .partition_point(|&start| start <= at)
                .saturating_sub(1);
            let event_end = starts.get(position + 1).copied().unwrap_or(run.range.end);
            let event_start = starts.get(position).copied().unwrap_or(0);
            let take = run.range.end.min(event_end);
            let text = joined.get(at..take).ok_or(BOUNDARY_ERROR)?;
            segment.chunks.push(Chunk {
                segment_start: segment.text.len(),
                event: stream.get(position).copied().ok_or(BOUNDARY_ERROR)?,
                event_start: at - event_start,
                len: take - at,
            });
            segment.text.push_str(text);
            at = take;
        }
    }
    Ok(segments)
}

/// Search one segment and add the edit of each match.
fn edit_segment(
    segment: &Segment,
    events: &[Event],
    redaction: &Redaction,
    edits: &mut [Vec<Edit>],
) -> Result<(), String> {
    let has_group = redaction
        .matcher
        .capture_names()
        .any(|name| name == Some(REDACT_GROUP));
    let ranges: Vec<Range<usize>> = if has_group {
        redaction
            .matcher
            .captures_iter(&segment.text)
            .filter_map(|captures| {
                captures
                    .name(REDACT_GROUP)
                    .or_else(|| captures.get(0))
                    .map(|found| found.range())
            })
            .collect()
    } else {
        redaction
            .matcher
            .find_iter(&segment.text)
            .map(|found| found.range())
            .collect()
    };
    for range in ranges.into_iter().filter(|range| !range.is_empty()) {
        let pieces = pieces_of(segment, &range);
        for (number, (event, piece)) in pieces.into_iter().enumerate() {
            let data = events
                .get(event)
                .and_then(|event| event.data.get(piece.clone()))
                .ok_or(BOUNDARY_ERROR)?;
            let text = replacement_for(redaction.replacement(), number, data);
            if let Some(list) = edits.get_mut(event) {
                list.push(Edit { range: piece, text });
            }
        }
    }
    Ok(())
}

/// The pieces of the events that a range of a segment covers. A piece is an
/// event index and a byte range in the data of that event.
fn pieces_of(segment: &Segment, range: &Range<usize>) -> Vec<(usize, Range<usize>)> {
    let first = segment
        .chunks
        .partition_point(|chunk| chunk.segment_start + chunk.len <= range.start);
    segment
        .chunks
        .iter()
        .skip(first)
        .take_while(|chunk| chunk.segment_start < range.end)
        .map(|chunk| {
            let from = range.start.max(chunk.segment_start) - chunk.segment_start;
            let to = range.end.min(chunk.segment_start + chunk.len) - chunk.segment_start;
            (
                chunk.event,
                chunk.event_start + from..chunk.event_start + to,
            )
        })
        .collect()
}

/// The new text of piece number `number` of a match. `data` is its old text.
fn replacement_for(replacement: &Replacement, number: usize, data: &str) -> String {
    match replacement {
        Replacement::Text(text) if number == 0 => text.clone(),
        Replacement::Text(_) => String::new(),
        Replacement::Asterisks => mask(data, ASTERISK),
        Replacement::Blocks => mask(data, BLOCK),
    }
}

/// Replace each run of characters that are not control characters with
/// `mark`, one for each terminal column. Control characters stay.
fn mask(data: &str, mark: char) -> String {
    let mut out = String::with_capacity(data.len());
    let mut run = String::new();
    for character in data.chars() {
        if character.is_control() {
            push_mask(&mut out, &run, mark);
            run.clear();
            out.push(character);
        } else {
            run.push(character);
        }
    }
    push_mask(&mut out, &run, mark);
    out
}

/// Add one `mark` to `out` for each terminal column of `run`.
fn push_mask(out: &mut String, run: &str, mark: char) {
    out.extend(std::iter::repeat_n(mark, UnicodeWidthStr::width(run)));
}

/// Build the data of one event again from its edits.
fn apply(event: Event, mut edits: Vec<Edit>) -> Result<Event, String> {
    if edits.is_empty() {
        return Ok(event);
    }
    edits.sort_by_key(|edit| edit.range.start);
    let mut data = String::with_capacity(event.data.len());
    let mut at = 0;
    for edit in &edits {
        data.push_str(event.data.get(at..edit.range.start).ok_or(BOUNDARY_ERROR)?);
        data.push_str(&edit.text);
        at = edit.range.end;
    }
    data.push_str(event.data.get(at..).ok_or(BOUNDARY_ERROR)?);
    Ok(Event { data, ..event })
}
