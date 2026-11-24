/// Read a typedstream integer used as a length.
///
/// A first byte below `0x80` is the value. `0x81` is followed by an i16 and
/// `0x82` by an i32, both little-endian. Returns the length and the count of
/// bytes read, or `None` for a negative value, any other first byte, or
/// missing bytes.
fn read_typedstream_length(bytes: &[u8]) -> Option<(usize, usize)> {
    let first = *bytes.first()?;
    match first {
        0x00..=0x7f => Some((usize::from(first), 1)),
        0x81 => {
            let raw: [u8; 2] = bytes.get(1..3)?.try_into().ok()?;
            let len = usize::try_from(i16::from_le_bytes(raw)).ok()?;
            Some((len, 3))
        }
        0x82 => {
            let raw: [u8; 4] = bytes.get(1..5)?.try_into().ok()?;
            let len = usize::try_from(i32::from_le_bytes(raw)).ok()?;
            Some((len, 5))
        }
        _ => None,
    }
}

/// Extract plain text from an iMessage `attributedBody` blob.
///
/// The blob is an NSKeyedArchiver/typedstream-encoded NSAttributedString.
/// We find the `NSString` marker and extract the text that follows.
///
/// # Errors
///
/// Returns `None` if the blob cannot be parsed.
pub fn extract_text_from_attributed_body(blob: &[u8]) -> Option<String> {
    const NSSTRING_MARKER: &[u8] = b"NSString";
    const HEADER: &[u8] = &[0x01, 0x94, 0x84, 0x01];
    const STRING_TYPE: u8 = 0x2b;

    let marker_pos = blob
        .windows(NSSTRING_MARKER.len())
        .position(|w| w == NSSTRING_MARKER)?;

    let header_start = marker_pos.checked_add(NSSTRING_MARKER.len())?;
    let header_end = header_start.checked_add(HEADER.len())?;

    if blob.get(header_start..header_end) != Some(HEADER) {
        log::warn!(
            "attributedBody: unexpected header bytes at offset {}: {:02x?}",
            header_start,
            blob.get(header_start..)
                .map(|rest| &rest[..rest.len().min(4)])
        );
        return None;
    }

    let type_byte = *blob.get(header_end)?;
    if type_byte != STRING_TYPE {
        log::warn!("attributedBody: unknown type byte 0x{type_byte:02x} at offset {header_end}");
        return None;
    }

    let len_pos = header_end + 1;
    let Some((text_len, len_size)) = blob.get(len_pos..).and_then(read_typedstream_length) else {
        log::warn!("attributedBody: invalid length at offset {len_pos}");
        return None;
    };

    let text_start = len_pos.checked_add(len_size)?;
    let Some(text_bytes) = text_start
        .checked_add(text_len)
        .and_then(|text_end| blob.get(text_start..text_end))
    else {
        log::warn!(
            "attributedBody: length {text_len} runs past the blob end, {} bytes available",
            blob.len().saturating_sub(text_start)
        );
        return None;
    };
    String::from_utf8(text_bytes.to_vec()).ok().or_else(|| {
        log::warn!("attributedBody: text is not valid UTF-8, using lossy conversion");
        Some(String::from_utf8_lossy(text_bytes).into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a blob with the real typedstream length encoding.
    fn build_test_blob(text: &str) -> Vec<u8> {
        crate::test_fixture::attributed_body(text)
    }

    #[test]
    fn test_extract_ascii() {
        let blob = build_test_blob("Hello, world!");
        assert_eq!(
            extract_text_from_attributed_body(&blob),
            Some("Hello, world!".to_string())
        );
    }

    #[test]
    fn test_extract_empty_string() {
        let blob = build_test_blob("");
        assert_eq!(
            extract_text_from_attributed_body(&blob),
            Some(String::new())
        );
    }

    #[test]
    fn test_extract_emoji() {
        let blob = build_test_blob("Hey 🎉🎊");
        assert_eq!(
            extract_text_from_attributed_body(&blob),
            Some("Hey 🎉🎊".to_string())
        );
    }

    #[test]
    fn test_extract_japanese() {
        let blob = build_test_blob("日本語メッセージ");
        assert_eq!(
            extract_text_from_attributed_body(&blob),
            Some("日本語メッセージ".to_string())
        );
    }

    #[test]
    fn test_extract_accented() {
        let blob = build_test_blob("café au lait");
        assert_eq!(
            extract_text_from_attributed_body(&blob),
            Some("café au lait".to_string())
        );
    }

    #[test]
    fn test_extract_no_nsstring_marker() {
        let blob = b"some random bytes without the marker";
        assert_eq!(extract_text_from_attributed_body(blob), None);
    }

    #[test]
    fn test_extract_truncated_blob() {
        let mut blob = Vec::new();
        blob.extend_from_slice(b"\x04\x0bNSString\x01\x94\x84\x01");
        assert_eq!(extract_text_from_attributed_body(&blob), None);
    }

    #[test]
    fn test_extract_empty_blob() {
        assert_eq!(extract_text_from_attributed_body(&[]), None);
    }

    #[test]
    fn test_extract_text_of_128_bytes() {
        let text = "a".repeat(128);
        assert_eq!(
            extract_text_from_attributed_body(&build_test_blob(&text)),
            Some(text)
        );
    }

    #[test]
    fn test_extract_text_of_300_bytes() {
        let text = "b".repeat(300);
        assert_eq!(
            extract_text_from_attributed_body(&build_test_blob(&text)),
            Some(text)
        );
    }

    #[test]
    fn test_extract_text_of_70000_bytes() {
        let text = "c".repeat(70_000);
        assert_eq!(
            extract_text_from_attributed_body(&build_test_blob(&text)),
            Some(text)
        );
    }

    #[test]
    fn test_extract_multibyte_text_over_128_bytes() {
        let text = "日本語🎉".repeat(30);
        assert!(text.len() > 128);
        assert_eq!(
            extract_text_from_attributed_body(&build_test_blob(&text)),
            Some(text)
        );
    }

    #[test]
    fn test_extract_text_of_127_bytes() {
        let text = "d".repeat(127);
        assert_eq!(
            extract_text_from_attributed_body(&build_test_blob(&text)),
            Some(text)
        );
    }

    #[test]
    fn test_extract_text_at_the_i16_boundary() {
        for len in [32_767, 32_768] {
            let text = "e".repeat(len);
            assert_eq!(
                extract_text_from_attributed_body(&build_test_blob(&text)),
                Some(text)
            );
        }
    }

    #[test]
    fn test_extract_rejects_the_0x2d_type_byte() {
        let mut blob = build_test_blob("hello");
        let type_pos = blob.len() - b"hello".len() - 2 - b"\x86\x84\x02iI".len();
        assert_eq!(blob[type_pos], 0x2b);
        blob[type_pos] = 0x2d;
        assert_eq!(extract_text_from_attributed_body(&blob), None);
    }

    #[test]
    fn test_extract_rejects_invalid_length_bytes() {
        for first in [0x80_u8, 0x83, 0xff] {
            let mut blob = build_test_blob("hello");
            let len_pos = blob.len() - b"hello".len() - 1 - b"\x86\x84\x02iI".len();
            assert_eq!(blob[len_pos], 5);
            blob[len_pos] = first;
            assert_eq!(extract_text_from_attributed_body(&blob), None);
        }
    }

    #[test]
    fn test_extract_rejects_a_negative_length() {
        let mut blob = build_test_blob(&"f".repeat(200));
        let len_pos = blob.len() - 200 - 3 - b"\x86\x84\x02iI".len();
        assert_eq!(blob[len_pos], 0x81);
        blob[len_pos + 1..len_pos + 3].copy_from_slice(&(-5_i16).to_le_bytes());
        assert_eq!(extract_text_from_attributed_body(&blob), None);
    }

    #[test]
    fn test_extract_rejects_a_length_past_the_blob_end() {
        let mut blob = build_test_blob("0123456789");
        let len_pos = blob.len() - 10 - 1 - b"\x86\x84\x02iI".len();
        assert_eq!(blob[len_pos], 10);
        blob[len_pos] = 50;
        assert_eq!(extract_text_from_attributed_body(&blob), None);
    }
}
