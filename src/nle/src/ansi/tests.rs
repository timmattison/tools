//! Tests of the reader of escape sequences.

use super::*;

/// The steps of `text`, fed to one reader in chunks.
fn steps(chunks: &[&str]) -> Vec<Step> {
    let mut scanner = Scanner::new();
    chunks
        .iter()
        .flat_map(|chunk| chunk.chars().collect::<Vec<_>>())
        .map(|c| scanner.feed(c))
        .collect()
}

/// The sequences that the reader completes in `chunks`.
fn sequences(chunks: &[&str]) -> Vec<Sequence> {
    steps(chunks)
        .into_iter()
        .filter_map(|step| match step {
            Step::Done(sequence) => Some(sequence),
            _ => None,
        })
        .collect()
}

fn csi(params: &str, intermediates: &str, final_char: char, controls: &str) -> Sequence {
    Sequence::Csi {
        params: params.to_string(),
        intermediates: intermediates.to_string(),
        final_char,
        controls: controls.to_string(),
    }
}

fn osc(text: &str, terminator: &str) -> Sequence {
    Sequence::Osc {
        text: text.to_string(),
        terminator: terminator.to_string(),
    }
}

#[test]
fn text_with_multibyte_characters_around_a_sequence_is_text() {
    let all = steps(&["日本語 café\x1b[1m🎉\r\n"]);

    let text: Vec<bool> = all.iter().map(|step| *step == Step::Text).collect();
    let mut expected = vec![true; 8];
    expected.extend([false; 4]);
    expected.extend([true; 3]);
    assert_eq!(text, expected, "{all:?}");
}

#[test]
fn a_control_sequence_is_pending_until_its_final_character() {
    let all = steps(&["\x1b[?25l"]);

    assert_eq!(all[..5], vec![Step::Pending; 5][..]);
    assert_eq!(all[5], Step::Done(csi("?25", "", 'l', "")));
}

#[test]
fn the_reader_reads_each_form_of_sequence() {
    assert_eq!(
        sequences(&["a\x1b[5 qb\x1b[?25;1049hc"]),
        vec![csi("5", " ", 'q', ""), csi("?25;1049", "", 'h', "")]
    );
    assert_eq!(
        sequences(&["\x1b]12;#ff0000\x07\x1b]12;red\x1b\\\u{9d}112\u{9c}"]),
        vec![
            osc("12;#ff0000", "\x07"),
            osc("12;red", "\x1b\\"),
            osc("112", "\u{9c}"),
        ]
    );
    assert_eq!(
        sequences(&["\x1bc\u{9b}?25h"]),
        vec![Sequence::Escape('c'), csi("?25", "", 'h', "")]
    );
}

#[test]
fn a_sequence_that_two_chunks_split_completes_in_the_second() {
    let mut scanner = Scanner::new();
    for c in "x\x1b[?2".chars().skip(1) {
        assert_eq!(scanner.feed(c), Step::Pending);
    }
    assert!(scanner.is_pending());

    assert_eq!(scanner.feed('5'), Step::Pending);
    assert_eq!(scanner.feed('h'), Step::Done(csi("?25", "", 'h', "")));
    assert!(!scanner.is_pending());
    assert_eq!(scanner.feed('x'), Step::Text);
}

#[test]
fn can_and_sub_stop_a_sequence_and_are_text() {
    for stop in ['\x18', '\x1a'] {
        let text = format!("\x1b[?2{stop}5l");
        let all = steps(&[&text]);

        assert_eq!(all[4], Step::Aborted, "{stop:?}");
        assert_eq!(all[5..], [Step::Text, Step::Text], "{stop:?}");
    }
}

#[test]
fn an_escape_inside_a_control_sequence_starts_a_new_one() {
    let all = steps(&["\x1b[?2\x1b[?25l"]);

    assert_eq!(all[4], Step::Restarted);
    assert_eq!(all.last(), Some(&Step::Done(csi("?25", "", 'l', ""))));
}

#[test]
fn a_control_inside_a_control_sequence_is_kept_apart() {
    assert_eq!(
        sequences(&["\x1b[?2\r5\nl"]),
        vec![csi("?25", "", 'l', "\r\n")]
    );
}

#[test]
fn a_control_sequence_that_a_terminal_ignores_is_ignored() {
    // A parameter after an intermediate, and a character that is not ASCII.
    for text in ["\x1b[ 5q", "\x1b[?2é5l"] {
        let all = steps(&[text]);

        assert_eq!(all.last(), Some(&Step::Ignored), "{text:?}");
        assert!(sequences(&[text]).is_empty(), "{text:?}");
    }
}

#[test]
fn the_text_of_a_string_is_not_read_as_sequences() {
    // Each string ends at its ST, so the sequence after it is read.
    for text in [
        "\x1bPq#0;2;[?25h\x1b\\\x1b[?25l",
        "\u{90}1$r[?25h\u{9c}\x1b[?25l",
        "\x1b_[?25h\x1b\\\x1b[?25l",
    ] {
        assert_eq!(
            sequences(&[text]),
            vec![csi("?25", "", 'l', "")],
            "{text:?}"
        );
    }
}

#[test]
fn an_escape_inside_a_string_ends_it_and_starts_a_new_sequence() {
    let all = steps(&["\x1bPq\x1b[?25h"]);

    assert_eq!(all[4], Step::EscapeBreak(Box::new(Step::Pending)));
    assert_eq!(all.last(), Some(&Step::Done(csi("?25", "", 'h', ""))));
}

#[test]
fn an_escape_with_an_intermediate_is_ignored() {
    let all = steps(&["\x1b(B"]);

    assert_eq!(all, vec![Step::Pending, Step::Pending, Step::Ignored]);
}

#[test]
fn an_operating_system_command_that_an_escape_breaks_stays_and_the_escape_starts_a_new_sequence() {
    let all = steps(&["\x1b]0;title\x1bcx"]);

    assert_eq!(all[all.len() - 3], Step::Pending);
    assert_eq!(
        all[all.len() - 2],
        Step::EscapeBreak(Box::new(Step::Done(Sequence::Escape('c'))))
    );
    assert_eq!(all.last(), Some(&Step::Text));
}
