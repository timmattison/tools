//! The splice of rewritten escape sequences, which the filters that rewrite
//! the output events share.
//!
//! A filter reads the data of the output events with one `Scanner`. It
//! decides what each sequence becomes through a `SequenceEditor`. A sequence
//! can start in one event and end in a later one, and the `Splicer` puts the
//! replacement where the rules of its doc comment say.

use crate::ansi::{Scanner, Sequence, Step};
use crate::types::{Event, EventType};

/// The terminator of each OSC that a filter writes.
pub(super) const ST: &str = "\x1b\\";

/// What a filter does with the escape sequences of its region.
pub(super) trait SequenceEditor {
    /// The text that replaces `sequence`, or `None` when the sequence stays
    /// as it is.
    fn rewrite(&mut self, sequence: &Sequence) -> Option<String>;

    /// The sequences that bring the answer to the state that the filter
    /// wants. The splicer puts them where no sequence is open.
    fn settle(&mut self) -> String;
}

/// The answer of a filter for the events of one region.
///
/// It reads the output events with one `Scanner`. Each sequence that the
/// editor rewrites goes into the event where the sequence completes. Each
/// other byte stays in its event. A sequence that is open at the end of the
/// region gets the tail text before it, because nothing could go after it.
/// When the start of a dropped sequence had ended an earlier OSC or string,
/// an ST goes where the dropped sequence started, so the rest of the stream
/// stays outside that OSC or string.
pub(super) struct Splicer {
    scanner: Scanner,
    /// The answer.
    out: Vec<Event>,
    /// Where each piece of the open sequence starts: the index of its event
    /// in `out`, and the byte offset in the data of that event. Each piece
    /// runs to the end of that data.
    pieces: Vec<(usize, usize)>,
    /// Where the last character went, as a piece.
    last: (usize, usize),
    /// True when the start of the open sequence ended an earlier sequence of
    /// the stream, through a restart or a break of an OSC or a string.
    ends_open: bool,
}

impl Splicer {
    /// An empty answer, and a scanner at the start of the stream.
    pub(super) fn new() -> Self {
        Self {
            scanner: Scanner::new(),
            out: Vec::new(),
            pieces: Vec::new(),
            last: (0, 0),
            ends_open: false,
        }
    }

    /// Append a copy of `event`, an input event, to the answer.
    pub(super) fn pass(&mut self, event: &Event) {
        self.out.push(event.clone());
    }

    /// Append an empty output event at `time` to the answer, and return its
    /// index.
    pub(super) fn open_output(&mut self, time: f64) -> usize {
        let index = self.out.len();
        self.out.push(output_event(time, String::new()));
        index
    }

    /// Read each character of `data` into the output event at `index`.
    pub(super) fn read(&mut self, index: usize, data: &str, editor: &mut impl SequenceEditor) {
        for c in data.chars() {
            let step = self.scanner.feed(c);
            self.take(step, c, index, editor);
        }
    }

    /// Bring the end of the event at `index` to the state that the editor
    /// wants. While a sequence is open, nothing goes out, because a sequence
    /// inside it would break it. The change then waits for the end of that
    /// sequence, or for the end of the region.
    pub(super) fn settle(&mut self, index: usize, editor: &mut impl SequenceEditor) {
        if self.scanner.is_pending() {
            return;
        }
        let wanted = editor.settle();
        if let Some(data) = self.data(index) {
            data.push_str(&wanted);
        }
    }

    /// The answer. `tail` gets true when a sequence is open at the end of
    /// the region, and it gives the text for the end of the region.
    ///
    /// When a sequence is open, the text goes into the event of its first
    /// piece, at the offset of that piece. Else, a text that is not empty
    /// goes into a new output event at `end_time`. Then a `start` that is
    /// not empty goes into a new output event at time 0, first in the
    /// answer.
    pub(super) fn finish(
        mut self,
        start: String,
        tail: impl FnOnce(bool) -> String,
        end_time: f64,
    ) -> Vec<Event> {
        if let Some(&(index, offset)) = self.pieces.first() {
            // A sequence is open at the end, so nothing could go after it.
            // Put the rest before it.
            let rest = tail(true);
            if let Some(event) = self.out.get_mut(index) {
                event.data.insert_str(offset, &rest);
            }
        } else {
            let rest = tail(false);
            if !rest.is_empty() {
                self.out.push(output_event(end_time, rest));
            }
        }
        if !start.is_empty() {
            self.out.insert(0, output_event(0.0, start));
        }
        self.out
    }

    /// Put `c` into the event at `index` of `out`, by its `step`.
    fn take(&mut self, step: Step, c: char, index: usize, editor: &mut impl SequenceEditor) {
        match step {
            Step::Text => self.push(index, c),
            Step::Pending => {
                self.mark(index);
                self.push(index, c);
            }
            Step::Done(sequence) => {
                self.mark(index);
                self.push(index, c);
                self.complete(&sequence, index, editor);
                self.ends_open = false;
                self.settle(index, editor);
            }
            Step::Ignored => {
                self.push(index, c);
                self.ends_open = false;
                self.pieces.clear();
                self.settle(index, editor);
            }
            Step::Aborted => {
                self.ends_open = false;
                self.pieces.clear();
                self.push(index, c);
                self.settle(index, editor);
            }
            Step::Restarted => {
                self.ends_open = true;
                self.pieces.clear();
                self.mark(index);
                self.push(index, c);
            }
            Step::EscapeBreak(inner) => {
                // The ESC before `c` starts the new sequence.
                self.ends_open = true;
                self.pieces.clear();
                self.pieces.push(self.last);
                self.take(*inner, c, index, editor);
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
    /// the editor asks for it.
    fn complete(&mut self, sequence: &Sequence, index: usize, editor: &mut impl SequenceEditor) {
        let pieces = std::mem::take(&mut self.pieces);
        if let Some(replacement) = editor.rewrite(sequence) {
            for &(piece, offset) in &pieces {
                if let Some(data) = self.data(piece) {
                    data.truncate(offset);
                }
            }
            // The start of the dropped sequence ended an earlier one. An ST
            // where that start was ends an open OSC or string, and the
            // rest of the stream stays outside it.
            if self.ends_open {
                if let Some(&(first, _)) = pieces.first() {
                    if let Some(data) = self.data(first) {
                        data.push_str(ST);
                    }
                }
            }
            if let Some(data) = self.data(index) {
                data.push_str(&replacement);
            }
        }
    }
}

/// An output event at `time` with `data`.
pub(super) fn output_event(time: f64, data: String) -> Event {
    Event {
        time,
        event_type: EventType::Output,
        data,
    }
}

#[cfg(test)]
mod tests;
