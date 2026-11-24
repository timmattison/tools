//! The identifier of a Claude Code session.
//!
//! Claude Code writes one transcript per session, at
//! `~/.claude/projects/<encoded working directory>/<session id>.jsonl`, and a
//! running session records which of them is its own in
//! `~/.claude/sessions/<pid>.json`. That record is read by [`crate::registry`].
//! This module holds only the identifier itself.

/// A Claude Code session id: a canonical UUID.
///
/// Validated on construction so that an id can be used to name a transcript
/// file without re-checking it. Rejecting anything else also guarantees no path
/// separator or traversal sequence rides through to a filesystem lookup.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// Parses a canonical UUID (`8-4-4-4-12` hex digits, case-insensitive).
    ///
    /// # Examples
    ///
    /// ```
    /// use occ::SessionId;
    ///
    /// assert!(SessionId::parse("d3b0d921-f0a1-41fc-b309-c11aa30c1173").is_some());
    /// assert!(SessionId::parse("not-a-session").is_none());
    /// ```
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        /// Hyphen positions in a canonical UUID, and its total length.
        const HYPHEN_POSITIONS: [usize; 4] = [8, 13, 18, 23];
        const UUID_LEN: usize = 36;

        if text.len() != UUID_LEN {
            return None;
        }
        let well_formed = text.bytes().enumerate().all(|(index, byte)| {
            if HYPHEN_POSITIONS.contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
        well_formed.then(|| Self(text.to_string()))
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::SessionId;

    const SESSION_A: &str = "d3b0d921-f0a1-41fc-b309-c11aa30c1173";

    #[test]
    fn accepts_a_canonical_session_id() {
        assert_eq!(
            SessionId::parse(SESSION_A)
                .expect("test id should parse")
                .as_str(),
            SESSION_A
        );
    }

    #[test]
    fn rejects_anything_that_is_not_a_uuid() {
        for text in [
            "",
            "not-a-session",
            "d3b0d921f0a141fcb309c11aa30c1173",
            "../etc/passwd",
            "d3b0d921-f0a1-41fc-b309-c11aa30c117",
            "g3b0d921-f0a1-41fc-b309-c11aa30c1173",
        ] {
            assert!(SessionId::parse(text).is_none(), "{text:?} must not parse");
        }
    }
}
