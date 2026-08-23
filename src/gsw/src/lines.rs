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
    /// across two reads come back whole.
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        let _ = chunk;
        Vec::new()
    }

    /// The unterminated remainder, once the stream has ended.
    ///
    /// A process that exits without a final newline still said something, and
    /// dropping it would lose the last line of every hook that ends that way.
    /// `None` when the stream ended on a terminator, so a caller never paints a
    /// blank row for output that was already complete.
    pub(crate) fn finish(&mut self) -> Option<String> {
        None
    }
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
    fn other_control_characters_are_removed() {
        // A bell would ring once per painted frame, and a backspace would eat
        // the column to its left.
        assert_eq!(split_all(b"be\x07ep\x08\n"), vec!["beep"]);
    }
}
