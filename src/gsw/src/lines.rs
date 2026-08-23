//! Cutting a child process's byte stream into lines gsw can paint.
//!
//! A push writes to two pipes, and what arrives on them is not a list of
//! lines. It is byte chunks, cut wherever the writer's buffer happened to
//! flush: mid-line, mid-word, and mid-character. Whatever reassembles them has
//! to hold three separate rules at once, which is why it is a type here rather
//! than a loop at each of the two call sites.
//!
//! **A chunk boundary is not a character boundary.** A pipe read returns bytes,
//! and a three-byte `日` can arrive two bytes in one chunk and one in the next.
//! Decoding each chunk on arrival turns that into two replacement characters
//! that no later step can undo, so this buffers *bytes* and decodes only when a
//! terminator says the line is whole.
//!
//! **A carriage return is a redraw, not a line.** A progress bar rewrites one
//! row by returning to column zero and printing over itself, so
//! `10%\r60%\r100%` is one line reading `100%`. gsw used to delete every `\r`
//! from a push's output, which pasted the three states into `10%60%100%` — a
//! row that says nothing and costs the same space as one that does.
//!
//! **Not every byte is safe to paint.** gsw's standing rule is that nothing it
//! paints wraps or scrolls the pane it was measured to fill, and the frame is
//! measured in display columns. A tab is one character and up to eight columns,
//! so a hook that prints a table would push the frame's bottom row off the
//! screen. An ANSI escape is several characters and zero columns, and left in
//! it would repaint gsw's own frame in the hook's colors. Both are resolved
//! here, at the one place a line becomes text.
//!
//! **A row erased is not a row said.** A hook that clears the row it is about
//! to reuse sends bytes that draw nothing, and once the escapes are gone the
//! empty string is all that is left of them. The window is six rows tall, so
//! passing that on would spend a row on output no reader could ever have seen.
//! A line that sanitizes away is therefore dropped — while a line that was
//! blank before it got here, from an `echo ""`, keeps its row.

use unicode_width::UnicodeWidthChar;

/// Columns between tab stops. Eight is what terminals do, and matching it is
/// what keeps a hook's aligned output aligned once the tabs are gone.
const TAB_WIDTH: usize = 8;

/// Reassembles the byte chunks of one stream into painted lines.
///
/// One per stream: a push reads stdout and stderr separately, and a single
/// splitter across both would join half a line from one with half a line from
/// the other.
pub(crate) struct LineSplitter {
    /// Bytes of the line being built, still waiting for a terminator. Bytes
    /// rather than a `String` because a character can span a chunk boundary.
    partial: Vec<u8>,
    /// Whether the last byte seen was a carriage return whose meaning is not
    /// settled yet. `\r\n` is one terminator and a bare `\r` is a redraw, and
    /// which one it is depends on the *next* byte — which can be in the next
    /// chunk, or never arrive at all.
    pending_cr: bool,
}

impl LineSplitter {
    /// A splitter with nothing buffered.
    pub(crate) fn new() -> Self {
        Self {
            partial: Vec::new(),
            pending_cr: false,
        }
    }

