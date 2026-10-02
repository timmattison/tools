//! The one place `ufa` asks the user a question.
//!
//! Every interactive moment in this crate — "are you sure?", "which site?" —
//! has the same three failure modes: the answer is read from somewhere that
//! isn't a person (a pipe, a CI job, `</dev/null`), the answer stream ends
//! mid-question, or the question is asked at all when it did not need to be.
//! Spreading `print!` / `flush` / `read_line` around the command modules gets
//! each of those wrong independently, so the decisions live here instead and
//! the commands only say *what* they need agreed to.
//!
//! The decisions are pure functions over `(match_count, assume_yes,
//! is_terminal, response)`; the terminal I/O is a thin [`Console`] around
//! them, which is what makes "did it even ask?" testable without a tty.
//!
//! Every question and every note goes to standard error. Standard output
//! carries only the document the user asked for, and under `--output json` a
//! program reads that stream.

use anyhow::{bail, Context, Result};
use std::io::{self, BufRead, IsTerminal, Write};

/// The verdict on an action that would destroy something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// Nothing matched, so there is nothing to approve and nothing to do.
    NothingMatched,
    /// The action may go ahead.
    Approved,
    /// The user was asked and said no.
    Declined,
}

/// Somewhere the user can be asked something.
///
/// Production answers come from the terminal ([`Stdio`]); tests script them,
/// which is the only way to assert that a question was *not* asked. What a
/// console shows is for a person, so the real one never writes it to
/// standard output.
pub trait Console {
    /// Whether the answers are coming from a person at a terminal.
    fn is_terminal(&self) -> bool;

    /// Show `question` and read one line of the answer.
    ///
    /// # Errors
    ///
    /// Returns an error if the answer stream ends before a line arrives, so a
    /// closed stdin ends the question instead of looping on empty answers.
    fn ask(&mut self, question: &str) -> Result<String>;

    /// Show `question` and read one line of the answer, but do not show what
    /// the user types.
    ///
    /// An answer read with [`Console::ask`] shows on the screen. A credential
    /// read that way stays in the scrollback, in a screen share and in a
    /// recording.
    ///
    /// # Errors
    ///
    /// Returns an error if the answer cannot be read.
    fn ask_hidden(&mut self, question: &str) -> Result<String>;

    /// Show `message` on a line of its own.
    fn tell(&mut self, message: &str);
}

/// The real terminal: answers from standard input, and questions and notes on
/// standard error.
pub struct Stdio;

impl Console for Stdio {
    fn is_terminal(&self) -> bool {
        io::stdin().is_terminal()
    }

    fn ask(&mut self, question: &str) -> Result<String> {
        eprint!("{question}");
        io::stderr().flush()?;

        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            bail!("Input ended before the question was answered.");
        }
        Ok(line)
    }

    fn ask_hidden(&mut self, question: &str) -> Result<String> {
        let (blank_lines, prompt) = hidden_prompt(question);
        eprint!("{blank_lines}");

        // An empty answer comes back empty. The caller decides what it means.
        dialoguer::Password::new()
            .with_prompt(prompt)
            .allow_empty_password(true)
            .interact()
            .context("Could not read the hidden answer from the terminal")
    }

    fn tell(&mut self, message: &str) {
        eprintln!("{message}");
    }
}

/// Split `question` into the blank lines in front of it and the prompt that
/// [`dialoguer::Password`] shows.
///
/// The theme of dialoguer puts `": "` after the prompt, so the prompt goes
/// without the colon of the question. The prompt holds one line, so the
/// blank lines go out on their own first.
///
/// # Returns
///
/// The blank lines, and the prompt.
fn hidden_prompt(question: &str) -> (&str, &str) {
    let blank_lines = question
        .strip_suffix(question.trim_start())
        .unwrap_or_default();
    (blank_lines, without_prompt_punctuation(question))
}

/// What a confirmation does *before* any answer is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmStep {
    /// Nothing matched: say nothing, do nothing.
    NothingMatched,
    /// `--yes` was given: no question needed.
    Approved,
    /// Put the question to the user.
    Ask,
    /// There is nobody to ask and no `--yes`: refuse rather than guess.
    NeedsExplicitYes,
}

/// Decide what a confirmation should do, without asking anything.
///
/// # Arguments
///
/// * `match_count` - How many items the action would affect.
/// * `assume_yes` - Whether the user passed `--yes`.
/// * `is_terminal` - Whether answers would come from a person.
fn plan_confirmation(match_count: usize, assume_yes: bool, is_terminal: bool) -> ConfirmStep {
    if match_count == 0 {
        ConfirmStep::NothingMatched
    } else if assume_yes {
        ConfirmStep::Approved
    } else if is_terminal {
        ConfirmStep::Ask
    } else {
        ConfirmStep::NeedsExplicitYes
    }
}

