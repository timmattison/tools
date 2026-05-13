//! Session ID type for tsm.
//!
//! A [`SessionId`] is a 128-bit identifier represented as exactly 32 lowercase
//! hexadecimal characters. There are two construction paths:
//!
//! - [`SessionId::random`] generates a fresh random id from 16 random bytes.
//! - [`session_id_from_tuple`] derives a stable id from a Zellij
//!   (session-name, tab-name, pane-ordinal) tuple by length-prefixed canonical
//!   encoding + SHA-256 truncated to its first 16 bytes.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tsm_tuple::Tuple;

/// Number of hex characters in a [`SessionId`] (16 bytes * 2).
const SESSION_ID_HEX_LEN: usize = 32;

/// Errors produced when constructing a [`SessionId`] from an existing string.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionIdError {
    /// The input was not exactly 32 characters long.
    #[error("session id must be exactly {expected} characters, got {actual}")]
    WrongLength {
        /// Expected character count.
        expected: usize,
        /// Actual character count.
        actual: usize,
    },

    /// The input contained an uppercase hex character; only lowercase is allowed.
    #[error("session id must be lowercase hex; found uppercase character {0:?}")]
    UppercaseHex(char),

    /// The input contained a character outside `[0-9a-f]`.
    #[error("session id contains non-hex character {0:?}")]
    NonHexCharacter(char),
}

/// A 128-bit session identifier rendered as 32 lowercase hex characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// Construct a [`SessionId`] from 16 random bytes, hex-encoded as 32
    /// lowercase characters.
    pub fn random() -> Self {
        let bytes: [u8; 16] = rand::random();
        Self(hex::encode(bytes))
    }

    /// Try to construct a [`SessionId`] from a pre-existing hex string.
    ///
    /// The input must be exactly 32 characters long and consist solely of
    /// lowercase hex characters (`0-9`, `a-f`). Uppercase hex is rejected
    /// to keep the canonical representation unambiguous.
    pub fn from_hex(s: &str) -> Result<Self, SessionIdError> {
        let actual = s.chars().count();
        if actual != SESSION_ID_HEX_LEN {
            return Err(SessionIdError::WrongLength {
                expected: SESSION_ID_HEX_LEN,
                actual,
            });
        }
        for c in s.chars() {
            if c.is_ascii_digit() || ('a'..='f').contains(&c) {
                continue;
            }
            if ('A'..='F').contains(&c) {
                return Err(SessionIdError::UppercaseHex(c));
            }
            return Err(SessionIdError::NonHexCharacter(c));
        }
        Ok(Self(s.to_string()))
    }

    /// Borrow the underlying hex string.
    pub fn as_hex(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for SessionId {
    type Err = SessionIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hex(s)
    }
}

impl Serialize for SessionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// Encode a [`Tuple`] into its canonical, length-prefixed byte representation.
///
/// The encoding concatenates, in this fixed order:
///
/// 1. `u32::to_be_bytes(byte_len(session))` followed by the session name's
///    UTF-8 bytes.
/// 2. `u32::to_be_bytes(byte_len(tab))` followed by the tab name's UTF-8
///    bytes.
/// 3. `u32::to_be_bytes(4)` followed by `u32::to_be_bytes(ordinal)`.
///
/// The length prefix is the **byte length** of the following segment, not its
/// character count. This is the input fed into SHA-256 by
/// [`session_id_from_tuple`]; the length prefixes prevent two distinct tuples
/// from ever producing the same hash input by exploiting the fact that
/// `"x" + "\0y"` and `"x\0y" + ""` have the same concatenation.
pub(crate) fn canonical_bytes(tuple: &Tuple) -> Vec<u8> {
    let session = tuple.zellij_session_name.as_ref().as_bytes();
    let tab = tuple.tab_name.as_ref().as_bytes();
    let ordinal = tuple.pane_ordinal_within_tab.value().to_be_bytes();

    // u32 is enough for any realistic session or tab name. Names with more
    // than 4 GiB of UTF-8 are vanishingly unlikely to exist on a Zellij
    // tab bar, so we treat overflow as a programmer error.
    let session_len = u32::try_from(session.len()).expect("session name fits in u32");
    let tab_len = u32::try_from(tab.len()).expect("tab name fits in u32");
    let ordinal_len: u32 = u32::try_from(ordinal.len()).expect("4 bytes fits in u32");

    let mut out =
        Vec::with_capacity(4 + session.len() + 4 + tab.len() + 4 + ordinal.len());
    out.extend_from_slice(&session_len.to_be_bytes());
    out.extend_from_slice(session);
    out.extend_from_slice(&tab_len.to_be_bytes());
    out.extend_from_slice(tab);
    out.extend_from_slice(&ordinal_len.to_be_bytes());
    out.extend_from_slice(&ordinal);
    out
}

