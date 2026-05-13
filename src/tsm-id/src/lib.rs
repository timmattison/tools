//! Session ID type for tsm.
//!
//! A [`SessionId`] is a 128-bit identifier represented as exactly 32 lowercase
//! hexadecimal characters. This slice covers only the random-construction path;
//! the deterministic SHA-256 path will be added in a later issue.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

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
        // Stub: returns an empty string so tests fail on behavior, not symbol resolution.
        Self(String::new())
    }

    /// Try to construct a [`SessionId`] from a pre-existing hex string.
    ///
    /// The input must be exactly 32 characters long and consist solely of
    /// lowercase hex characters (`0-9`, `a-f`).
    pub fn from_hex(s: &str) -> Result<Self, SessionIdError> {
        // Stub: accept anything so validation tests fail on behavior.
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

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD_HEX: &str = "0123456789abcdef0123456789abcdef";

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
}
