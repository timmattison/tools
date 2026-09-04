//! Where the chain comes from.
//!
//! A chain is written once and read many times. It is typed into a plan, then
//! it is copied out of that plan and into a terminal. The copy is the step this
//! module removes: the chain is already on the clipboard when the reader asks
//! what is next, so `wn` with no argument at all reads it from there.
//!
//! # The order of the inputs
//!
//! An argument first, then standard input, then the clipboard, then a run of
//! `claude` that builds a plan. The order is the order of how loudly each
//! input was asked for. An argument was typed on purpose. A pipe was built on
//! purpose. The clipboard was neither, so it answers only when nothing else
//! did. A run that costs money and a minute of waiting is quieter still, so it
//! answers only when the other three did not.
//!
//! `refresh` is the one way past that order. A plan that is still on the
//! clipboard and no longer true would otherwise answer every run, and the
//! reader has no way to say so.
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

use crate::build::{BuildError, NO_CLAUDE_ENV};

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

/// What a run of `claude` gave back.
///
/// `Ok` is the document it printed, which the JSON reader then takes. The
/// document is not read here: this module knows where a text came from and
/// never what is in it.
pub type PlanBuild = Result<String, BuildError>;

/// What a write of the clipboard gave back.
pub type ClipboardWrite = Result<(), ClipboardUnavailable>;

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
    /// A run of `claude` on the `plan-parallel-work` skill.
    Plan,
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

    /// Keep a plan this run built, so the next run reads it back rather than
    /// paying for a second one. Gives the note the reader earns, when they
    /// earn one.
    ///
    /// The clipboard is the cache. It needs no second reader, because the
    /// clipboard fallback of this module already is one, and no file goes
    /// stale in a directory nobody looks in. A reader who copies a line of
    /// code throws the plan away, and a plan somebody threw away was cheap to
    /// rebuild.
    ///
    /// Nothing is kept but a plan this run built. A chain that came from an
    /// argument, from a pipe, or from the clipboard is text the reader already
    /// has, and writing it back would overwrite their clipboard for nothing.
    ///
    /// `read` is whether the reader of the document could read it. A document
    /// that could not be read is never kept: a bad plan on the clipboard is a
    /// bad plan every later run reads, and the reader would have to copy
    /// something else to get out of it.
    ///
    /// `write` is `None` when [`NO_CLIPBOARD_ENV`] turns the clipboard off. A
    /// reader who turned the clipboard off turned the cache off with it, so
    /// nothing is written and nothing is said.
    ///
    /// A write that fails is a note and never a failure. The answer over it is
    /// right, and the one cost is that the next run builds a new plan.
    #[must_use]
    pub fn keep(
        &self,
        read: bool,
        write: Option<&dyn Fn(&str) -> ClipboardWrite>,
    ) -> Option<String> {
        if self.source != Source::Plan || !read {
            return None;
        }
        let write = write?;
        match write(&self.text) {
            Ok(()) => Some(KEPT.to_string()),
            Err(cause) => Some(format!(
                "The plan could not be written to the clipboard ({cause}). \
The next run builds a new one."
            )),
        }
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
    /// does not, so it takes any error rather than one kind of error, and the
    /// message names the input alone. A message that called the text a chain
    /// would tell the reader of a plan the wrong thing about what `wn`
    /// refused.
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
            // A plan `wn` built is a plan `wn` asked for, and the reason it
            // could not be read is the reason of the reader of it, unchanged.
            // Naming the run would tell a reader to look at their clipboard,
            // which holds none of it.
            Source::Argument | Source::Stdin | Source::Plan => anyhow::Error::new(err),
            Source::Clipboard => anyhow::anyhow!("wn cannot read the clipboard: {err}"),
        }
    }
}

/// The note a run that kept its plan earns.
const KEPT: &str = "The plan is on the clipboard. Run wn --refresh to build a new one.";

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
    /// Builds a plan by running `claude`. `None` when [`NO_CLAUDE_ENV`] turns
    /// the run off.
    pub plan: Option<&'a dyn Fn() -> PlanBuild>,
    /// Whether the reader asked for a new plan whatever the other inputs hold.
    pub refresh: bool,
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
        if self.refresh {
            let Some(build) = self.plan else {
                return Err(InputError::RefreshWithoutClaude);
            };
            return built(build);
        }

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

        // A clipboard that holds no chain and a clipboard that could not be
        // opened both say the same thing to the input after them: this input
        // did not answer. So the run is reached from either, and the two
        // errors stand only when there is no run to reach.
        if let Some(read) = self.clipboard {
            match read() {
                Ok(Some(copied)) if !copied.trim().is_empty() => {
                    return Ok(Chain::new(copied, Source::Clipboard));
                }
                Ok(_) => {
                    if self.plan.is_none() {
                        return Err(InputError::EmptyClipboard);
                    }
                }
                Err(cause) => {
                    if self.plan.is_none() {
                        return Err(InputError::Unavailable(cause));
                    }
                }
            }
        }

        let Some(build) = self.plan else {
            // The clipboard was not one of the inputs, and no other input
            // answered. This is the message the tool printed before the run
            // stood beside them.
            return Err(InputError::NoChain);
        };
        built(build)
    }
}

