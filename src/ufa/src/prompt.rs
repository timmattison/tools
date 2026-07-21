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

use anyhow::{bail, Result};
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
/// which is the only way to assert that a question was *not* asked.
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
}

/// The real terminal.
pub struct Stdio;

impl Console for Stdio {
    fn is_terminal(&self) -> bool {
        io::stdin().is_terminal()
    }

    fn ask(&mut self, question: &str) -> Result<String> {
        print!("{question}");
        io::stdout().flush()?;

        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            bail!("Input ended before the question was answered.");
        }
        Ok(line)
    }
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

/// A [`Console`] with its answers written in advance.
///
/// Records every question so a test can assert that nothing was asked.
#[cfg(test)]
pub struct Scripted {
    answers: std::collections::VecDeque<String>,
    questions: Vec<String>,
    is_terminal: bool,
}

#[cfg(test)]
impl Scripted {
    /// A terminal that will answer with `answers`, in order.
    pub fn terminal(answers: &[&str]) -> Self {
        Self {
            answers: answers.iter().map(|a| (*a).to_string()).collect(),
            questions: Vec::new(),
            is_terminal: true,
        }
    }

    /// A pipe: no person, no answers.
    pub fn not_a_terminal() -> Self {
        Self {
            answers: std::collections::VecDeque::new(),
            questions: Vec::new(),
            is_terminal: false,
        }
    }

    /// Whether anything was asked at all.
    pub fn was_asked(&self) -> bool {
        !self.questions.is_empty()
    }
}

#[cfg(test)]
impl Console for Scripted {
    fn is_terminal(&self) -> bool {
        self.is_terminal
    }

    fn ask(&mut self, question: &str) -> Result<String> {
        self.questions.push(question.to_string());
        match self.answers.pop_front() {
            Some(answer) => Ok(answer),
            None => bail!("the test scripted no answer for {question:?}"),
        }
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
