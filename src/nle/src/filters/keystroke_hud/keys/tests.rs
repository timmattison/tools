//! Tests of the keystroke parser.

use super::{parse, Key};

fn text(s: &str) -> Key {
    Key::Text(s.to_string())
}

fn special(label: &str) -> Key {
    Key::Special(label.to_string())
}

/// Check each (data, keys) pair.
fn check(cases: &[(&str, Vec<Key>)]) {
    for (data, expected) in cases {
        assert_eq!(&parse(data), expected, "data: {data:?}");
    }
}

#[test]
fn typed_text_merges_into_one_text() {
    check(&[
        ("", vec![]),
        ("ls", vec![text("ls")]),
        ("a b", vec![text("a b")]),
        ("café 日本語 🎉", vec![text("café 日本語 🎉")]),
    ]);
}

#[test]
fn control_keys_have_labels() {
    check(&[
        ("\r", vec![special("Enter")]),
        ("\n", vec![special("Enter")]),
        ("\r\n", vec![special("Enter")]),
        ("\r\r", vec![special("Enter"), special("Enter")]),
        ("\t", vec![special("Tab")]),
        ("\x7f", vec![special("Backspace")]),
        ("\x08", vec![special("Backspace")]),
        ("\x00", vec![special("Ctrl+Space")]),
        ("\x01", vec![special("Ctrl+A")]),
        ("\x03", vec![special("Ctrl+C")]),
        ("\x1a", vec![special("Ctrl+Z")]),
        ("\x1c", vec![special("Ctrl+\\")]),
        ("\x1d", vec![special("Ctrl+]")]),
        ("\x1e", vec![special("Ctrl+^")]),
        ("\x1f", vec![special("Ctrl+_")]),
        ("\u{85}", vec![]),
    ]);
}

#[test]
fn text_and_special_keys_mix() {
    check(&[
        ("ls\r", vec![text("ls"), special("Enter")]),
        ("a\x1b[Ab", vec![text("a"), special("↑"), text("b")]),
    ]);
}

#[test]
fn escape_alone_and_doubled() {
    check(&[
        ("\x1b", vec![special("Esc")]),
        ("\x1b\x1b", vec![special("Esc"), special("Esc")]),
        ("a\x1b", vec![text("a"), special("Esc")]),
    ]);
}

#[test]
fn alt_with_printable_and_control_characters() {
    check(&[
        ("\x1b.", vec![special("Alt+.")]),
        ("\x1bf", vec![special("Alt+f")]),
        ("\x1bF", vec![special("Alt+F")]),
        ("\x1bü", vec![special("Alt+ü")]),
        ("\x1b ", vec![special("Alt+ ")]),
        ("\x1b\r", vec![special("Alt+Enter")]),
        ("\x1b\x7f", vec![special("Alt+Backspace")]),
        ("\x1b\t", vec![special("Alt+Tab")]),
        ("\x1b\x01", vec![special("Ctrl+Alt+A")]),
        ("\x1b\x00", vec![special("Ctrl+Alt+Space")]),
        ("\x1b\u{85}", vec![]),
        ("\x1b[", vec![special("Alt+[")]),
        ("\x1bO", vec![special("Alt+O")]),
        ("\x1b]", vec![special("Alt+]")]),
        ("\x1bP", vec![special("Alt+P")]),
        ("\x1b_", vec![special("Alt+_")]),
    ]);
}

#[test]
fn csi_cursor_keys_with_and_without_modifiers() {
    check(&[
        ("\x1b[A", vec![special("↑")]),
        ("\x1b[B", vec![special("↓")]),
        ("\x1b[C", vec![special("→")]),
        ("\x1b[D", vec![special("←")]),
        ("\x1b[H", vec![special("Home")]),
        ("\x1b[F", vec![special("End")]),
        ("\x1b[P", vec![special("F1")]),
        ("\x1b[Q", vec![special("F2")]),
        ("\x1b[S", vec![special("F4")]),
        ("\x1b[1;5A", vec![special("Ctrl+↑")]),
        ("\x1b[1;3D", vec![special("Alt+←")]),
        ("\x1b[1;6A", vec![special("Ctrl+Shift+↑")]),
        ("\x1b[1;8C", vec![special("Ctrl+Alt+Shift+→")]),
        ("\x1b[1;9A", vec![special("Super+↑")]),
        ("\x1b[1;2:2A", vec![special("Shift+↑")]),
        ("\x1b[Z", vec![special("Shift+Tab")]),
        ("\x1b[1;5Z", vec![special("Ctrl+Shift+Tab")]),
    ]);
}

#[test]
fn csi_tilde_keys() {
    check(&[
        ("\x1b[1~", vec![special("Home")]),
        ("\x1b[2~", vec![special("Insert")]),
        ("\x1b[3~", vec![special("Delete")]),
        ("\x1b[4~", vec![special("End")]),
        ("\x1b[5~", vec![special("PgUp")]),
        ("\x1b[6~", vec![special("PgDn")]),
        ("\x1b[7~", vec![special("Home")]),
        ("\x1b[8~", vec![special("End")]),
        ("\x1b[11~", vec![special("F1")]),
        ("\x1b[12~", vec![special("F2")]),
        ("\x1b[13~", vec![special("F3")]),
        ("\x1b[14~", vec![special("F4")]),
        ("\x1b[15~", vec![special("F5")]),
        ("\x1b[17~", vec![special("F6")]),
        ("\x1b[18~", vec![special("F7")]),
        ("\x1b[19~", vec![special("F8")]),
        ("\x1b[20~", vec![special("F9")]),
        ("\x1b[21~", vec![special("F10")]),
        ("\x1b[23~", vec![special("F11")]),
        ("\x1b[24~", vec![special("F12")]),
        ("\x1b[3;2~", vec![special("Shift+Delete")]),
        ("\x1b[5;5~", vec![special("Ctrl+PgUp")]),
        ("\x1b[99~", vec![]),
    ]);
}

