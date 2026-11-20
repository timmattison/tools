//! Helpers that check whether a branch name is safe to delete.
//!
//! `names_branch` finds a branch name in a commit message as a whole token.
//! `ask` puts a yes or no question to the person at the terminal.

use std::io::{BufRead, Write};

/// Tell whether `message` names `branch` as a whole token.
///
/// The character before the match and the character after it must not be a
/// name character. The start and the end of the message count as boundaries.
/// An empty `branch` names nothing.
///
/// A substring match is the defect this check exists to prevent. `issue-70`
/// is a substring of `Branch: issue-704`, so a substring match lets that typo
/// through.
pub(crate) fn names_branch(message: &str, branch: &str) -> bool {
    if branch.is_empty() {
        return false;
    }
    // `match_indices` gives offsets on char boundaries. `get` returns `None`
    // instead of panicking if an offset is ever not on one.
    message.match_indices(branch).any(|(start, _)| {
        let before = message
            .get(..start)
            .and_then(|head| head.chars().next_back());
        let after = message
            .get(start + branch.len()..)
            .and_then(|tail| tail.chars().next());
        !before.is_some_and(is_name_char) && !after.is_some_and(is_name_char)
    })
}

/// Tell whether `c` can be part of a branch name token.
///
/// The set is ASCII letters, ASCII digits, `.`, `_`, `/` and `-`. Any other
/// character, such as a space, a bracket, `\r`, NUL or a non-ASCII character,
/// ends a token.
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-')
}

/// What the person answered, or that nobody could answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// `y` or `yes`, in any case.
    Yes,
    /// Any other line, an empty line, or end of input.
    No,
    /// Standard input is not a terminal, so gitnuke asked nothing.
    NoTerminal,
}

/// Tell whether `answer` is `y` or `yes`, in any case, after trimming.
fn is_yes(answer: &str) -> bool {
    let answer = answer.trim();
    answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
}

/// Put `question` to the person and read one line of answer.
///
/// With no terminal, this function writes nothing, reads nothing, and returns
/// `Answer::NoTerminal`. A caller that runs without a person, such as an agent,
/// can type the wrong target too, so the caller must not continue silently.
///
/// With a terminal, it writes `question` to `output` as given, flushes, and
/// reads one line. Only `y` or `yes` gives `Answer::Yes`. End of input gives
/// `Answer::No`.
pub(crate) fn ask(
    question: &str,
    terminal: bool,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> std::io::Result<Answer> {
    if !terminal {
        return Ok(Answer::NoTerminal);
    }
    output.write_all(question.as_bytes())?;
    output.flush()?;
    let mut line = String::new();
    // Zero bytes read is end of input, and `is_yes("")` is false.
    input.read_line(&mut line)?;
    Ok(if is_yes(&line) {
        Answer::Yes
    } else {
        Answer::No
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const Q: &str = "Branch not found in recent commits. Delete it? [y/N] ";

    fn run(input: &str) -> (Answer, Vec<u8>) {
        let mut out = Vec::new();
        let answer = ask(Q, true, &mut input.as_bytes(), &mut out).unwrap();
        (answer, out)
    }

    #[test]
    fn match_at_end_of_message() {
        assert!(names_branch("Squash\n\nBranch: issue-42", "issue-42"));
        assert!(!names_branch("Squash\n\nBranch: issue-704", "issue-70"));
    }

    #[test]
    fn shorter_number_is_not_a_match() {
        assert!(!names_branch("Branch: issue-70", "issue-7"));
        assert!(names_branch("Branch: issue-7", "issue-7"));
    }

    #[test]
    fn brackets_are_boundaries() {
        assert!(names_branch("fix: thing (issue-42)", "issue-42"));
        assert!(!names_branch("fix: thing (xissue-42)", "issue-42"));
    }

    #[test]
    fn match_at_start_of_message() {
        assert!(names_branch("issue-42 merged", "issue-42"));
        assert!(!names_branch("issue-421 merged", "issue-42"));
    }

    #[test]
    fn crlf_is_a_boundary() {
        assert!(names_branch("Branch: issue-42\r\n", "issue-42"));
        assert!(!names_branch("Branch: issue-42x\r\n", "issue-42"));
    }

    #[test]
    fn multi_byte_neighbours_do_not_panic() {
        assert!(names_branch("日本語 issue-42 🎉", "issue-42"));
        assert!(names_branch("日本語issue-42🎉", "issue-42"));
        assert!(!names_branch("日本語 issue-42.🎉", "issue-42"));
        assert!(!names_branch("café issue-42-é", "issue-42"));
    }

    #[test]
    fn slash_names_match_whole() {
        assert!(names_branch("Branch: feature/login", "feature/login"));
        assert!(!names_branch("Branch: feature/login/v2", "feature/login"));
        assert!(!names_branch("Branch: feature/login", "login"));
    }

    #[test]
    fn a_later_occurrence_counts() {
        assert!(names_branch("Branch: issue-421 and issue-42", "issue-42"));
        assert!(!names_branch("Branch: issue-421 and issue-420", "issue-42"));
        assert!(!names_branch("anything", ""));
    }

    #[test]
    fn nul_separated_messages() {
        assert!(names_branch("first\0Branch: issue-42\0", "issue-42"));
        assert!(!names_branch("first\0Branch: issue-420\0", "issue-42"));
    }

    #[test]
    fn y_continues_and_n_refuses() {
        assert_eq!(run("y\n").0, Answer::Yes);
        assert_eq!(run("n\n").0, Answer::No);
    }

    #[test]
    fn yes_in_any_case_continues() {
        for line in ["Y\n", "yes\n", "YES\n", "Yes\r\n"] {
            assert_eq!(run(line).0, Answer::Yes, "{line:?}");
        }
        assert_eq!(run("yep\n").0, Answer::No);
    }

    #[test]
    fn empty_line_and_end_of_input_refuse() {
        assert_eq!(run("\n").0, Answer::No);
        assert_eq!(run("").0, Answer::No);
    }

    #[test]
    fn only_the_first_line_is_read() {
        assert_eq!(run("yes please\n").0, Answer::No);
        assert_eq!(run("n\ny\n").0, Answer::No);
    }

    #[test]
    fn question_goes_to_output_verbatim() {
        let (_, out) = run("y\n");
        assert_eq!(out, Q.as_bytes());
    }

    #[test]
    fn no_terminal_asks_nothing_and_reads_nothing() {
        let mut out = Vec::new();
        let mut input: &[u8] = b"y\n";
        let answer = ask(Q, false, &mut input, &mut out).unwrap();
        assert_eq!(answer, Answer::NoTerminal);
        assert!(out.is_empty());
        assert_eq!(input, b"y\n");
    }
}