/// The chain a run of `claude` gave back.
///
/// # Errors
///
/// Gives [`InputError::EmptyPlan`] for a run that printed nothing at all, and
/// [`InputError::NoPlan`] for a run that could not happen.
fn built(build: &dyn Fn() -> PlanBuild) -> Result<Chain, InputError> {
    match build() {
        Ok(document) if !document.trim().is_empty() => Ok(Chain::new(document, Source::Plan)),
        Ok(_) => Err(InputError::EmptyPlan),
        Err(cause) => Err(InputError::NoPlan(cause)),
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
    /// The run of `claude` did not give a plan back.
    #[error(transparent)]
    NoPlan(BuildError),
    /// The run of `claude` gave nothing back at all.
    #[error(
        "claude gave no plan back. Run it yourself to see what it says: \
         claude --print '{}'",
        crate::build::PROMPT
    )]
    EmptyPlan,
    /// The reader asked for a new plan and turned the run that builds one off.
    #[error(
        "wn --refresh builds a plan by running claude, and {NO_CLAUDE_ENV} turns that run off. \
         Unset it to build one. {PASS_IT_AS_AN_ARGUMENT}"
    )]
    RefreshWithoutClaude,
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

/// Write `text` to the system clipboard.
///
/// # Errors
///
/// Gives [`ClipboardUnavailable`] when the clipboard could not be opened or
/// could not be written. A machine with no display is such a machine, and so
/// is a session over SSH.
pub fn write_system_clipboard(text: &str) -> ClipboardWrite {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|cause| ClipboardUnavailable::new(&cause))?;
    clipboard
        .set_text(text)
        .map_err(|cause| ClipboardUnavailable::new(&cause))
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

    /// A run of `claude` that must never happen.
    ///
    /// The run costs money and a minute of waiting, so it is the one input
    /// this file must prove is never touched on speculation.
    fn unbuilt_plan() -> PlanBuild {
        panic!("claude was run after an earlier input gave the chain")
    }

    /// The document a run of `claude` gives back in these tests.
    const DOCUMENT: &str = "{\"version\": 1, \"streams\": []}";

    /// A run of `claude` that gives [`DOCUMENT`] back.
    fn built_plan() -> PlanBuild {
        Ok(DOCUMENT.to_string())
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
            plan: None,
            refresh: false,
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
            plan: None,
            refresh: false,
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
            plan: None,
            refresh: false,
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
            plan: None,
            refresh: false,
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
                plan: None,
                refresh: false,
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
            plan: None,
            refresh: false,
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
            plan: None,
            refresh: false,
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
            plan: None,
            refresh: false,
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
            plan: None,
            refresh: false,
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
            plan: None,
            refresh: false,
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
            plan: None,
            refresh: false,
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
            plan: None,
            refresh: false,
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
            "wn cannot read the clipboard: \"let\" is not an issue number"
        );
    }

    #[test]
    fn a_clipboard_with_no_number_at_all_is_blamed_on_the_clipboard() {
        let arrows = "→ → →";
        let err = parse_chain(arrows).expect_err("arrows alone are not a chain");
        assert_eq!(
            clipboard_chain(arrows).blame(err).to_string(),
            "wn cannot read the clipboard: no issue number found in \"→ → →\""
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
            plan: None,
            refresh: false,
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

    /// A writer of the clipboard that must never run.
    fn unwritten_clipboard(_text: &str) -> ClipboardWrite {
        panic!("the clipboard was written for a text this run did not build")
    }

    /// The chain of a run whose only input was a run of `claude` that gave
    /// `text` back.
    fn built_chain(text: &str) -> Chain {
        let plan = || -> PlanBuild { Ok(text.to_string()) };
        Sources {
            argument: &[],
            stdin: None,
            clipboard: None,
            plan: Some(&plan),
            refresh: false,
        }
        .chain()
        .expect("the run gave a plan back")
    }

    #[test]
    fn a_plan_this_run_built_reaches_the_clipboard_whole() {
        let written = std::cell::RefCell::new(String::new());
        let write = |text: &str| -> ClipboardWrite {
            written.borrow_mut().push_str(text);
            Ok(())
        };
        let note = built_chain(DOCUMENT).keep(true, Some(&write));
        assert_eq!(written.into_inner(), DOCUMENT);
        assert_eq!(note, Some(KEPT.to_string()));
        let note = note.expect("a plan that was kept earns a note");
        assert!(note.contains("clipboard"), "{note}");
        assert!(note.contains("--refresh"), "{note}");
    }

    #[test]
    fn a_plan_that_could_not_be_read_never_reaches_the_clipboard() {
        // A bad plan on the clipboard is a bad plan every later run reads.
        assert_eq!(
            built_chain(DOCUMENT).keep(false, Some(&unwritten_clipboard)),
            None
        );
    }

    #[test]
    fn a_chain_the_reader_already_has_never_reaches_the_clipboard() {
        let argument = arguments(&[CHAIN]);
        let typed = Sources {
            argument: &argument,
            stdin: None,
            clipboard: None,
            plan: None,
            refresh: false,
        }
        .chain()
        .expect("the argument holds the chain");
        assert_eq!(typed.keep(true, Some(&unwritten_clipboard)), None);
        assert_eq!(
            clipboard_chain(CHAIN).keep(true, Some(&unwritten_clipboard)),
            None
        );
    }

    #[test]
    fn a_clipboard_that_is_off_keeps_nothing_and_says_nothing() {
        assert_eq!(built_chain(DOCUMENT).keep(true, None), None);
    }

    #[test]
    fn a_clipboard_that_could_not_be_written_earns_a_note_and_not_a_failure() {
        let write = |_: &str| -> ClipboardWrite { Err(ClipboardUnavailable::new(&"no display")) };
        let note = built_chain(DOCUMENT)
            .keep(true, Some(&write))
            .expect("a write that failed earns a note");
        assert_eq!(
            note,
            "The plan could not be written to the clipboard (no display). \
The next run builds a new one."
        );
    }

    #[test]
    fn the_plan_is_built_only_when_the_other_three_inputs_held_nothing() {
        let clipboard = || -> ClipboardRead { Ok(Some(CHAIN.to_string())) };
        let chain = Sources {
            argument: &[],
            stdin: None,
            clipboard: Some(&clipboard),
            plan: Some(&unbuilt_plan),
            refresh: false,
        }
        .chain()
        .expect("the clipboard holds the chain");
        assert_eq!(chain.text(), CHAIN);
    }

    #[test]
    fn an_argument_and_a_pipe_both_outrank_the_run() {
        let argument = arguments(&[CHAIN]);
        assert_eq!(
            Sources {
                argument: &argument,
                stdin: None,
                clipboard: None,
                plan: Some(&unbuilt_plan),
                refresh: false,
            }
            .chain()
            .expect("the argument holds the chain")
            .text(),
            CHAIN
        );
        let stdin = || -> std::io::Result<String> { Ok(CHAIN.to_string()) };
        assert_eq!(
            Sources {
                argument: &[],
                stdin: Some(&stdin),
                clipboard: None,
                plan: Some(&unbuilt_plan),
                refresh: false,
            }
            .chain()
            .expect("the pipe holds the chain")
            .text(),
            CHAIN
        );
    }

    #[test]
    fn an_empty_clipboard_builds_the_plan() {
        let clipboard = || -> ClipboardRead { Ok(None) };
        let chain = Sources {
            argument: &[],
            stdin: None,
            clipboard: Some(&clipboard),
            plan: Some(&built_plan),
            refresh: false,
        }
        .chain()
        .expect("the run gave a plan back");
        assert_eq!(chain.text(), DOCUMENT);
    }

    #[test]
    fn a_clipboard_that_could_not_be_opened_builds_the_plan() {
        // A machine with no clipboard is a machine, not a mistake. The run is
        // the next input, and it answers.
        let clipboard = || -> ClipboardRead { Err(ClipboardUnavailable::new(&"no display")) };
        let chain = Sources {
            argument: &[],
            stdin: None,
            clipboard: Some(&clipboard),
            plan: Some(&built_plan),
            refresh: false,
        }
        .chain()
        .expect("the run gave a plan back");
        assert_eq!(chain.text(), DOCUMENT);
    }

    #[test]
    fn a_clipboard_that_is_off_builds_the_plan() {
        let chain = Sources {
            argument: &[],
            stdin: None,
            clipboard: None,
            plan: Some(&built_plan),
            refresh: false,
        }
        .chain()
        .expect("the run gave a plan back");
        assert_eq!(chain.text(), DOCUMENT);
    }

    #[test]
    fn refresh_builds_the_plan_even_when_the_clipboard_holds_one() {
        let clipboard = || -> ClipboardRead { panic!("refresh does not read the clipboard") };
        let argument = arguments(&[CHAIN]);
        let chain = Sources {
            argument: &argument,
            stdin: Some(&unread_stdin),
            clipboard: Some(&clipboard),
            plan: Some(&built_plan),
            refresh: true,
        }
        .chain()
        .expect("the run gave a plan back");
        assert_eq!(chain.text(), DOCUMENT);
    }

    #[test]
    fn refresh_with_the_run_turned_off_is_a_refusal() {
        let clipboard = || -> ClipboardRead { Ok(Some(CHAIN.to_string())) };
        let err = Sources {
            argument: &[],
            stdin: None,
            clipboard: Some(&clipboard),
            plan: None,
            refresh: true,
        }
        .chain()
        .expect_err("the run is off and refresh asks for one");
        assert_eq!(err, InputError::RefreshWithoutClaude);
        // Two sentences, each a real option. The instruction is a sentence of
        // its own in the three other messages that carry it, so a clause in
        // front of it has to close before it starts.
        assert_eq!(
            err.to_string(),
            "wn --refresh builds a plan by running claude, and WN_NO_CLAUDE turns that run off. \
Unset it to build one. Pass it as an argument, in quotes: wn \"#277 → #278\""
        );
    }

    #[test]
    fn a_run_that_gives_nothing_back_names_claude() {
        for given in ["", "   \n\t "] {
            let plan = || -> PlanBuild { Ok(given.to_string()) };
            let err = Sources {
                argument: &[],
                stdin: None,
                clipboard: None,
                plan: Some(&plan),
                refresh: false,
            }
            .chain()
            .expect_err("the run gave nothing back");
            assert_eq!(err, InputError::EmptyPlan);
            assert!(err.to_string().contains("claude"), "{err}");
        }
    }

    #[test]
    fn a_run_that_failed_carries_its_reason() {
        let plan = || -> PlanBuild {
            Err(BuildError::BadTimeout {
                value: "10m".to_string(),
            })
        };
        let err = Sources {
            argument: &[],
            stdin: None,
            clipboard: None,
            plan: Some(&plan),
            refresh: false,
        }
        .chain()
        .expect_err("the run failed");
        assert_eq!(
            err,
            InputError::NoPlan(BuildError::BadTimeout {
                value: "10m".to_string()
            })
        );
        assert!(err.to_string().contains("WN_PLAN_TIMEOUT"), "{err}");
    }

    #[test]
    fn the_run_that_is_off_leaves_the_two_errors_the_tool_printed_before() {
        let empty = || -> ClipboardRead { Ok(None) };
        assert_eq!(
            Sources {
                argument: &[],
                stdin: None,
                clipboard: Some(&empty),
                plan: None,
                refresh: false,
            }
            .chain()
            .expect_err("the clipboard holds nothing"),
            InputError::EmptyClipboard
        );
        assert_eq!(
            Sources {
                argument: &[],
                stdin: None,
                clipboard: None,
                plan: None,
                refresh: false,
            }
            .chain()
            .expect_err("no input holds a chain"),
            InputError::NoChain
        );
    }

    #[test]
    fn a_plan_that_cannot_be_read_is_blamed_on_no_input_at_all() {
        // A plan `wn` built is a plan `wn` asked for. A message that named the
        // clipboard would send the reader to look at a clipboard that holds
        // none of it.
        let plan = || -> PlanBuild { Ok("#277 an #278".to_string()) };
        let chain = Sources {
            argument: &[],
            stdin: None,
            clipboard: None,
            plan: Some(&plan),
            refresh: false,
        }
        .chain()
        .expect("the run gave a plan back");
        let err = parse_chain(chain.text()).expect_err("the word is not an issue number");
        let message = chain.blame(err).to_string();
        assert_eq!(message, "\"an\" is not an issue number");
        assert!(!message.contains("clipboard"), "{message}");
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
