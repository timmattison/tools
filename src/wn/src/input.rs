//! Where the chain comes from.
//!
//! A chain is written once and read many times. It is typed into a plan, then
//! it is copied out of that plan and into a terminal. The copy is the step this
//! module removes: the chain is already on the clipboard when the reader asks
//! what is next, so `wn` with no argument at all reads it from there.
//!
//! # The order of the inputs
//!
//! An argument first, then standard input, then the clipboard. The order is the
//! order of how loudly each input was asked for. An argument was typed on
//! purpose. A pipe was built on purpose. The clipboard was neither, so it
//! answers only when nothing else did.
//!
//! An EMPTY standard input falls through to the clipboard, and this is the one
//! rule that is not obvious. A parent that redirects standard input from
//! `/dev/null` did not ask for an empty chain — it closed a mouth it does not
//! use. A run under such a parent must read the clipboard, or `wn` stops with
//! "no chain given" for a reader who has a chain on the clipboard and no way to
//! know why it was not read.
//!
//! # Why every input is a function
//!
//! [`Sources`] holds a reader for each input rather than the text of each
//! input. The clipboard is one shared resource of the whole machine, and a
//! read of it is a system call under a lock that another program can hold. An
//! input that answered first must therefore be the only input that was read,
//! and a function is what makes that true rather than something a caller
//! remembers. The tests hold it: a reader that must not run panics.
//!
//! It is also what lets the tests keep their hands off the real clipboard. A
//! test that reads it is a test that races the person at the keyboard, and a
//! test that writes it destroys what that person copied.

use std::fmt;

use thiserror::Error;

/// The variable that turns the clipboard fallback off. Any value with a
/// character in it turns it off.
pub const NO_CLIPBOARD_ENV: &str = "WN_NO_CLIPBOARD";

/// The instruction every error that found no chain ends with.
///
/// One constant, because three messages end with it. A reader who sees the
/// instruction twice in two different wordings reads them as two different
/// instructions, and the wording drifts the first time one of the three
/// messages is edited on its own.
const PASS_IT_AS_AN_ARGUMENT: &str = "Pass it as an argument, in quotes: wn \"#277 → #278\"";

/// What a read of the clipboard gave back.
///
/// `Ok(None)` is a clipboard with no text in it. An empty clipboard and a
/// clipboard that holds an image are the same thing to a tool that wants a
/// chain, so both arrive here.
pub type ClipboardRead = Result<Option<String>, ClipboardUnavailable>;

/// The clipboard could not be opened at all.
///
/// A newtype over the cause rather than the cause itself, because the cause is
/// a different type on every platform and a headless machine has no clipboard
/// to name a type of. The message is kept as text for the same reason: the
/// reader of it wants to know why the read failed, not which crate said so.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct ClipboardUnavailable(String);

impl ClipboardUnavailable {
    /// The failure `cause` names.
    ///
    /// The space around the cause is dropped, and so is a period at the end of
    /// it. A cause is a clause of a longer sentence here — the message puts it
    /// in parentheses and then goes on with the instruction — and `arboard`
    /// writes a whole sentence with a period on it. Keeping that period gives
    /// the reader `.).` in the middle of one line. A cause that is nothing but
    /// a period is kept as it is, because an error that names no cause at all
    /// reads as a bug in the tool rather than as a state of the machine.
    #[must_use]
    pub fn new(cause: &impl fmt::Display) -> Self {
        let written = cause.to_string();
        let clause = written.trim();
        let shortened = clause.strip_suffix('.').unwrap_or(clause);
        Self(
            if shortened.is_empty() {
                clause
            } else {
                shortened
            }
            .to_string(),
        )
    }
}

/// Where the chain came from.
///
/// Private, because a caller acts on the chain and never on the input it came
/// out of. The one thing the input changes is how a bad chain is reported, and
/// [`Chain::blame`] is where that happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The command line.
    Argument,
    /// Standard input.
    Stdin,
    /// The system clipboard.
    Clipboard,
}

/// The chain, and the input it came out of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    /// The text of the chain, as the input wrote it.
    text: String,
    /// The input that wrote it.
    source: Source,
}

impl Chain {
    /// The chain that `source` gave as `text`.
    fn new(text: String, source: Source) -> Self {
        Self { text, source }
    }