#[test]
fn ss3_keys() {
    check(&[
        ("\x1bOA", vec![special("↑")]),
        ("\x1bOB", vec![special("↓")]),
        ("\x1bOC", vec![special("→")]),
        ("\x1bOD", vec![special("←")]),
        ("\x1bOH", vec![special("Home")]),
        ("\x1bOF", vec![special("End")]),
        ("\x1bOP", vec![special("F1")]),
        ("\x1bOQ", vec![special("F2")]),
        ("\x1bOR", vec![special("F3")]),
        ("\x1bOS", vec![special("F4")]),
        ("\x1bOM", vec![special("Enter")]),
        ("\x1bOj", vec![text("*")]),
        ("\x1bOk", vec![text("+")]),
        ("\x1bOl", vec![text(",")]),
        ("\x1bOm", vec![text("-")]),
        ("\x1bOn", vec![text(".")]),
        ("\x1bOo", vec![text("/")]),
        ("\x1bOX", vec![text("=")]),
        ("\x1bOp", vec![text("0")]),
        ("\x1bOy", vec![text("9")]),
        ("\x1bOz", vec![]),
        ("a\x1bOpb", vec![text("a0b")]),
    ]);
}

#[test]
fn kitty_keys() {
    check(&[
        ("\x1b[99;5u", vec![special("Ctrl+C")]),
        ("\x1b[46;3u", vec![special("Alt+.")]),
        ("\x1b[97u", vec![text("a")]),
        ("ab\x1b[99ucd", vec![text("abccd")]),
        ("\x1b[13u", vec![special("Enter")]),
        ("\x1b[13;2u", vec![special("Shift+Enter")]),
        ("\x1b[9u", vec![special("Tab")]),
        ("\x1b[27u", vec![special("Esc")]),
        ("\x1b[127u", vec![special("Backspace")]),
        ("\x1b[8;5u", vec![special("Ctrl+Backspace")]),
        ("\x1b[32u", vec![text(" ")]),
        ("\x1b[32;5u", vec![special("Ctrl+Space")]),
        ("\x1b[97;1:3u", vec![]),
        ("\x1b[97;1:1u", vec![text("a")]),
        ("\x1b[57441u", vec![]),
        ("\x1b[57358u", vec![]),
        ("\x1b[57344u", vec![]),
        ("\x1b[63743u", vec![]),
        ("\x1b[1114112u", vec![]),
        ("\x1b[0u", vec![]),
        ("\x1b[233u", vec![text("é")]),
        ("\x1b[233;5u", vec![special("Ctrl+é")]),
    ]);
}

#[test]
fn reports_of_the_terminal_give_no_key() {
    check(&[
        ("\x1b[<0;10;5M", vec![]),
        ("\x1b[<0;10;5m", vec![]),
        ("a\x1b[<0;10;5Mb", vec![text("ab")]),
        ("\x1b[M ab", vec![]),
        ("\x1b[M abc", vec![text("c")]),
        ("\x1b[M a", vec![]),
        ("\x1b[12;40R", vec![]),
        ("x\x1b[12;40Ry", vec![text("xy")]),
        ("\x1b[I", vec![]),
        ("\x1b[O", vec![]),
        ("\x1b]11;rgb:0000/0000/0000\x07", vec![]),
        ("a\x1b]11;rgb:0000/0000/0000\x07b", vec![text("ab")]),
        ("a\x1b]0;title\x1b\\b", vec![text("ab")]),
        ("a\x1b]0;title", vec![text("a")]),
        ("a\x1bP1$r\x1b\\b", vec![text("ab")]),
        ("a\x1b_Gdata\x1b\\b", vec![text("ab")]),
        ("a\x1bPdata", vec![text("a")]),
        ("\x1b[?1;2c", vec![]),
        ("\x1b[?1;2cx", vec![text("x")]),
    ]);
}

#[test]
fn bracketed_paste_gives_one_paste() {
    check(&[
        ("\x1b[200~hello\x1b[201~", vec![special("Paste")]),
        (
            "\x1b[200~hello\x1b[201~tail",
            vec![special("Paste"), text("tail")],
        ),
        ("\x1b[200~hello", vec![special("Paste")]),
        (
            "a\x1b[200~\x1b[201~b",
            vec![text("a"), special("Paste"), text("b")],
        ),
    ]);
}

#[test]
fn incomplete_and_oversized_sequences_do_not_panic() {
    check(&[
        ("\x1b[", vec![special("Alt+[")]),
        ("a\x1b[1;5", vec![text("a")]),
        ("a\x1b[12", vec![text("a")]),
        ("\x1b[99999999999999999999u", vec![]),
        ("\x1b[1;99999999999999999999A", vec![special("↑")]),
        // An overflowing number is absent, and the code then has its default, 1.
        ("\x1b[99999999999999999999~", vec![special("Home")]),
        ("\x1b[1;4294967295A", vec![special("Ctrl+Alt+Super+↑")]),
    ]);
}
