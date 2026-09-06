//! The question this crate asks the terminal, and what it makes of the answer.
//!
//! The rest of this crate reads the environment and names the terminal from
//! what it finds. That answer is a guess, and it is wrong in the one place a
//! guess costs the most: a terminal that carries no name of its own. A pane of
//! a multiplexer and a session of mosh both arrive that way, and both of them
//! draw pictures.
//!
//! A terminal answers two questions about the sequences it reads. The kitty
//! graphics protocol carries a query action, and a terminal that draws it
//! answers `OK`. The primary device attributes carry parameter 4, which names
//! sixel. So the sentence this crate was written on, "a terminal answers no
//! question about the ones it reads", is false for two of the three protocols.
//!
//! # Why the answer is worth a round trip
//!
//! mosh is the case that pays for it. A mosh session runs the emulator on the
//! server, and that emulator answers both questions for the **pair** — mosh
//! together with the terminal of the user. So a program inside a mosh session
//! learns what the far terminal draws, and it learns it from the one party
//! that knows. No environment variable carries that answer, because no
//! variable crosses the session.
//!
//! # The shape of the round trip
//!
//! [`ask_the_terminal`] writes [`IMAGE_QUERY`] to the controlling terminal and
//! reads until the answer of the primary device attributes arrives or until
//! the budget is spent. **The attributes request stands last, and it must stay
//! last.** Every terminal answers it, so its answer is what ends the read. A
//! terminal that draws no kitty graphics answers the first question with
//! silence, and a read that waited for that silence would spend the whole
//! budget on every run.
//!
//! # The answer that arrives behind the budget
//!
//! The budget ends the read, and it ends no answer of a terminal. A terminal
//! that answers late writes those bytes to the descriptor the shell of the
//! user reads next, and the shell takes them for keystrokes. So the budget is
//! generous enough that a terminal on the far side of a network reaches it,
//! and the probe runs only for a terminal that carries no name at all.

use crate::detect::AnsweredProtocol;
use std::time::Duration;

/// The bytes [`ask_the_terminal`] writes.
///
/// The first is the query action of the kitty graphics protocol: a
/// transmission of one pixel that the terminal answers and never draws. The
/// second is the request of the primary device attributes, which every
/// terminal answers and which therefore ends the read.
pub(crate) const IMAGE_QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";

/// How long [`ask_the_terminal`] waits for the answer.
pub(crate) const QUERY_BUDGET: Duration = Duration::from_millis(500);

/// The protocol the answer of a terminal names, or [`None`] for an answer that
/// names neither.
///
/// A kitty answer outranks a sixel one. A terminal that draws both draws a
/// kitty transmission with no palette and no band, so the picture keeps every
/// colour it arrived with.
pub(crate) fn read_answer(_answer: &[u8]) -> Option<AnsweredProtocol> {
    None
}

/// Ask the controlling terminal which image protocol it draws.
///
/// Gives [`None`] when there is no controlling terminal, when the terminal
/// answers nothing inside `budget`, or when the answer names neither protocol.
pub(crate) fn ask_the_terminal(_budget: Duration) -> Option<AnsweredProtocol> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_that_names_sixel_gives_sixel() {
        // tmux 3.7c answers this, measured 2026-09-06. Parameter 4 names sixel.
        assert_eq!(
            read_answer(b"\x1b[?1;2;4c"),
            Some(AnsweredProtocol::Sixel),
            "parameter 4 of the primary device attributes names sixel"
        );
    }

    #[test]
    fn a_zellij_answer_names_sixel() {
        // zellij 0.45.0 answers this, measured 2026-09-06.
        assert_eq!(
            read_answer(b"\x1bP>|Zellij(4500)\x1b\\\x1b[?62;4;52c"),
            Some(AnsweredProtocol::Sixel)
        );
    }

    #[test]
    fn an_ok_of_the_kitty_query_gives_kitty() {
        assert_eq!(
            read_answer(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;4;52c"),
            Some(AnsweredProtocol::Kitty),
            "a terminal that answers both draws the kitty transmission"
        );
    }

    #[test]
    fn an_answer_that_names_neither_gives_nothing() {
        assert_eq!(read_answer(b"\x1b[?62;22c"), None);
    }

    #[test]
    fn a_refusal_of_the_kitty_query_is_no_kitty_answer() {
        // The specification answers a refused query with a code, not with OK.
        assert_eq!(read_answer(b"\x1b_Gi=31;ENOTSUPP:nope\x1b\\\x1b[?62;22c"), None);
    }

    #[test]
    fn a_parameter_that_merely_holds_a_four_is_no_sixel() {
        // 14 and 41 are not 4. A test of the digit alone would take them.
        assert_eq!(read_answer(b"\x1b[?14;41c"), None);
    }

    #[test]
    fn silence_gives_nothing() {
        assert_eq!(read_answer(b""), None);
    }

    #[test]
    fn the_query_ends_with_the_attributes_request() {
        assert!(
            IMAGE_QUERY.ends_with(b"\x1b[c"),
            "the answer of the attributes request is what ends the read"
        );
    }
}
