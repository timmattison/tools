//! The search and the rewrite of the event data.
//!
//! The work has four steps. First, the data of the events of one stream is
//! joined into one string. Second, a tokenizer cuts the string into runs of
//! text. Third, the text runs form segments, and the matcher searches each
//! segment. Fourth, each match is mapped back to pieces of the original
//! events, and each event is built again.

use std::ops::Range;

use unicode_width::UnicodeWidthStr;

use crate::filters::redaction::{ASTERISK, BLOCK, REDACT_GROUP, Redaction, Replacement};
use crate::types::{Event, EventType};

/// The error text for a range that is not a character boundary. It cannot
/// happen, because each cut falls on an ASCII byte or on an event boundary.
const BOUNDARY_ERROR: &str = "a match does not fall on a character boundary";

/// A run of text in the joined string. The segment 0 is the visible text.
/// Each other segment is the payload of one OSC sequence.
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
        edit_stream(&events, &stream, redaction, &mut edits)?;
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
    let segments = build_segments(&joined, &runs(&joined), &starts, stream)?;
    for segment in &segments {
        edit_segment(segment, events, redaction, edits)?;
    }
    Ok(())
}

/// The escape byte.
const ESC: u8 = 0x1B;

/// The bell byte. It ends an OSC sequence.
const BEL: u8 = 0x07;

/// The segment of the visible text.
const VISIBLE: usize = 0;

/// Cut the joined string into runs of text. Escape sequences are not text.
/// The payload of an OSC sequence is a run of its own segment.
fn runs(joined: &str) -> Vec<Run> {
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
        let (payload, end) = escape_sequence(bytes, at);
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
/// The answer is the range of the payload when the sequence is an OSC
/// sequence, and the end of the sequence. The end is always above `at`.
fn escape_sequence(bytes: &[u8], at: usize) -> (Option<Range<usize>>, usize) {
    let length = bytes.len();
    match bytes.get(at + 1) {
        Some(b'[') => {
            let mut end = at + 2;
            end = skip_while(bytes, end, |byte| (0x20..=0x3F).contains(&byte));
            if bytes.get(end).is_some_and(|byte| (0x40..=0x7E).contains(byte)) {
                end += 1;
            }
            (None, end)
        }
        Some(b']') => {
            let start = at + 2;
            let mut end = start;
            while end < length {
                match (bytes.get(end), bytes.get(end + 1)) {
                    (Some(&BEL), _) => return (Some(start..end), end + 1),
                    (Some(&ESC), Some(b'\\')) => return (Some(start..end), end + 2),
                    _ => end += 1,
                }
            }
            (Some(start..length), length)
        }
        Some(b'P' | b'X' | b'^' | b'_') => {
            let mut end = at + 2;
            while end < length {
                if bytes.get(end) == Some(&ESC) && bytes.get(end + 1) == Some(&b'\\') {
                    return (None, end + 2);
                }
                end += 1;
            }
            (None, length)
        }
        Some(_) => {
            let mut end = skip_while(bytes, at + 1, |byte| (0x20..=0x2F).contains(&byte));
            if bytes.get(end).is_some_and(|byte| (0x30..=0x7E).contains(byte)) {
                end += 1;
            }
            (None, end)
        }
        None => (None, length),
    }
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
            let position = starts.partition_point(|&start| start <= at).saturating_sub(1);
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
