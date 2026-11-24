//! The short name of this host, as `hostname -s` gives it.

use std::fmt;

/// The name that stands for a host with no name.
pub const UNKNOWN_HOST: &str = "unknown";

/// The short name of a host: its name before the first `.`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostName(String);

impl HostName {
    /// Makes a short name from the raw bytes that `gethostname` gives.
    ///
    /// A byte that is not UTF-8 becomes the replacement character. The name
    /// ends before the first `.`. An empty name becomes [`UNKNOWN_HOST`].
    #[must_use]
    pub fn from_raw(bytes: &[u8]) -> HostName {
        let text = String::from_utf8_lossy(bytes);
        let short = text.split('.').next().unwrap_or_default();
        if short.is_empty() {
            HostName(UNKNOWN_HOST.to_owned())
        } else {
            HostName(short.to_owned())
        }
    }

    /// The short name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The short name that `raw` gives.
    fn short(raw: &[u8]) -> String {
        HostName::from_raw(raw).to_string()
    }

    #[test]
    fn the_name_ends_before_the_first_dot() {
        assert_eq!(short(b"M4-Pro-Mac-Mini.local"), "M4-Pro-Mac-Mini");
    }

    #[test]
    fn a_name_with_no_dot_stays_whole() {
        assert_eq!(short(b"m1-ts"), "m1-ts");
    }

    #[test]
    fn multi_byte_names_split_on_the_character() {
        assert_eq!(short("café-mac.local".as_bytes()), "café-mac");
        assert_eq!(short("日本語.local".as_bytes()), "日本語");
        assert_eq!(short("🎉.local".as_bytes()), "🎉");
    }

    #[test]
    fn an_empty_name_becomes_unknown() {
        assert_eq!(short(b""), UNKNOWN_HOST);
        assert_eq!(short(b".local"), UNKNOWN_HOST);
    }

    #[test]
    fn a_byte_that_is_not_utf8_becomes_the_replacement_character() {
        assert_eq!(short(b"ab\xffcd.local"), "ab\u{FFFD}cd");
        assert_eq!(HostName::from_raw(b"x.y").as_str(), "x");
    }
}
