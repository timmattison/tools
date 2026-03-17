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
    const SHORT_STRING: u8 = 0x2b;
    const LONG_STRING: u8 = 0x2d;

    let marker_pos = blob
        .windows(NSSTRING_MARKER.len())
        .position(|w| w == NSSTRING_MARKER)?;

    let after_marker = marker_pos + NSSTRING_MARKER.len();

    if blob.len() < after_marker + HEADER.len() + 2 {
        log::warn!("attributedBody too short after NSString marker (len={})", blob.len());
        return None;
    }

    let header_start = after_marker;
    if blob[header_start..header_start + HEADER.len()] != *HEADER {
        log::warn!(
            "attributedBody: unexpected header bytes at offset {}: {:02x?}",
            header_start,
            &blob[header_start..std::cmp::min(header_start + 4, blob.len())]
        );
        return None;
    }

    let type_pos = header_start + HEADER.len();
    let type_byte = blob[type_pos];

    let (text_start, text_len) = match type_byte {
        SHORT_STRING => {
            let len_pos = type_pos + 1;
            if len_pos >= blob.len() {
                return None;
            }
            let len = blob[len_pos] as usize;
            (len_pos + 1, len)
        }
        LONG_STRING => {
            let len_start = type_pos + 1;
            if len_start + 4 > blob.len() {
                return None;
            }
            let len = u32::from_le_bytes([
                blob[len_start],
                blob[len_start + 1],
                blob[len_start + 2],
                blob[len_start + 3],
            ]) as usize;
            (len_start + 4, len)
        }
        _ => {
            log::warn!(
                "attributedBody: unknown type byte 0x{:02x} at offset {}",
                type_byte,
                type_pos
            );
            return None;
        }
    };

    if text_start + text_len > blob.len() {
        log::warn!("attributedBody: text extends past blob end");
        let available = &blob[text_start..];
        return String::from_utf8(available.to_vec()).ok();
    }

    let text_bytes = &blob[text_start..text_start + text_len];
    String::from_utf8(text_bytes.to_vec()).ok().or_else(|| {
        log::warn!("attributedBody: text is not valid UTF-8, using lossy conversion");
        Some(String::from_utf8_lossy(text_bytes).into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic attributedBody blob for testing.
    fn build_test_blob(text: &str, long: bool) -> Vec<u8> {
        let mut blob = Vec::new();
        blob.extend_from_slice(b"\x04\x0bstreamtyped\x81\xe8\x03\x84\x01@\x84\x84\x84\x12NSAttributedString\x00\x84\x84\x08NSObject\x00\x85\x92\x84\x84\x84\x08");
        blob.extend_from_slice(b"NSString");
        blob.extend_from_slice(&[0x01, 0x94, 0x84, 0x01]);
        let text_bytes = text.as_bytes();
        if long {
            blob.push(0x2d);
            blob.extend_from_slice(&(text_bytes.len() as u32).to_le_bytes());
        } else {
            blob.push(0x2b);
            #[expect(clippy::cast_possible_truncation, reason = "test helper, length always < 256")]
            blob.push(text_bytes.len() as u8);
        }
        blob.extend_from_slice(text_bytes);
        blob.extend_from_slice(b"\x86\x84\x02iI");
        blob
    }

    #[test]
    fn test_extract_ascii() {
        let blob = build_test_blob("Hello, world!", false);
        assert_eq!(extract_text_from_attributed_body(&blob), Some("Hello, world!".to_string()));
    }

    #[test]
    fn test_extract_empty_string() {
        let blob = build_test_blob("", false);
        assert_eq!(extract_text_from_attributed_body(&blob), Some(String::new()));
    }

    #[test]
    fn test_extract_emoji() {
        let blob = build_test_blob("Hey 🎉🎊", false);
        assert_eq!(extract_text_from_attributed_body(&blob), Some("Hey 🎉🎊".to_string()));
    }

    #[test]
    fn test_extract_japanese() {
        let blob = build_test_blob("日本語メッセージ", false);
        assert_eq!(extract_text_from_attributed_body(&blob), Some("日本語メッセージ".to_string()));
    }

    #[test]
    fn test_extract_accented() {
        let blob = build_test_blob("café au lait", false);
        assert_eq!(extract_text_from_attributed_body(&blob), Some("café au lait".to_string()));
    }

    #[test]
    fn test_extract_long_string() {
        let long_text = "a".repeat(300);
        let blob = build_test_blob(&long_text, true);
        assert_eq!(extract_text_from_attributed_body(&blob), Some(long_text));
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
}