    /// Feed one chunk of bytes, and take back every line it completed.
    ///
    /// Returns an empty vector for a chunk that carried no terminator: those
    /// bytes are held until one arrives, which is what makes a line split
    /// across two reads come back whole. A terminator whose line sanitizes
    /// away completes no line either, by the rule [`LineSplitter::take_line`]
    /// carries, so what comes back is not one line per terminator.
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in chunk {
            if self.pending_cr {
                self.pending_cr = false;
                if byte == b'\n' {
                    // `\r\n`: one terminator wearing two bytes. Ending the line
                    // here is also what keeps the `\r` out of it, which matters
                    // because a stray one would send the cursor to column zero
                    // in the middle of a painted frame.
                    lines.extend(self.take_line());
                    continue;
                }
                // A bare `\r`: the writer went back to column zero and is
                // about to print over what it drew, so what it drew never
                // becomes a line. The byte that told us falls through to the
                // match below and starts the replacement.
                self.partial.clear();
            }
            match byte {
                // Undecided until the next byte arrives, which can be in the
                // next chunk or never.
                b'\r' => self.pending_cr = true,
                b'\n' => lines.extend(self.take_line()),
                _ => self.partial.push(byte),
            }
        }
        lines
    }

    /// The unterminated remainder, once the stream has ended.
    ///
    /// A process that exits without a final newline still said something, and
    /// dropping it would lose the last line of every hook that ends that way.
    /// `None` when the stream ended on a terminator, so a caller never paints a
    /// blank row for output that was already complete — and `None` again when
    /// the remainder sanitizes away, which is [`LineSplitter::take_line`]'s
    /// rule and applies to every line rather than only to this one.
    pub(crate) fn finish(&mut self) -> Option<String> {
        // The one thing this rules out that `take_line` does not: an empty
        // buffer is a stream that ended on a terminator, not a blank line
        // somebody printed, and it owes the caller no row.
        if self.partial.is_empty() {
            return None;
        }
        // `pending_cr` is deliberately not consulted. A bare `\r` moves the
        // cursor without erasing, so a stream that stops right after one
        // leaves its text on the screen — nothing came along to draw over it.
        self.take_line()
    }

    /// Decode and sanitize the buffered bytes, and empty the buffer.
    ///
    /// The one place a line becomes text, which is what lets the byte rules
    /// above and the column rules below be stated separately and still meet
    /// exactly once.
    ///
    /// `None` for buffered bytes that sanitize away — a lone color reset, a
    /// bell, an erase of the row the hook is about to reuse. Those bytes were
    /// never a line, and reporting the empty string they leave behind would
    /// cost the caller a row of a window six rows tall. An *empty* buffer is a
    /// different thing and comes back as `Some("")`: a hook that ran
    /// `echo ""` said something, and it said it blank.
    fn take_line(&mut self) -> Option<String> {
        let raw = String::from_utf8_lossy(&self.partial).into_owned();
        self.partial.clear();
        let line = sanitize(&raw);
        if line.is_empty() && !raw.is_empty() {
            None
        } else {
            Some(line)
        }
    }
}

/// The byte that starts every escape sequence.
const ESC: char = '\u{1b}';

/// Where [`sanitize`] is in an escape sequence.
///
/// A state machine rather than a search for a closing byte, because a sequence
/// ends in one of three ways and the shortest of them is not a prefix of the
/// others: a two-character escape ends at its second character, a CSI ends at
/// the first byte in its final range, and a control string runs to a bell or to
/// a two-character string terminator. A rule written for one leaks the others'
/// payloads into the frame as text.
enum Scan {
    /// Ordinary text, and the only state that writes anything out.
    Text,
    /// An `ESC` was seen and its second character decides what follows.
    Escape,
    /// Inside `ESC [ … final`, the color and cursor sequences.
    Csi,
    /// Inside a control string: `ESC ]` (OSC, a title set), `ESC P` (DCS, a
    /// sixel image or a tmux passthrough), `ESC X` (SOS), `ESC ^` (PM) or
    /// `ESC _` (APC). Named for the ECMA-48 category rather than for OSC,
    /// which is only the most common of the five, because all five carry a
    /// free-form payload to the same string terminator and so are one state.
    ControlString,
    /// Inside a control string, on an `ESC` that can be half of its terminator.
    ControlStringEscape,
}

