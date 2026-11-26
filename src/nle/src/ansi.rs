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

const ESC: char = '\x1b';
const BEL: char = '\x07';
const CAN: char = '\x18';
const SUB: char = '\x1a';
const C1_CSI: char = '\u{9b}';
const C1_OSC: char = '\u{9d}';
const C1_ST: char = '\u{9c}';
const C1_DCS: char = '\u{90}';
const C1_SOS: char = '\u{98}';
const C1_PM: char = '\u{9e}';
const C1_APC: char = '\u{9f}';
/// The terminator `ESC \` of an OSC.
const ST: &str = "\x1b\\";

/// A C0 control other than ESC, CAN, and SUB, or DEL. Inside a sequence, a
/// terminal runs such a control (or drops DEL) and stays in the sequence.
fn is_control(c: char) -> bool {
    matches!(c, '\x00'..='\x1f' | '\x7f')
}

/// Where the reader is in the stream.
#[derive(Debug, Default)]
enum State {
    /// Outside a sequence.
    #[default]
    Ground,
    /// After an ESC.
    Escape,
    /// After an ESC and one or more intermediate characters.
    EscapeIntermediate,
    /// Inside a control sequence.
    Csi {
        params: String,
        intermediates: String,
        controls: String,
    },
    /// Inside a control sequence that a terminal ignores, up to its final
    /// character.
    CsiIgnore,
    /// Inside an OSC.
    Osc { text: String },
    /// After an ESC inside an OSC.
    OscEscape { text: String },
    /// Inside a DCS, SOS, PM, or APC string. Its text is not read.
    String,
    /// After an ESC inside a string.
    StringEscape,
}

/// The reader. It starts outside a sequence.
///
/// It obeys the parser of a VT500 terminal for the forms that it knows: an
/// ESC anywhere stops the open sequence, CAN and SUB stop it, and a C0
/// control inside a control sequence does not stop it. An OSC ends at BEL,
/// at `ESC \`, or at the 8-bit ST. A DCS, SOS, PM, or APC string ends at
/// `ESC \` or at the 8-bit ST.
#[derive(Debug, Default)]
pub(crate) struct Scanner {
    state: State,
}

impl Scanner {
    /// Make a reader that starts outside a sequence.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Read the next character of the stream.
    pub(crate) fn feed(&mut self, c: char) -> Step {
        let (state, step) = Self::next(std::mem::take(&mut self.state), c);
        self.state = state;
        step
    }

    /// True when a sequence is open: it started and is not complete.
    pub(crate) fn is_pending(&self) -> bool {
        !matches!(self.state, State::Ground)
    }

    /// The state after `c`, and the step of `c`.
    fn next(state: State, c: char) -> (State, Step) {
        match state {
            State::Ground => match c {
                ESC => (State::Escape, Step::Pending),
                C1_CSI => (State::csi(), Step::Pending),
                C1_OSC => (State::osc(), Step::Pending),
                C1_DCS | C1_SOS | C1_PM | C1_APC => (State::String, Step::Pending),
                _ => (State::Ground, Step::Text),
            },
            State::Escape => Self::after_escape(c),
            State::EscapeIntermediate => match c {
                ESC => (State::Escape, Step::Restarted),
                CAN | SUB => (State::Ground, Step::Aborted),
                ' '..='/' => (State::EscapeIntermediate, Step::Pending),
                '0'..='~' => (State::Ground, Step::Ignored),
                c if is_control(c) => (State::EscapeIntermediate, Step::Pending),
                _ => (State::Ground, Step::Ignored),
            },
            State::Csi {
                mut params,
                mut intermediates,
                mut controls,
            } => match c {
                ESC => (State::Escape, Step::Restarted),
                CAN | SUB => (State::Ground, Step::Aborted),
                '0'..='?' if intermediates.is_empty() => {
                    params.push(c);
                    (
                        State::Csi {
                            params,
                            intermediates,
                            controls,
                        },
                        Step::Pending,
                    )
                }
                ' '..='/' => {
                    intermediates.push(c);
                    (
                        State::Csi {
                            params,
                            intermediates,
                            controls,
                        },
                        Step::Pending,
                    )
                }
                '@'..='~' => (
                    State::Ground,
                    Step::Done(Sequence::Csi {
                        params,
                        intermediates,
                        final_char: c,
                        controls,
                    }),
                ),
                c if is_control(c) => {
                    controls.push(c);
                    (
                        State::Csi {
                            params,
                            intermediates,
                            controls,
                        },
                        Step::Pending,
                    )
                }
                // A parameter after an intermediate, or a character that is
                // not ASCII.
                _ => (State::CsiIgnore, Step::Pending),
            },
            State::CsiIgnore => match c {
                ESC => (State::Escape, Step::Restarted),
                CAN | SUB => (State::Ground, Step::Aborted),
                '@'..='~' => (State::Ground, Step::Ignored),
                _ => (State::CsiIgnore, Step::Pending),
            },
            State::Osc { mut text } => match c {
                BEL | C1_ST => (
                    State::Ground,
                    Step::Done(Sequence::Osc {
                        text,
                        terminator: c.to_string(),
                    }),
                ),
                ESC => (State::OscEscape { text }, Step::Pending),
                CAN | SUB => (State::Ground, Step::Aborted),
                _ => {
                    text.push(c);
                    (State::Osc { text }, Step::Pending)
                }
            },
            State::OscEscape { text } => match c {
                '\\' => (
                    State::Ground,
                    Step::Done(Sequence::Osc {
                        text,
                        terminator: ST.to_string(),
                    }),
                ),
                _ => Self::break_with_escape(c),
            },
            State::String => match c {
                C1_ST => (State::Ground, Step::Ignored),
                ESC => (State::StringEscape, Step::Pending),
                CAN | SUB => (State::Ground, Step::Aborted),
                _ => (State::String, Step::Pending),
            },
            State::StringEscape => match c {
                '\\' => (State::Ground, Step::Ignored),
                _ => Self::break_with_escape(c),
            },
        }
    }

    /// The state and the step of `c` after an ESC that starts a sequence.
    fn after_escape(c: char) -> (State, Step) {
        match c {
            '[' => (State::csi(), Step::Pending),
            ']' => (State::osc(), Step::Pending),
            'P' | 'X' | '^' | '_' => (State::String, Step::Pending),
            ESC => (State::Escape, Step::Restarted),
            CAN | SUB => (State::Ground, Step::Aborted),
            ' '..='/' => (State::EscapeIntermediate, Step::Pending),
            '0'..='~' => (State::Ground, Step::Done(Sequence::Escape(c))),
            c if is_control(c) => (State::Escape, Step::Pending),
            _ => (State::Ground, Step::Ignored),
        }
    }

    /// The ESC before `c` ends an OSC or a string, and starts a new sequence.
    fn break_with_escape(c: char) -> (State, Step) {
        let (state, inner) = Self::after_escape(c);
        (state, Step::EscapeBreak(Box::new(inner)))
    }
}

impl State {
    fn csi() -> Self {
        State::Csi {
            params: String::new(),
            intermediates: String::new(),
            controls: String::new(),
        }
    }

    fn osc() -> Self {
        State::Osc {
            text: String::new(),
        }
    }
}

#[cfg(test)]
mod tests;