    /// The chain, as the input wrote it.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The reason the text could not be read, with an invisible input named.
    ///
    /// Text that came from an argument or from a pipe is text the reader can
    /// see, so the reason stands on its own. Text that came from the
    /// clipboard is not, and a reader who typed no argument and reads
    /// `"an" is not an issue number` has no way to know where the word came
    /// from. The clipboard was read on the initiative of the tool, so the
    /// message names it.
    ///
    /// `err` is the reason of whichever reader took the text: one chain that
    /// holds a word, and a plan whose `Order` field holds one, both arrive
    /// here. The input is what this function knows and the reader is what it
    /// does not, so it takes any error rather than one kind of error.
    ///
    /// The message names the clipboard and it writes nothing out of it. The
    /// clipboard is the one input the reader did not choose, and it holds a
    /// password, a token, or a recovery code as readily as it holds a chain. A
    /// message that repeats the clipboard puts that secret in the scrollback,
    /// and in every log that keeps standard error. The reason `err` gives
    /// names as much of the text as the argument and the pipe already name,
    /// and no more.
    #[must_use]
    pub fn blame<E>(&self, err: E) -> anyhow::Error
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        match self.source {
            Source::Argument | Source::Stdin => anyhow::Error::new(err),
            Source::Clipboard => anyhow::anyhow!("the clipboard is not a chain: {err}"),
        }
    }
}

/// Every input a chain can come out of, in the order `wn` tries them.
pub struct Sources<'a> {
    /// The chain as the command line gave it, one element for each argument.
    /// Empty when the command line gave none.
    pub argument: &'a [String],
    /// Reads standard input. `None` when standard input is a terminal, which is
    /// a run with nothing piped into it.
    pub stdin: Option<&'a dyn Fn() -> std::io::Result<String>>,
    /// Reads the system clipboard. `None` when [`NO_CLIPBOARD_ENV`] turns the
    /// fallback off.
    pub clipboard: Option<&'a dyn Fn() -> ClipboardRead>,
}

impl Sources<'_> {
    /// The chain, out of the first input that holds one.
    ///
    /// # Errors
    ///
    /// Gives [`InputError::Stdin`] when standard input could not be read at
    /// all, [`InputError::Unavailable`] when the clipboard could not be opened,
    /// [`InputError::EmptyClipboard`] when the clipboard was the last input and
    /// holds no text, and [`InputError::NoChain`] when the clipboard was not
    /// tried and no other input answered.
    pub fn chain(&self) -> Result<Chain, InputError> {
        if !self.argument.is_empty() {
            // A shell splits an unquoted chain into one argument for each
            // word, and a quoted one into a single argument. Joining with a
            // space gives the same line either way, because the parser reads
            // whitespace as a separator.
            return Ok(Chain::new(self.argument.join(" "), Source::Argument));
        }

        if let Some(read) = self.stdin {
            let piped = read().map_err(|cause| InputError::Stdin(cause.to_string()))?;
            if !piped.trim().is_empty() {
                return Ok(Chain::new(piped, Source::Stdin));
            }
        }

        let Some(read) = self.clipboard else {
            return Err(InputError::NoChain);
        };
        match read() {
            Ok(Some(copied)) if !copied.trim().is_empty() => {
                Ok(Chain::new(copied, Source::Clipboard))
            }
            Ok(_) => Err(InputError::EmptyClipboard),
            Err(cause) => Err(InputError::Unavailable(cause)),
        }
    }
}

/// Why no input gave a chain.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InputError {
    /// No input holds a chain, and the clipboard was not one of the inputs.
    #[error("no chain given. {PASS_IT_AS_AN_ARGUMENT}")]
    NoChain,
    /// The clipboard was the last input, and it holds no text.
    #[error("the clipboard is empty. {PASS_IT_AS_AN_ARGUMENT}")]
    EmptyClipboard,
    /// The clipboard could not be opened.
    #[error("the clipboard could not be read ({0}). {PASS_IT_AS_AN_ARGUMENT}")]
    Unavailable(ClipboardUnavailable),
    /// Standard input could not be read.
    #[error("could not read the chain from standard input: {0}")]
    Stdin(String),
}

/// Whether `value`, the value of [`NO_CLIPBOARD_ENV`], turns the fallback off.
///
/// Takes the value as an argument rather than reading the environment, so a
/// test of it touches no process-global state. This mirrors `StartCommand::new`
/// in `main.rs`.
///
/// A value of nothing but whitespace leaves the fallback on. An exported but
/// empty variable is a common accident, and it is not the same statement as
/// `WN_NO_CLIPBOARD=1`.
#[must_use]
pub fn clipboard_is_off(value: Option<&str>) -> bool {
    value.is_some_and(|named| !named.trim().is_empty())
}

