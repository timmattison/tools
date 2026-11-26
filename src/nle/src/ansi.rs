//! A reader of the escape sequences in the output of a terminal.
//!
//! The reader takes one character at a time and keeps its state between
//! calls. Thus a sequence can start in one event of a recording and end in a
//! later one.

/// A complete escape sequence that a filter can rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Sequence {
    /// A control sequence: `ESC [` or the 8-bit CSI, the parameter
    /// characters, the intermediate characters, and a final character.
    Csi {
        /// The parameter characters (`0` to `?`), for example `?25`.
        params: String,
        /// The intermediate characters (space to `/`), for example the space
        /// of `CSI 5 SP q`.
        intermediates: String,
        /// The final character (`@` to `~`), for example `h`.
        final_char: char,
        /// The C0 controls inside the sequence. A terminal runs each one
        /// where it stands, so a rewrite of the sequence must keep them.
        controls: String,
    },
    /// An operating system command: `ESC ]` or the 8-bit OSC, its text, and
    /// its terminator.
    Osc {
        /// The text between the introducer and the terminator, for example
        /// `12;#ff0000`.
        text: String,
        /// The terminator: BEL, `ESC \`, or the 8-bit ST.
        terminator: String,
    },
    /// `ESC` and one final character, for example `ESC c`.
    Escape(char),
}

/// What one character does in the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// The character is not a part of a sequence.
    Text,
    /// The character starts a sequence or continues the open one.
    Pending,
    /// The character completes a sequence that a filter can rewrite.
    Done(Sequence),
    /// The character completes a sequence that no filter rewrites. The
    /// sequence stays as it is.
    Ignored,
    /// The character (CAN or SUB) stops the open sequence. The sequence stays
    /// as it is, and the character is text.
    Aborted,
    /// The character (ESC) stops the open sequence and starts a new one. The
    /// old sequence stays as it is.
    Restarted,
    /// The ESC before this character ends the open OSC or string, because
    /// the character is not the backslash of an ST. The old sequence stays as
    /// it is. That ESC starts a new sequence, and the inner step is the step
    /// of this character in the new sequence.
    EscapeBreak(Box<Step>),
}

/// The reader. It starts outside a sequence.
#[derive(Debug, Default)]
pub(crate) struct Scanner {}

impl Scanner {
    /// Make a reader that starts outside a sequence.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Read the next character of the stream.
    pub(crate) fn feed(&mut self, c: char) -> Step {
        let _ = c;
        Step::Text
    }

    /// True when a sequence is open: it started and is not complete.
    pub(crate) fn is_pending(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests;
