//! The printer of `subito`.
//!
//! An MQTT payload is a byte string of any content, and an MQTT topic name
//! carries every character other than the null character. This module turns
//! each of the two into text that a terminal can print without damage and in
//! the order of the bytes.

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

/// The first character that embeds or overrides the direction of the text,
/// which is LEFT-TO-RIGHT EMBEDDING.
const FIRST_DIRECTION_OVERRIDE: char = '\u{202a}';

/// The last character that embeds or overrides the direction of the text,
/// which is RIGHT-TO-LEFT OVERRIDE.
const LAST_DIRECTION_OVERRIDE: char = '\u{202e}';

/// The first character that isolates the direction of the text, which is
/// LEFT-TO-RIGHT ISOLATE.
const FIRST_DIRECTION_ISOLATE: char = '\u{2066}';

/// The last character that isolates the direction of the text, which is
/// POP DIRECTIONAL ISOLATE.
const LAST_DIRECTION_ISOLATE: char = '\u{2069}';

/// Turns the bytes of one MQTT message into the text the tool prints.
///
/// The rules apply in this order:
///
/// 1. An empty payload gives `(empty)`.
/// 2. `pretty_json` is true and the payload holds JSON: the JSON with
///    indentation.
/// 3. The payload is valid UTF-8, holds no control character other than the
///    tab, the line feed and the carriage return, and holds no character that
///    changes the direction of the text: the text unchanged.
/// 4. Every other payload: a hex dump.
///
/// Rule 3 is stricter than "valid UTF-8 is text". A null byte, an escape byte
/// and a direction override are all valid UTF-8. An escape byte starts a
/// terminal escape sequence, and a terminal that prints such a sequence
/// changes its colors, moves its cursor, or clears itself. A direction
/// override makes the terminal print the characters of the line in an order
/// the bytes do not have. The hex dump of rule 4 stops both, because a hex
/// dump holds hex digits and printable ASCII only.
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

/// Turns the topic of one MQTT message into the text the tool prints.
///
/// A topic that is safe to print comes back unchanged. Every other topic gives
/// a hex dump of the bytes of the topic.
///
/// The rule for a topic is stricter than the rule of [`format_payload`]. A
/// payload keeps the tab, the line feed and the carriage return, because a
/// payload holds text of more than one line. A topic does not: the tool prints
/// the topic on a line of its own, ahead of the line of the message, so a line
/// feed in a topic writes a second `Topic:` line and a second `Message:` line
/// that the broker never sent. MQTT forbids the null character in a topic name
/// and forbids no other control character, so a publisher can put a line feed
/// in the topic of a message. This function therefore takes no control
/// character at all.
#[must_use]
pub fn format_topic(topic: &str) -> String {
    if topic.chars().all(is_safe_on_one_line) {
        return topic.to_string();
    }

    hex_dump(topic.as_bytes())
}

/// Says whether a terminal can print one character of a payload without damage
/// and in the order of the bytes.
///
/// This rule is the rule of [`is_safe_on_one_line`] with one addition: the
/// tab, the line feed and the carriage return are safe, because a payload
/// holds text of more than one line and the tool prints such a payload as the
/// publisher wrote it.
fn is_safe_to_print(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\r') || is_safe_on_one_line(character)
}

/// Says whether a terminal can print one character on one line without damage
/// and in the order of the bytes.
///
/// A control character is not safe. The escape character is the one that
/// matters most, because it starts a terminal escape sequence. The line feed
/// and the carriage return are control characters too, and they end a line
/// that the tool holds to one line.
///
/// A character that changes the direction of the text is also not safe. Two
/// ranges hold such characters: the embeddings and the overrides, and the
/// isolates. [`char::is_control`] answers for the Unicode category Cc alone,
/// and these characters are in the category Cf. A terminal that prints one of
/// them puts the characters of the line in an order the bytes do not have.
fn is_safe_on_one_line(character: char) -> bool {
    !character.is_control()
        && !(FIRST_DIRECTION_OVERRIDE..=LAST_DIRECTION_OVERRIDE).contains(&character)
        && !(FIRST_DIRECTION_ISOLATE..=LAST_DIRECTION_ISOLATE).contains(&character)
}

/// Gives the hex dump of a byte string.
///
/// Each line holds the offset, the bytes as hex digits, and the bytes again as
/// printable ASCII between two `|` characters. A short last line pads with
/// spaces, so the gutter of every line starts in the same column. The lines
/// join with a line feed, and the text has no line feed at its end.
fn hex_dump(bytes: &[u8]) -> String {
    let mut dump = String::new();

    for (line, chunk) in bytes.chunks(BYTES_PER_LINE).enumerate() {
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

    #[test]
    fn a_plain_topic_comes_back_unchanged() {
        assert_eq!(format_topic("sensors/kitchen"), "sensors/kitchen");
    }

    #[test]
    fn a_topic_of_non_ascii_text_comes_back_unchanged() {
        assert_eq!(format_topic("sensors/温度"), "sensors/温度");
    }

    #[test]
    fn a_topic_that_holds_an_escape_byte_makes_a_hex_dump() {
        assert_eq!(
            format_topic("a\u{1b}b"),
            "00000000  61 1b 62                                          |a.b|"
        );
    }

    /// A payload keeps a line feed, and a topic does not. A line feed in a
    /// topic writes a `Topic:` line and a `Message:` line the broker never
    /// sent.
    #[test]
    fn a_topic_that_holds_a_line_feed_makes_a_hex_dump() {
        assert_eq!(
            format_topic("a\nb"),
            "00000000  61 0a 62                                          |a.b|"
        );
    }

    #[test]
    fn a_topic_that_holds_a_right_to_left_override_makes_a_hex_dump() {
        assert_eq!(
            format_topic("ab\u{202e}"),
            "00000000  61 62 e2 80 ae                                    |ab...|"
        );
    }
}