/// Read the system clipboard.
///
/// # Errors
///
/// Gives [`ClipboardUnavailable`] when the clipboard could not be opened or
/// could not be read. A machine with no display is such a machine, and so is a
/// session over SSH.
pub fn system_clipboard() -> ClipboardRead {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|cause| ClipboardUnavailable::new(&cause))?;
    from_arboard(clipboard.get_text())
}

/// The read `result` as a [`ClipboardRead`].
///
/// Pure, so the mapping is tested without a clipboard. The one judgment in it
/// is `ContentNotAvailable`: `arboard` says that for a clipboard with nothing
/// in it and for a clipboard that holds an image, and neither of them is a
/// failure to a tool that wants a chain.
///
/// # Errors
///
/// Gives [`ClipboardUnavailable`] for every other failure.
fn from_arboard(result: Result<String, arboard::Error>) -> ClipboardRead {
    match result {
        Ok(text) => Ok(Some(text)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        // `arboard::Error` is `#[non_exhaustive]`, so this arm is required and
        // a new variant of a later version arrives here as a failure that
        // names itself, rather than as a build that stops.
        Err(cause) => Err(ClipboardUnavailable::new(&cause)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::parse_chain;

    /// The chain the tests type, whichever input they type it into.
    const CHAIN: &str = "#277 → #278";

    /// A reader of standard input that must never run.
    fn unread_stdin() -> std::io::Result<String> {
        panic!("standard input was read after an earlier input gave the chain")
    }

    /// A reader of the clipboard that must never run.
    fn unread_clipboard() -> ClipboardRead {
        panic!("the clipboard was read after an earlier input gave the chain")
    }

    /// The arguments of a command line that typed `words`.
    fn arguments(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    /// The chain of a run whose clipboard holds `text` and whose other inputs
    /// hold nothing.
    fn clipboard_chain(text: &str) -> Chain {
        let clipboard = || -> ClipboardRead { Ok(Some(text.to_string())) };
        Sources {
            argument: &[],
            stdin: None,
            clipboard: Some(&clipboard),
        }
        .chain()
        .expect("the clipboard holds text")
    }

    #[test]
    fn an_argument_outranks_a_clipboard_that_holds_another_chain() {
        let argument = arguments(&[CHAIN]);
        let chain = Sources {
            argument: &argument,
            stdin: None,
            clipboard: Some(&unread_clipboard),
        }
        .chain()
        .expect("the argument holds the chain");
        assert_eq!(chain.text(), CHAIN);
    }

    #[test]
    fn an_argument_outranks_a_pipe() {
        let argument = arguments(&[CHAIN]);
        let chain = Sources {
            argument: &argument,
            stdin: Some(&unread_stdin),
            clipboard: None,
        }
        .chain()
        .expect("the argument holds the chain");
        assert_eq!(chain.text(), CHAIN);
    }

    #[test]
    fn a_pipe_that_holds_a_chain_outranks_the_clipboard() {
        let stdin = || -> std::io::Result<String> { Ok(format!("{CHAIN}\n")) };
        let chain = Sources {
            argument: &[],
            stdin: Some(&stdin),
            clipboard: Some(&unread_clipboard),
        }
        .chain()
        .expect("the pipe holds the chain");
        assert_eq!(chain.text(), format!("{CHAIN}\n"));
    }

    #[test]
    fn an_empty_pipe_falls_through_to_the_clipboard() {
        // A parent that redirects standard input from /dev/null did not ask
        // for an empty chain.
        for piped in ["", "   \n\t "] {
            let stdin = || -> std::io::Result<String> { Ok(piped.to_string()) };
            let clipboard = || -> ClipboardRead { Ok(Some(CHAIN.to_string())) };
            let chain = Sources {
                argument: &[],
                stdin: Some(&stdin),
                clipboard: Some(&clipboard),
            }
            .chain()
            .expect("the clipboard holds the chain");
            assert_eq!(chain.text(), CHAIN);
        }
    }

    #[test]
    fn a_terminal_on_standard_input_falls_through_to_the_clipboard() {
        assert_eq!(clipboard_chain(CHAIN).text(), CHAIN);
    }

    #[test]
    fn a_clipboard_chain_reads_the_same_as_the_argument_that_holds_it() {
        let argument = arguments(&[CHAIN]);
        let typed = Sources {
            argument: &argument,
            stdin: None,
            clipboard: None,
        }
        .chain()
        .expect("the argument holds the chain");
        assert_eq!(clipboard_chain(CHAIN).text(), typed.text());
    }

    #[test]
    fn several_arguments_join_back_into_one_line() {
        let argument = arguments(&["#277", "→", "#278"]);
        let chain = Sources {
            argument: &argument,
            stdin: None,
            clipboard: None,
        }
        .chain()
        .expect("the arguments hold the chain");
        assert_eq!(chain.text(), "#277 → #278");
    }

    #[test]
    fn an_empty_clipboard_names_the_clipboard_and_keeps_the_instruction() {
        let clipboard = || -> ClipboardRead { Ok(None) };
        let err = Sources {
            argument: &[],
            stdin: None,
            clipboard: Some(&clipboard),
        }
        .chain()
        .expect_err("the clipboard holds nothing");
        assert_eq!(err, InputError::EmptyClipboard);
        assert_eq!(
            err.to_string(),
            "the clipboard is empty. Pass it as an argument, in quotes: wn \"#277 → #278\""
        );
    }

    #[test]
    fn a_clipboard_of_whitespace_is_an_empty_clipboard() {
        let clipboard = || -> ClipboardRead { Ok(Some("  \n\t ".to_string())) };
        let err = Sources {
            argument: &[],
            stdin: None,
            clipboard: Some(&clipboard),
        }
        .chain()
        .expect_err("the clipboard holds no chain");
        assert_eq!(err, InputError::EmptyClipboard);
    }

    #[test]
    fn a_clipboard_that_cannot_be_opened_names_the_cause_and_keeps_the_instruction() {
        let clipboard = || -> ClipboardRead { Err(ClipboardUnavailable::new(&"no display")) };
        let err = Sources {
            argument: &[],
            stdin: None,
            clipboard: Some(&clipboard),
        }
        .chain()
        .expect_err("the clipboard could not be opened");
        assert_eq!(
            err,
            InputError::Unavailable(ClipboardUnavailable::new(&"no display"))
        );
        assert_eq!(
            err.to_string(),
            "the clipboard could not be read (no display). \
Pass it as an argument, in quotes: wn \"#277 → #278\""
        );
    }

    #[test]
    fn a_cause_that_ends_with_a_period_loses_it() {
        // `arboard` writes a whole sentence, period included: "The selected
        // clipboard is not supported with the current system configuration."
        // The message puts the cause in parentheses and then continues, so a
        // period the cause carries reads as ".)." to the person at the
        // terminal.
        let cause = ClipboardUnavailable::new(&"the clipboard is not supported.");
        assert_eq!(cause.to_string(), "the clipboard is not supported");
        assert_eq!(
            InputError::Unavailable(cause).to_string(),
            "the clipboard could not be read (the clipboard is not supported). \
Pass it as an argument, in quotes: wn \"#277 → #278\""
        );
    }

    #[test]
    fn the_space_around_a_cause_is_dropped() {
        assert_eq!(
            ClipboardUnavailable::new(&"  no display \n").to_string(),
            "no display"
        );
    }

    #[test]
    fn a_cause_of_nothing_but_a_period_is_kept_as_it_is() {
        // Nothing is worse than something, and an error that names no cause at
        // all reads as a bug in the tool rather than as a state of the machine.
        assert_eq!(ClipboardUnavailable::new(&".").to_string(), ".");
    }

    #[test]
    fn no_input_at_all_gives_the_message_the_tool_printed_before() {
        let err = Sources {
            argument: &[],
            stdin: None,
            clipboard: None,
        }
        .chain()
        .expect_err("no input holds a chain");
        assert_eq!(err, InputError::NoChain);
        assert_eq!(
            err.to_string(),
            "no chain given. Pass it as an argument, in quotes: wn \"#277 → #278\""
        );
    }

    #[test]
    fn a_pipe_that_fails_to_read_names_standard_input() {
        let stdin = || -> std::io::Result<String> {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            ))
        };
        let err = Sources {
            argument: &[],
            stdin: Some(&stdin),
            clipboard: Some(&unread_clipboard),
        }
        .chain()
        .expect_err("standard input could not be read");
        assert_eq!(
            err,
            InputError::Stdin("stream did not contain valid UTF-8".to_string())
        );
        assert_eq!(
            err.to_string(),
            "could not read the chain from standard input: stream did not contain valid UTF-8"
        );
    }

    #[test]
    fn the_blame_writes_nothing_out_of_the_clipboard() {
        // A clipboard holds a password, a token, or a recovery code as
        // readily as it holds a chain, and the reader of a run with no
        // argument never asked for the clipboard to be read. So the message
        // names the input and writes nothing out of it.
        let secret = "correct-horse-battery-staple";
        let err = parse_chain(secret).expect_err("the words are not a chain");
        let message = clipboard_chain(secret).blame(err).to_string();
        assert!(message.contains("clipboard"), "{message}");
        assert!(!message.contains(secret), "{message}");
    }

    #[test]
    fn prose_from_the_clipboard_is_blamed_on_the_clipboard() {
        let prose = "let me know what you think";
        let err = parse_chain(prose).expect_err("prose is not a chain");
        assert_eq!(
            clipboard_chain(prose).blame(err).to_string(),
            "the clipboard is not a chain: \"let\" is not an issue number"
        );
    }

    #[test]
    fn a_clipboard_with_no_number_at_all_is_blamed_on_the_clipboard() {
        let arrows = "→ → →";
        let err = parse_chain(arrows).expect_err("arrows alone are not a chain");
        assert_eq!(
            clipboard_chain(arrows).blame(err).to_string(),
            "the clipboard is not a chain: no issue number found in \"→ → →\""
        );
    }

    #[test]
    fn a_plan_from_the_clipboard_is_not_called_a_chain() {
        // A plan is read by the plan reader and it is not a chain, so a
        // message that calls it one tells the reader the wrong thing about
        // what `wn` refused. The input is what the message knows, and the
        // input is the clipboard either way.
        let plan = "Stream: S1 ic\nOrder: #277 an #278";
        let err = crate::plan::parse(plan).expect_err("the word is not an issue number");
        assert_eq!(
            clipboard_chain(plan).blame(err).to_string(),
            "wn cannot read the clipboard: stream \"S1 ic\": \"an\" is not an issue number"
        );
    }

    #[test]
    fn a_word_in_an_argument_is_blamed_on_no_input_at_all() {
        let argument = arguments(&["#277 an #278"]);
        let chain = Sources {
            argument: &argument,
            stdin: None,
            clipboard: None,
        }
        .chain()
        .expect("the argument holds text");
        let err = parse_chain(chain.text()).expect_err("the word is not an issue number");
        let message = chain.blame(err).to_string();
        assert_eq!(message, "\"an\" is not an issue number");
        assert!(!message.contains("clipboard"), "{message}");
    }

    #[test]
    fn a_long_clipboard_text_is_not_echoed_in_the_blame() {
        // A page of prose on the clipboard is the reader's page. The message
        // names the clipboard and stops there.
        let long = "word ".repeat(30);
        let err = parse_chain(&long).expect_err("words are not a chain");
        let message = clipboard_chain(&long).blame(err).to_string();
        assert!(message.contains("clipboard"), "{message}");
        assert!(!message.contains(long.trim()), "{message}");
    }

    #[test]
    fn a_clipboard_of_multi_byte_characters_is_not_echoed_without_a_panic() {
        // The clipboard of a person who reads Japanese holds Japanese. The
        // message is built out of such a clipboard without a panic, and it
        // still writes none of it.
        for word in ["日本語", "🎉", "café"] {
            let text = format!("{word} ").repeat(40);
            let err = parse_chain(&text).expect_err("the text is not a chain");
            let message = clipboard_chain(&text).blame(err).to_string();
            assert!(message.contains("clipboard"), "{message}");
            assert!(!message.contains(text.trim()), "{message}");
        }
    }

    #[test]
    fn a_value_with_a_character_in_it_turns_the_clipboard_off() {
        assert!(!clipboard_is_off(None));
        assert!(!clipboard_is_off(Some("")));
        assert!(!clipboard_is_off(Some("   ")));
        assert!(clipboard_is_off(Some("1")));
        assert!(clipboard_is_off(Some("no")));
    }

    #[test]
    fn text_from_the_clipboard_arrives_whole() {
        assert_eq!(
            from_arboard(Ok(CHAIN.to_string())),
            Ok(Some(CHAIN.to_string()))
        );
    }

    #[test]
    fn a_clipboard_with_no_content_is_an_empty_clipboard() {
        assert_eq!(
            from_arboard(Err(arboard::Error::ContentNotAvailable)),
            Ok(None)
        );
    }

    #[test]
    fn a_platform_with_no_clipboard_cannot_be_read() {
        let read = from_arboard(Err(arboard::Error::ClipboardNotSupported));
        let cause = read.expect_err("the platform has no clipboard").to_string();
        assert!(cause.contains("not supported"), "{cause}");
    }

    #[test]
    fn an_unknown_failure_carries_its_description() {
        let read = from_arboard(Err(arboard::Error::Unknown {
            description: "the window server said no".to_string(),
        }));
        let cause = read.expect_err("the read failed").to_string();
        assert!(cause.contains("the window server said no"), "{cause}");
    }
}