/// Interpret one typed answer to a `[y/N]` question.
///
/// Anything that is not an explicit yes — including an empty line — is a no.
fn answered_yes(response: &str) -> bool {
    matches!(response.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Ask the user to approve an action that would destroy `match_count` items.
///
/// # Arguments
///
/// * `console` - Where the question is put.
/// * `question` - The question, without the `[y/N]` suffix.
/// * `match_count` - How many items the action would affect.
/// * `assume_yes` - Whether the user passed `--yes`.
///
/// # Returns
///
/// Whether the action may go ahead.
///
/// # Errors
///
/// Returns an error when the answers do not come from a terminal and `--yes`
/// was not given, so a piped invocation refuses instead of silently
/// destroying things or hanging on a prompt nobody can see.
pub fn confirm_destructive(
    console: &mut impl Console,
    question: &str,
    match_count: usize,
    assume_yes: bool,
) -> Result<Approval> {
    match plan_confirmation(match_count, assume_yes, console.is_terminal()) {
        ConfirmStep::NothingMatched => Ok(Approval::NothingMatched),
        ConfirmStep::Approved => Ok(Approval::Approved),
        ConfirmStep::NeedsExplicitYes => bail!(
            "{question} needs confirmation, but stdin is not a terminal. \
             Re-run with --yes to confirm without being asked."
        ),
        ConfirmStep::Ask => {
            let answer = console.ask(&format!("{question} [y/N]: "))?;
            Ok(if answered_yes(&answer) {
                Approval::Approved
            } else {
                Approval::Declined
            })
        }
    }
}

/// Ask which of `count` numbered options to use.
///
/// The options themselves have already been shown to the user; this asks for
/// the number and keeps asking until one of them is named.
///
/// # Arguments
///
/// * `console` - Where the question is put.
/// * `question` - The question, without the `[1-n]` suffix.
/// * `count` - How many options there are.
///
/// # Returns
///
/// The zero-based index of the chosen option, or `None` when there is no
/// terminal to ask at -- which the caller has to answer for itself, because
/// only it knows how the choice can be named on the command line instead.
///
/// # Errors
///
/// Returns an error if the answer stream ends before a choice is made.
pub fn select_one(
    console: &mut impl Console,
    question: &str,
    count: usize,
) -> Result<Option<usize>> {
    // Nothing to choose between, and nobody to ask when the answers are not
    // coming from a person.
    if count == 0 || !console.is_terminal() {
        return Ok(None);
    }

    loop {
        let answer = console.ask(&format!("{question} [1-{count}]: "))?;

        match answer.trim().parse::<usize>() {
            Ok(choice) if (1..=count).contains(&choice) => return Ok(Some(choice - 1)),
            _ => console.tell("Invalid choice. Please try again."),
        }
    }
}

/// The question as it reads inside a sentence.
///
/// A prompt carries the spacing and the colon that separate it from what the
/// user types. Neither belongs in an error message.
fn without_prompt_punctuation(question: &str) -> &str {
    question.trim().trim_end_matches(':').trim_end()
}

/// Put `question` to the user and return the trimmed answer.
///
/// A free-text question has no `--yes` and no default, so a run with nobody
/// to ask cannot get an answer at all: it refuses here rather than reads.
/// A closed stdin would end the read on its own, but a stdin that is neither
/// a terminal nor closed -- a pipe inherited from a scheduler or a CI runner
/// -- sends nothing and closes nothing, and the read waits for good.
///
/// # Arguments
///
/// * `console` - Where the question is put.
/// * `question` - The question, with the punctuation and spacing it is shown
///   with.
///
/// # Returns
///
/// The answer, without the space around it.
///
/// # Errors
///
/// Returns an error when the answers do not come from a terminal, and if the
/// answer stream ends before a line arrives.
pub fn ask_line(console: &mut impl Console, question: &str) -> Result<String> {
    refuse_without_a_terminal(console, question)?;
    Ok(console.ask(question)?.trim().to_string())
}

/// Put `question` to the user, do not show what the user types, and return
/// the trimmed answer.
///
/// This is [`ask_line`] for a credential. It refuses the same run that
/// [`ask_line`] refuses, with the same words. An empty answer comes back
/// empty, because a caller can give an empty answer a meaning, for example
/// "skip".
///
/// # Arguments
///
/// * `console` - Where the question is put.
/// * `question` - The question, with the punctuation and spacing it is shown
///   with.
///
/// # Returns
///
/// The answer, without the space around it.
///
/// # Errors
///
/// Returns an error when the answers do not come from a terminal, and if the
/// answer cannot be read.
pub fn ask_hidden_line(console: &mut impl Console, question: &str) -> Result<String> {
    refuse_without_a_terminal(console, question)?;
    Ok(console.ask_hidden(question)?.trim().to_string())
}

/// Refuse a free-text question when the answers do not come from a terminal.
///
/// # Errors
///
/// Returns an error that names the question and says a terminal is missing.
fn refuse_without_a_terminal(console: &impl Console, question: &str) -> Result<()> {
    if !console.is_terminal() {
        bail!(
            "{} needs an answer, but stdin is not a terminal. \
             Run the command at a terminal to answer it.",
            without_prompt_punctuation(question)
        );
    }
    Ok(())
}

/// Ask a yes/no question that defaults to no.
///
/// # Arguments
///
/// * `console` - Where the question is put.
/// * `question` - The question, without the `[y/N]` suffix.
///
/// # Returns
///
/// Whether the answer was an explicit yes.
///
/// # Errors
///
/// Returns an error when the answers do not come from a terminal, and if the
/// answer stream ends before a line arrives. An unanswerable yes/no question
/// is not read as a no: [`confirm_destructive`] is the one that has a `--yes`
/// to name instead.
pub fn confirm(console: &mut impl Console, question: &str) -> Result<bool> {
    Ok(answered_yes(&ask_line(
        console,
        &format!("{question} [y/N]: "),
    )?))
}

/// A [`Console`] with its answers written in advance.
///
/// Records every question so a test can assert that nothing was asked, and
/// every message so a test can assert what the user was shown. A hidden
/// question goes in a record of its own, so a test can tell a read that shows
/// the answer from a read that does not.
#[cfg(test)]
pub struct Scripted {
    answers: std::collections::VecDeque<String>,
    questions: Vec<String>,
    hidden_questions: Vec<String>,
    told: Vec<String>,
    is_terminal: bool,
}

#[cfg(test)]
impl Scripted {
    /// A terminal that will answer with `answers`, in order. Hidden and shown
    /// questions take their answers from the same list.
    pub fn terminal(answers: &[&str]) -> Self {
        Self {
            answers: answers.iter().map(|a| (*a).to_string()).collect(),
            questions: Vec::new(),
            hidden_questions: Vec::new(),
            told: Vec::new(),
            is_terminal: true,
        }
    }

    /// A pipe: no person, no answers.
    pub fn not_a_terminal() -> Self {
        Self {
            answers: std::collections::VecDeque::new(),
            questions: Vec::new(),
            hidden_questions: Vec::new(),
            told: Vec::new(),
            is_terminal: false,
        }
    }

    /// Whether anything was asked at all, hidden or shown.
    pub fn was_asked(&self) -> bool {
        !self.questions.is_empty() || !self.hidden_questions.is_empty()
    }

    /// Every question put through [`Console::ask_hidden`], one question per
    /// line.
    pub fn asked_hidden(&self) -> String {
        self.hidden_questions.join("\n")
    }

    /// The next scripted answer to `question`.
    fn next_answer(&mut self, question: &str) -> Result<String> {
        match self.answers.pop_front() {
            Some(answer) => Ok(answer),
            None => bail!("the test scripted no answer for {question:?}"),
        }
    }

    /// Everything shown through [`Console::tell`], one message per line.
    pub fn told(&self) -> String {
        self.told.join("\n")
    }

    /// Every question put through [`Console::ask`], one question per line.
    pub fn asked(&self) -> String {
        self.questions.join("\n")
    }
}

#[cfg(test)]
impl Console for Scripted {
    fn is_terminal(&self) -> bool {
        self.is_terminal
    }

    fn ask(&mut self, question: &str) -> Result<String> {
        self.questions.push(question.to_string());
        self.next_answer(question)
    }

    fn ask_hidden(&mut self, question: &str) -> Result<String> {
        self.hidden_questions.push(question.to_string());
        self.next_answer(question)
    }

    fn tell(&mut self, message: &str) {
        self.told.push(message.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An action that would affect nothing must not put a question to anyone.
    #[test]
    fn nothing_matched_asks_nothing() {
        assert_eq!(
            plan_confirmation(0, false, true),
            ConfirmStep::NothingMatched,
            "a filter that matched nothing has nothing to confirm"
        );
        assert_eq!(
            plan_confirmation(0, true, false),
            ConfirmStep::NothingMatched,
            "an empty match stays empty whatever the flags say"
        );
    }

    /// `--yes` is the scripted caller's way of answering in advance.
    #[test]
    fn assume_yes_skips_the_question() {
        assert_eq!(
            plan_confirmation(3, true, true),
            ConfirmStep::Approved,
            "--yes must not stop to ask"
        );
        assert_eq!(
            plan_confirmation(3, true, false),
            ConfirmStep::Approved,
            "--yes is exactly how a pipe approves"
        );
    }

    /// A person at a terminal gets asked.
    #[test]
    fn a_terminal_is_asked() {
        assert_eq!(
            plan_confirmation(3, false, true),
            ConfirmStep::Ask,
            "a destructive action at a terminal must be confirmed"
        );
    }

    /// A pipe cannot answer, so proceeding would be guessing.
    #[test]
    fn a_pipe_without_yes_refuses() {
        assert_eq!(
            plan_confirmation(3, false, false),
            ConfirmStep::NeedsExplicitYes,
            "a non-interactive run must refuse rather than auto-confirm"
        );
    }

    /// Only an explicit yes is a yes; an empty line is the default no.
    #[test]
    fn only_an_explicit_yes_approves() {
        for yes in ["y", "Y", "yes", "YES", " y \n"] {
            assert!(answered_yes(yes), "{yes:?} must be read as approval");
        }
        for no in ["", "\n", "n", "no", "N", "maybe", "ye s"] {
            assert!(!answered_yes(no), "{no:?} must not be read as approval");
        }
    }

    /// A person at a terminal can say which one they meant.
    #[test]
    fn a_terminal_is_asked_which_one_to_use() {
        let mut console = Scripted::terminal(&["2"]);

        let chosen = select_one(&mut console, "Select a site", 3)
            .expect("a scripted answer must be readable");

        assert_eq!(chosen, Some(1), "the second option is index 1");
        assert!(console.was_asked(), "the user must have been asked");
    }

    /// A number that names no option is a typo, not a choice.
    #[test]
    fn an_answer_outside_the_range_is_asked_again() {
        let mut console = Scripted::terminal(&["0", "4", "nope", "3"]);

        let chosen = select_one(&mut console, "Select a site", 3)
            .expect("a scripted answer must be readable");

        assert_eq!(chosen, Some(2), "only the in-range answer counts");
    }

    /// A pipe cannot pick, and must not be left hanging on a prompt.
    #[test]
    fn a_pipe_is_not_asked_which_one_to_use() {
        let mut console = Scripted::not_a_terminal();

        let chosen =
            select_one(&mut console, "Select a site", 3).expect("no terminal is not an error");

        assert_eq!(chosen, None, "there is nobody to ask");
        assert!(
            !console.was_asked(),
            "nothing can be asked without a terminal"
        );
    }

    /// A person at a terminal answers a free-text question, and the answer
    /// arrives without the newline the terminal put on the end of it.
    #[test]
    fn a_terminal_answers_a_free_text_question() {
        let mut console = Scripted::terminal(&["  https://192.168.1.1  \n"]);

        let answer = ask_line(&mut console, "Enter your UniFi controller URL: ")
            .expect("a scripted answer must be readable");

        assert_eq!(
            answer, "https://192.168.1.1",
            "the answer must arrive without the space around it"
        );
        assert!(console.was_asked(), "the user must have been asked");
    }

    /// A free-text question has no `--yes` and no default, so a run with
    /// nobody to ask cannot get an answer at all. A pipe that stays open and
    /// sends nothing -- an inherited handle under a scheduler or a CI runner
    /// -- never ends the read either, so the question must not be put at all.
    #[test]
    fn a_pipe_is_not_asked_a_free_text_question() {
        let mut console = Scripted::not_a_terminal();

        let error = ask_line(&mut console, "Enter your UniFi controller URL: ")
            .expect_err("a pipe cannot answer a free-text question");

        assert!(
            !console.was_asked(),
            "nothing can be asked when there is no terminal"
        );
        assert!(
            format!("{error:#}").contains("terminal"),
            "the refusal must say a terminal is what is missing, got {error:#}"
        );
    }

    /// A hidden question gets its answer without the space around it, and
    /// the console records it as hidden, not as a question that shows the
    /// answer.
    #[test]
    fn a_terminal_answers_a_hidden_question() {
        let mut console = Scripted::terminal(&["  key-from-the-clipboard  \n"]);

        let answer = ask_hidden_line(&mut console, "\nPaste your API key here: ")
            .expect("a scripted answer must be readable");

        assert_eq!(
            answer, "key-from-the-clipboard",
            "the answer must arrive without the space around it"
        );
        assert_eq!(
            console.asked_hidden(),
            "\nPaste your API key here: ",
            "the question must be put as a hidden question"
        );
        assert_eq!(
            console.asked(),
            "",
            "a hidden question must not also be put as a question that shows the answer"
        );
    }

    /// An empty answer to a hidden question comes back empty. A caller can
    /// give it a meaning, and the Site Manager prompt reads it as "skip".
    #[test]
    fn an_empty_hidden_answer_comes_back_empty() {
        let mut console = Scripted::terminal(&["\n"]);

        let answer = ask_hidden_line(&mut console, "Site Manager API key [skip]: ")
            .expect("an empty answer is an answer");

        assert_eq!(answer, "", "an empty line must come back empty");
    }

    /// A hidden question refuses a pipe for the same reason that a question
    /// that shows the answer does, and with the same words.
    #[test]
    fn a_pipe_is_not_asked_a_hidden_question() {
        let mut console = Scripted::not_a_terminal();

        let error = ask_hidden_line(&mut console, "\nPaste your API key here: ")
            .expect_err("a pipe cannot answer a hidden question");

        assert!(
            !console.was_asked(),
            "nothing can be asked when there is no terminal"
        );
        assert_eq!(
            format!("{error:#}"),
            format!(
                "{:#}",
                ask_line(
                    &mut Scripted::not_a_terminal(),
                    "\nPaste your API key here: "
                )
                .expect_err("a pipe cannot answer a free-text question")
            ),
            "the hidden read must refuse with the words of the read that shows the answer"
        );
        assert!(
            format!("{error:#}").contains("terminal"),
            "the refusal must say a terminal is what is missing, got {error:#}"
        );
    }

    /// The theme of dialoguer puts `": "` after the prompt, so the prompt
    /// goes without the colon of the question, and the blank lines in front
    /// of the question go out on their own.
    #[test]
    fn a_hidden_prompt_loses_its_colon_and_keeps_its_blank_lines() {
        for (question, blank_lines, prompt) in [
            (
                "\nPaste your API key here: ",
                "\n",
                "Paste your API key here",
            ),
            (
                "Site Manager API key or 1Password reference [skip]: ",
                "",
                "Site Manager API key or 1Password reference [skip]",
            ),
            ("\n\n🔑 Schlüssel 日本語: ", "\n\n", "🔑 Schlüssel 日本語"),
        ] {
            assert_eq!(
                hidden_prompt(question),
                (blank_lines, prompt),
                "{question:?} must split into its blank lines and its prompt"
            );
        }
    }

    /// The same guard reaches the yes/no questions of the setup wizard, which
    /// go through the same read.
    #[test]
    fn a_pipe_is_not_asked_a_yes_no_question() {
        let mut console = Scripted::not_a_terminal();

        let error = confirm(&mut console, "Save configuration anyway?")
            .expect_err("a pipe cannot answer a yes/no question");

        assert!(
            !console.was_asked(),
            "nothing can be asked when there is no terminal"
        );
        assert!(
            format!("{error:#}").contains("terminal"),
            "the refusal must say a terminal is what is missing, got {error:#}"
        );
    }

    /// At a terminal the question is still put, and only an explicit yes is a
    /// yes.
    #[test]
    fn a_terminal_answers_a_yes_no_question() {
        for (answer, approved) in [("y\n", true), ("\n", false), ("no\n", false)] {
            let mut console = Scripted::terminal(&[answer]);

            let outcome = confirm(&mut console, "Use this URL anyway?")
                .expect("a scripted answer must be readable");

            assert_eq!(
                outcome, approved,
                "{answer:?} must be read as approved={approved}"
            );
            assert!(console.was_asked(), "the user must have been asked");
        }
    }

    /// The refusal has to name the escape hatch, or a scripted caller has no
    /// way to find out how to proceed.
    #[test]
    fn the_refusal_names_the_yes_flag() {
        let mut console = Scripted::not_a_terminal();
        let error = confirm_destructive(&mut console, "Delete 3 voucher(s)?", 3, false)
            .expect_err("a pipe must not be able to confirm a destructive action");

        assert!(
            format!("{error:#}").contains("--yes"),
            "the refusal must point at --yes, got {error:#}"
        );
        assert!(
            !console.was_asked(),
            "nothing can be asked when there is no terminal"
        );
    }
}