/// Derive a deterministic [`SessionId`] from a Zellij tuple.
///
/// The same tuple always yields the same id; two tuples that differ in any
/// field yield different ids with overwhelming probability.
///
/// The id is the first 16 bytes of `SHA-256(canonical_bytes(&tuple))`, hex
/// encoded as 32 lowercase characters.
pub fn session_id_from_tuple(tuple: &Tuple) -> SessionId {
    let bytes = canonical_bytes(tuple);
    let digest = Sha256::new().chain_update(&bytes).finalize();
    // Take the leading 128 bits (16 bytes) of the SHA-256 output.
    let hex = hex::encode(&digest.as_slice()[..16]);
    // The hex crate emits lowercase 0-9a-f exclusively, so the SessionId
    // invariants (length 32, lowercase hex only) are satisfied by
    // construction. Re-validate through from_hex to keep all SessionId
    // values flowing through the same gate and to surface a bug
    // immediately if hex::encode ever changed its contract.
    SessionId::from_hex(&hex).expect("sha256 truncated to 16 bytes is always valid hex")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsm_tuple::{PaneOrdinal, TabName, Tuple, ZellijSessionName};

    const GOOD_HEX: &str = "0123456789abcdef0123456789abcdef";

    /// Build a [`Tuple`] from raw strings + ordinal for KAT fixtures.
    fn make_tuple(session: &str, tab: &str, ordinal: u32) -> Tuple {
        Tuple {
            zellij_session_name: ZellijSessionName::from(session.to_string()),
            tab_name: TabName::from(tab.to_string()),
            pane_ordinal_within_tab: PaneOrdinal::from(ordinal),
        }
    }

    #[test]
    fn random_produces_32_chars() {
        let id = SessionId::random();
        assert_eq!(id.as_hex().chars().count(), SESSION_ID_HEX_LEN);
    }

    #[test]
    fn random_produces_lowercase_hex_only() {
        let id = SessionId::random();
        for c in id.as_hex().chars() {
            assert!(
                c.is_ascii_digit() || ('a'..='f').contains(&c),
                "char {c:?} is not lowercase hex"
            );
        }
    }

    #[test]
    fn two_random_calls_differ() {
        let a = SessionId::random();
        let b = SessionId::random();
        assert_ne!(a, b, "two random session ids should not collide");
    }

    #[test]
    fn from_hex_accepts_good_input() {
        let id = SessionId::from_hex(GOOD_HEX).expect("good hex must parse");
        assert_eq!(id.as_hex(), GOOD_HEX);
    }

    #[test]
    fn from_hex_rejects_empty_string() {
        let err = SessionId::from_hex("").expect_err("empty must fail");
        assert!(matches!(err, SessionIdError::WrongLength { actual: 0, .. }));
    }

    #[test]
    fn from_hex_rejects_31_chars() {
        let s = "0123456789abcdef0123456789abcde";
        let err = SessionId::from_hex(s).expect_err("31 chars must fail");
        assert!(matches!(
            err,
            SessionIdError::WrongLength { actual: 31, .. }
        ));
    }

    #[test]
    fn from_hex_rejects_33_chars() {
        let s = "0123456789abcdef0123456789abcdef0";
        let err = SessionId::from_hex(s).expect_err("33 chars must fail");
        assert!(matches!(
            err,
            SessionIdError::WrongLength { actual: 33, .. }
        ));
    }

    #[test]
    fn from_hex_rejects_uppercase() {
        let s = "0123456789ABCDEF0123456789abcdef";
        let err = SessionId::from_hex(s).expect_err("uppercase must fail");
        assert!(matches!(err, SessionIdError::UppercaseHex(_)));
    }

    #[test]
    fn from_hex_rejects_non_hex_char() {
        let s = "0123456789abcdeg0123456789abcdef";
        let err = SessionId::from_hex(s).expect_err("non-hex must fail");
        assert!(matches!(err, SessionIdError::NonHexCharacter('g')));
    }

    #[test]
    fn serde_round_trip() {
        let id = SessionId::from_hex(GOOD_HEX).expect("good hex must parse");
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{GOOD_HEX}\""));
        let back: SessionId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, id);
    }

    #[test]
    fn display_matches_as_hex() {
        let id = SessionId::from_hex(GOOD_HEX).expect("good hex must parse");
        assert_eq!(format!("{id}"), id.as_hex());
    }

    #[test]
    fn from_str_agrees_with_from_hex() {
        let via_from_str: SessionId = GOOD_HEX.parse().expect("parse");
        let via_from_hex = SessionId::from_hex(GOOD_HEX).expect("from_hex");
        assert_eq!(via_from_str, via_from_hex);
    }

    // ---------- Known-answer fixtures for session_id_from_tuple ----------
    //
    // Expected hex strings below were computed independently with Python
    // (hashlib.sha256 + struct.pack('>I', ...)) against the canonical
    // length-prefixed encoding spelled out in `canonical_bytes`. They are
    // pasted as literals here so the test does not self-validate against the
    // same code under test.

    #[test]
    fn kat_my_session_main_zero() {
        let t = make_tuple("my-session", "main", 0);
        assert_eq!(
            session_id_from_tuple(&t).as_hex(),
            "f429a77343e6ee531923b1e054435dcc",
        );
    }

    #[test]
    fn kat_dev_logs_three() {
        let t = make_tuple("dev", "logs", 3);
        assert_eq!(
            session_id_from_tuple(&t).as_hex(),
            "991dc61a48f0b5f10dcb980cc0a80e41",
        );
    }

    #[test]
    fn kat_multi_byte_utf8_tab_name() {
        // "日本語" — three CJK chars, three bytes each in UTF-8.
        let t = make_tuple("workshop", "日本語", 1);
        assert_eq!(
            session_id_from_tuple(&t).as_hex(),
            "7296beed29bbd436464232eb264edf56",
        );
    }

    #[test]
    fn kat_embedded_nul_in_tab_name() {
        // Embedded NUL in the tab name MUST NOT terminate the string when
        // hashing — UTF-8 bytes are passed wholesale, length-prefixed.
        let t = make_tuple("ci", "tab\0null", 0);
        assert_eq!(
            session_id_from_tuple(&t).as_hex(),
            "e75a8cabfe8d72640b3b16c95a628bf4",
        );
    }

    #[test]
    fn kat_empty_session_and_tab() {
        // Zero-length data segments must still hash, with the length-prefix
        // being four zero bytes. This guards against accidental
        // empty-string short-circuiting.
        let t = make_tuple("", "", 0);
        assert_eq!(
            session_id_from_tuple(&t).as_hex(),
            "56ae268ef08d7137cef01fcd73902eda",
        );
    }

    #[test]
    fn length_prefix_prevents_boundary_collision() {
        // Without length-prefixing, T_A and T_B would produce identical
        // concatenated bytes: "x" + "\0y" == "x\0y" + "". The whole point of
        // the u32-BE length prefix is to make their canonical encodings
        // (and therefore their session ids) differ. This is exactly the
        // example from the PRD.
        let a = make_tuple("x", "\u{0}y", 0);
        let b = make_tuple("x\u{0}y", "", 0);
        assert_ne!(
            session_id_from_tuple(&a),
            session_id_from_tuple(&b),
            "length-prefix must prevent boundary collisions"
        );
    }

    #[test]
    fn session_id_from_tuple_is_deterministic() {
        let t = make_tuple("repeat", "tab", 42);
        let first = session_id_from_tuple(&t);
        let second = session_id_from_tuple(&t);
        assert_eq!(first, second, "the same tuple must always hash to the same id");
    }

    #[test]
    fn canonical_bytes_matches_hand_computed_layout() {
        // Hand-built reference for ("a", "bc", 7):
        //   [0,0,0,1] 'a' [0,0,0,2] 'b' 'c' [0,0,0,4] [0,0,0,7]
        let t = make_tuple("a", "bc", 7);
        let expected: Vec<u8> = vec![
            0, 0, 0, 1, b'a', 0, 0, 0, 2, b'b', b'c', 0, 0, 0, 4, 0, 0, 0, 7,
        ];
        assert_eq!(canonical_bytes(&t), expected);
    }
}