/// Make one decoded line safe to paint in a frame measured in display columns.
///
/// Two removals and one substitution, all for the same reason: gsw lays the
/// frame out to fill the pane exactly, so a row that measures shorter than it
/// draws pushes the frame's bottom row off the screen, and a row that carries
/// escape sequences repaints gsw's own colors in the hook's.
///
/// - **Tabs become spaces**, out to the next [`TAB_WIDTH`] stop. One character
///   and up to eight columns is the widest gap between what a row measures and
///   what it draws, and a hook that prints a table prints one per row.
/// - **Escape sequences go**, every family: the two-character escapes, the CSI
///   sequences, and the control strings. Nothing gsw shows from a child process
///   is worth letting that child address the terminal directly.
/// - **Remaining control characters go.** A bell would ring on every repaint,
///   and a backspace would eat the column to its left.
fn sanitize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    // Display columns written so far, which is what a tab stop is counted in.
    // Not characters: a full-width `日` is one character and two columns, and
    // counting it as one would put the stop in the wrong place.
    let mut column = 0_usize;
    let mut scan = Scan::Text;

    for ch in raw.chars() {
        scan = match scan {
            Scan::Text => match ch {
                ESC => Scan::Escape,
                '\t' => {
                    let pad = TAB_WIDTH - (column % TAB_WIDTH);
                    for _ in 0..pad {
                        out.push(' ');
                    }
                    column += pad;
                    Scan::Text
                }
                // Covers C0, DEL, and C1. `\t` is one of them and is handled
                // above; `\n` and `\r` are terminators and never arrive here.
                _ if ch.is_control() => Scan::Text,
                _ => {
                    out.push(ch);
                    // Zero for a combining mark, which is the right answer: it
                    // draws on top of the character before it.
                    column += ch.width().unwrap_or(0);
                    Scan::Text
                }
            },
            Scan::Escape => match ch {
                '[' => Scan::Csi,
                // The five introducers that open a control string. Read as
                // two-character escapes they would drop the introducer and
                // then print the payload, which is how a sixel image or a
                // tmux passthrough arrives as a row of garbage.
                ']' | 'P' | 'X' | '^' | '_' => Scan::ControlString,
                // A two-character escape. Both characters are already dropped.
                _ => Scan::Text,
            },
            // A CSI ends at its first byte in `@` through `~`; everything
            // before that is parameters and is dropped with it.
            Scan::Csi => {
                if ('\u{40}'..='\u{7e}').contains(&ch) {
                    Scan::Text
                } else {
                    Scan::Csi
                }
            }
            // A control string ends at a bell or at `ESC \`, whichever the
            // writer chose. Its payload is free-form, so nothing between the
            // introducer and the terminator can be read as text.
            Scan::ControlString => match ch {
                '\u{07}' => Scan::Text,
                ESC => Scan::ControlStringEscape,
                _ => Scan::ControlString,
            },
            Scan::ControlStringEscape => match ch {
                '\\' => Scan::Text,
                // Not a terminator after all, so the string runs on.
                _ => Scan::ControlString,
            },
        };
    }

    out
}

#[cfg(test)]
mod tests {
    use super::LineSplitter;

    /// Feed one chunk and take the lines it completed.
    fn feed(splitter: &mut LineSplitter, chunk: &str) -> Vec<String> {
        splitter.feed(chunk.as_bytes())
    }

    /// Everything one whole byte string splits into, remainder included.
    fn split_all(input: &[u8]) -> Vec<String> {
        let mut splitter = LineSplitter::new();
        let mut lines = splitter.feed(input);
        lines.extend(splitter.finish());
        lines
    }

    #[test]
    fn a_newline_ends_a_line() {
        assert_eq!(split_all(b"first\nsecond\n"), vec!["first", "second"]);
    }

    #[test]
    fn a_line_split_across_chunks_is_joined() {
        // The defect this type exists for: a pipe read returns whatever the
        // writer flushed, and half a line is the common case.
        let mut splitter = LineSplitter::new();
        assert!(
            feed(&mut splitter, "half a ").is_empty(),
            "a chunk with no terminator completes no line",
        );
        assert_eq!(feed(&mut splitter, "line\n"), vec!["half a line"]);
    }

    #[test]
    fn crlf_is_one_terminator() {
        // A stray `\r` left on the end would send the cursor to column zero
        // mid-frame and scramble the rows under it.
        assert_eq!(split_all(b"windows\r\nstyle\r\n"), vec!["windows", "style"]);
    }

    #[test]
    fn a_bare_carriage_return_discards_the_redrawn_line() {
        // One progress bar, three states, one row. The newest state is the one
        // the terminal would be showing.
        assert_eq!(
            split_all(b"progress 10%\rprogress 60%\rprogress 100%\n"),
            vec!["progress 100%"],
        );
    }

    #[test]
    fn a_carriage_return_at_a_chunk_boundary_still_pairs_with_a_newline() {
        // Whether a `\r` is a terminator or a redraw depends on the next byte,
        // and the next byte can be in the next chunk. Deciding at the boundary
        // would cut `first` short and hand `second` a leading blank.
        let mut splitter = LineSplitter::new();
        assert!(feed(&mut splitter, "first\r").is_empty());
        assert_eq!(feed(&mut splitter, "\nsecond\n"), vec!["first", "second"]);
    }

