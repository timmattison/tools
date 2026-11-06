//! `sidecar add <name> [--yes]`: make a sidecar repository and register it.

use std::io::BufRead;

/// The question that `sidecar add` asks on standard error.
const QUESTION: &str = "Do these steps? [y/N] ";

/// What the question about the plan decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    /// Do the steps.
    Proceed,
    /// Stop. There is no `--yes` and no terminal to ask.
    NeedsYes,
    /// Stop. The user did not answer yes.
    Declined,
}

/// Decides whether to do the steps of the plan.
///
/// With `yes`, it does not read. Without `yes` and without a terminal, it refuses.
/// On a terminal, it asks on standard error and reads one line from `input`.
/// The answer `y` or `yes`, in any case and with spaces around, means yes.
pub fn confirm(yes: bool, is_terminal: bool, input: &mut impl BufRead) -> Confirmation {
    if yes {
        return Confirmation::Proceed;
    }
    if !is_terminal {
        return Confirmation::NeedsYes;
    }
    eprint!("{QUESTION}");
    let mut line = String::new();
    if input.read_line(&mut line).is_err() {
        return Confirmation::Declined;
    }
    if matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Confirmation::Proceed
    } else {
        Confirmation::Declined
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn answer(yes: bool, is_terminal: bool, text: &str) -> (Confirmation, u64) {
        let mut input = Cursor::new(text.as_bytes().to_vec());
        let decision = confirm(yes, is_terminal, &mut input);
        (decision, input.position())
    }

    #[test]
    fn the_flag_goes_on_without_a_read() {
        for is_terminal in [true, false] {
            assert_eq!(
                answer(true, is_terminal, "n\n"),
                (Confirmation::Proceed, 0),
                "terminal: {is_terminal}"
            );
        }
    }

    #[test]
    fn no_terminal_and_no_flag_refuses_without_a_read() {
        assert_eq!(answer(false, false, "y\n"), (Confirmation::NeedsYes, 0));
    }

    #[test]
    fn a_yes_answer_on_a_terminal_goes_on() {
        for text in ["y\n", "YES\n", " yes \n", "Y\r\n", "yes"] {
            assert_eq!(
                answer(false, true, text).0,
                Confirmation::Proceed,
                "{text:?}"
            );
        }
    }

    #[test]
    fn any_other_answer_on_a_terminal_stops() {
        for text in ["n\n", "\n", "", "maybe\n", "yess\n", "no\n"] {
            assert_eq!(
                answer(false, true, text).0,
                Confirmation::Declined,
                "{text:?}"
            );
        }
    }
}
