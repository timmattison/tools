//! The payload printer of `subito`.
//!
//! An MQTT payload is a byte string of any content. This module turns one such
//! byte string into text that a terminal can print without damage.

/// The text an empty payload gives.
const EMPTY_PAYLOAD: &str = "(empty)";

/// The count of bytes one line of the hex dump shows.
const BYTES_PER_LINE: usize = 16;

/// The count of bytes after which the hex field takes one more space.
const HALF_LINE: usize = 8;

/// The lowest byte the ASCII gutter prints as itself.
const FIRST_PRINTABLE: u8 = 0x20;

/// The highest byte the ASCII gutter prints as itself.
const LAST_PRINTABLE: u8 = 0x7e;

/// The character the ASCII gutter prints for every other byte.
const UNPRINTABLE: char = '.';

/// Turns the bytes of one MQTT message into the text the tool prints.
///
/// The rules apply in this order:
///
/// 1. An empty payload gives `(empty)`.
/// 2. `pretty_json` is true and the payload holds JSON: the JSON with
///    indentation.
/// 3. The payload is valid UTF-8 and holds no control character other than
///    the tab, the line feed and the carriage return: the text unchanged.
/// 4. Every other payload: a hex dump.
///
/// Rule 3 is stricter than "valid UTF-8 is text". A null byte and an escape
/// byte are both valid UTF-8, and an escape byte starts a terminal escape
/// sequence. A terminal that prints such a sequence changes its colors, moves
/// its cursor, or clears itself. The hex dump of rule 4 stops that, because a
/// hex dump holds hex digits and printable ASCII only.
#[must_use]
pub fn format_payload(payload: &[u8], pretty_json: bool) -> String {
    if payload.is_empty() {
        return EMPTY_PAYLOAD.to_string();
    }

    if pretty_json {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) {
            if let Ok(indented) = serde_json::to_string_pretty(&value) {
                return indented;
            }
        }
    }

    if let Ok(text) = std::str::from_utf8(payload) {
        if text.chars().all(is_safe_to_print) {
            return text.to_string();
        }
    }

    hex_dump(payload)
}

/// Says whether a terminal can print one character without damage.
///
/// A control character other than the tab, the line feed and the carriage
/// return is not safe. The escape character is the one that matters most,
/// because it starts a terminal escape sequence.
fn is_safe_to_print(character: char) -> bool {
    !character.is_control() || matches!(character, '\t' | '\n' | '\r')
}

/// Gives the hex dump of a payload.
///
/// Each line holds the offset, the bytes as hex digits, and the bytes again as
/// printable ASCII between two `|` characters. A short last line pads with
/// spaces, so the gutter of every line starts in the same column. The lines
/// join with a line feed, and the text has no line feed at its end.
fn hex_dump(payload: &[u8]) -> String {
    let mut dump = String::new();

    for (line, chunk) in payload.chunks(BYTES_PER_LINE).enumerate() {
        if line > 0 {
            dump.push('\n');
        }

        let offset = line * BYTES_PER_LINE;
        dump.push_str(&format!("{offset:08x}  "));

        for slot in 0..BYTES_PER_LINE {
            if slot > 0 {
                dump.push(' ');
            }
            if slot == HALF_LINE {
                dump.push(' ');
            }
            match chunk.get(slot) {
                Some(byte) => dump.push_str(&format!("{byte:02x}")),
                None => dump.push_str("  "),
            }
        }

        dump.push_str("  |");
        for byte in chunk {
            dump.push(if (FIRST_PRINTABLE..=LAST_PRINTABLE).contains(byte) {
                char::from(*byte)
            } else {
                UNPRINTABLE
            });
        }
        dump.push('|');
    }

    dump
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dump of `b"Hello, hex dump!\x00\x01\x02\xff"`. Twenty bytes make two
    /// lines: one full line, and one short line that pads to the gutter.
    const EXPECTED_HEX_DUMP: &str = "00000000  48 65 6c 6c 6f 2c 20 68  65 78 20 64 75 6d 70 21  |Hello, hex dump!|\n00000010  00 01 02 ff                                       |....|";

    #[test]
    fn an_empty_payload_says_it_is_empty() {
        assert_eq!(format_payload(b"", false), "(empty)");
    }

    #[test]
    fn ascii_text_comes_back_unchanged() {
        assert_eq!(format_payload(b"hello", false), "hello");
    }

    #[test]
    fn japanese_text_comes_back_unchanged() {
        assert_eq!(format_payload("日本語".as_bytes(), false), "日本語");
    }

    #[test]
    fn accented_text_comes_back_unchanged() {
        assert_eq!(format_payload("café".as_bytes(), false), "café");
    }

    #[test]
    fn an_emoji_comes_back_unchanged() {
        assert_eq!(format_payload("🎉".as_bytes(), false), "🎉");
    }

    #[test]
    fn a_tab_a_line_feed_and_a_carriage_return_keep_the_payload_text() {
        assert_eq!(format_payload(b"a\tb\nc\rd", false), "a\tb\nc\rd");
    }

    #[test]
    fn a_null_byte_makes_a_hex_dump() {
        assert_eq!(
            format_payload(b"ab\0", false),
            "00000000  61 62 00                                          |ab.|"
        );
    }

    #[test]
    fn an_escape_byte_makes_a_hex_dump() {
        assert_eq!(
            format_payload(b"\x1b[31m", false),
            "00000000  1b 5b 33 31 6d                                    |.[31m|"
        );
    }

    #[test]
    fn invalid_utf8_makes_a_hex_dump() {
        assert_eq!(
            format_payload(&[0xff, 0xfe], false),
            "00000000  ff fe                                             |..|"
        );
    }

    #[test]
    fn a_payload_longer_than_sixteen_bytes_makes_one_line_for_each_sixteen() {
        assert_eq!(
            format_payload(b"Hello, hex dump!\x00\x01\x02\xff", false),
            EXPECTED_HEX_DUMP
        );
    }

    #[test]
    fn json_gets_indentation_when_the_flag_asks_for_it() {
        assert_eq!(format_payload(br#"{"a":1}"#, true), "{\n  \"a\": 1\n}");
    }

    #[test]
    fn a_payload_that_is_not_json_stays_text_when_the_flag_asks_for_json() {
        assert_eq!(format_payload(b"not json", true), "not json");
    }

    #[test]
    fn json_comes_back_unchanged_when_the_flag_does_not_ask_for_it() {
        assert_eq!(format_payload(br#"{"a":1}"#, false), r#"{"a":1}"#);
    }

    #[test]
    fn a_left_to_right_embedding_makes_a_hex_dump() {
        assert_eq!(
            format_payload("ab\u{202a}".as_bytes(), false),
            "00000000  61 62 e2 80 aa                                    |ab...|"
        );
    }

    #[test]
    fn a_right_to_left_override_makes_a_hex_dump() {
        assert_eq!(
            format_payload("ab\u{202e}".as_bytes(), false),
            "00000000  61 62 e2 80 ae                                    |ab...|"
        );
    }

    #[test]
    fn a_left_to_right_isolate_makes_a_hex_dump() {
        assert_eq!(
            format_payload("ab\u{2066}".as_bytes(), false),
            "00000000  61 62 e2 81 a6                                    |ab...|"
        );
    }

    #[test]
    fn a_pop_directional_isolate_makes_a_hex_dump() {
        assert_eq!(
            format_payload("ab\u{2069}".as_bytes(), false),
            "00000000  61 62 e2 81 a9                                    |ab...|"
        );
    }
}