    #[test]
    fn a_carriage_return_at_the_end_of_the_stream_keeps_what_was_drawn() {
        // A bare `\r` moves the cursor without erasing, so a stream that stops
        // there leaves its text on screen. Nothing overwrote it.
        assert_eq!(split_all(b"done\r"), vec!["done"]);
    }

    #[test]
    fn a_multibyte_character_split_across_chunks_is_not_corrupted() {
        // `日` is three bytes. Decoding per chunk turns a boundary inside it
        // into replacement characters, and no later step can put it back.
        let text = "日本語";
        let bytes = text.as_bytes();
        let mut splitter = LineSplitter::new();
        assert!(splitter.feed(&bytes[..4]).is_empty());
        let mut lines = splitter.feed(&bytes[4..]);
        lines.extend(splitter.finish());
        assert_eq!(lines, vec![text]);
    }

    #[test]
    fn finish_emits_the_unterminated_remainder() {
        // A hook that exits without a trailing newline still said something.
        assert_eq!(split_all(b"no newline here"), vec!["no newline here"]);
    }

    #[test]
    fn finish_emits_nothing_when_the_stream_ended_on_a_terminator() {
        // Otherwise every well-behaved hook costs the window a blank row.
        let mut splitter = LineSplitter::new();
        assert_eq!(feed(&mut splitter, "complete\n"), vec!["complete"]);
        assert_eq!(splitter.finish(), None);
    }

    #[test]
    fn a_line_that_sanitizes_away_is_not_a_line() {
        // A hook that erases the row it is about to reuse said nothing a
        // reader could see, and the window is six rows tall. Left in, one
        // erase before each newline spends the window on blanks.
        assert!(
            split_all(b"\x1b[2K\n").is_empty(),
            "a line that is only escape sequences costs no row",
        );
        // The distinction the rule turns on: `echo \"\"` said something, and
        // it said it blank. Dropping every empty line would swallow that too.
        assert_eq!(split_all(b"a\n\nb\n"), vec!["a", "", "b"]);
    }

    #[test]
    fn a_tab_expands_to_the_next_tab_stop() {
        // One character, up to eight columns. Left in, the frame is measured
        // eight columns short and its bottom row goes off the screen.
        assert_eq!(split_all(b"a\tb\n"), vec!["a       b"]);
        assert_eq!(split_all(b"12345678\tx\n"), vec!["12345678        x"]);
    }

    #[test]
    fn an_ansi_escape_sequence_is_removed() {
        // A tool that forces color reaches gsw's frame through the pipe.
        assert_eq!(
            split_all(b"\x1b[31merror\x1b[0m: it failed\n"),
            vec!["error: it failed"],
        );
    }

    #[test]
    fn an_operating_system_command_sequence_is_removed() {
        // Terminal title sets end at BEL rather than at a CSI final byte, so
        // the CSI rule alone would leak the whole payload as text.
        assert_eq!(
            split_all(b"\x1b]0;building\x07compiling\n"),
            vec!["compiling"],
        );
    }

    #[test]
    fn a_control_string_sequence_is_removed() {
        // Four more introducers open a run of payload that ends at a string
        // terminator rather than at a CSI final byte: DCS (`ESC P`, which
        // carries sixel images and tmux passthrough), SOS (`ESC X`), PM
        // (`ESC ^`) and APC (`ESC _`). Read as two-character escapes, each
        // drops its own introducer and then prints its payload — a whole
        // sixel image arrives as a row of garbage in a window six rows tall.
        for introducer in ['P', 'X', '^', '_'] {
            // Both terminators, because a rule written for one and not the
            // other leaks every payload that happens to end the other way.
            for terminator in ["\u{07}", "\u{1b}\\"] {
                let input = format!("\u{1b}{introducer}payload{terminator}said");
                assert_eq!(
                    split_all(input.as_bytes()),
                    vec!["said"],
                    "ESC {introducer} runs to {terminator:?} and takes its payload with it",
                );
            }
        }
    }

    #[test]
    fn other_control_characters_are_removed() {
        // A bell would ring once per painted frame, and a backspace would eat
        // the column to its left.
        assert_eq!(split_all(b"be\x07ep\x08\n"), vec!["beep"]);
    }
}
