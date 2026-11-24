//! The name of a sidecar.

use std::fmt;

/// The rule for a sidecar name, in words. The text of [`NameError`] holds it.
pub const NAME_RULE: &str = "lowercase letters, digits, hyphens and dots, with a letter or a digit at each end and between two marks";

/// A valid sidecar name. The field is private, so each value passed the rule.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SidecarName(String);

/// The error for a text that is not a sidecar name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameError {
    name: String,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "\"{}\" is not a sidecar name. A name holds only {NAME_RULE}.",
            self.name
        )
    }
}

impl std::error::Error for NameError {}

impl SidecarName {
    /// Checks `raw` against the name rule.
    ///
    /// # Errors
    ///
    /// Returns [`NameError`] when `raw` breaks the rule.
    pub fn parse(raw: &str) -> Result<Self, NameError> {
        if is_valid(raw) {
            Ok(Self(raw.to_string()))
        } else {
            Err(NameError {
                name: raw.to_string(),
            })
        }
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// True when `raw` matches `^[a-z0-9]+(?:[.-][a-z0-9]+)*$`.
fn is_valid(raw: &str) -> bool {
    let is_core = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let is_mark = |c: char| c == '.' || c == '-';
    let mut previous_was_mark = true;
    for c in raw.chars() {
        if is_core(c) {
            previous_was_mark = false;
        } else if is_mark(c) && !previous_was_mark {
            previous_was_mark = true;
        } else {
            return false;
        }
    }
    !raw.is_empty() && !previous_was_mark
}

impl fmt::Display for SidecarName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_names() {
        for raw in [
            "testdata",
            "pleasantville.cc",
            "remotesensing.fyi",
            "a-b.c",
            "0",
        ] {
            let name = SidecarName::parse(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert_eq!(name.as_str(), raw);
            assert_eq!(name.to_string(), raw);
        }
    }

    #[test]
    fn refuses_invalid_names() {
        for raw in [
            "..", ".a", "a.", "a..b", "a.-b", "-a", "A", "Bad_Name", "a_b", "", "日本", "a b",
        ] {
            assert!(SidecarName::parse(raw).is_err(), "{raw:?} must be refused");
        }
    }

    #[test]
    fn error_text_holds_the_name_and_the_rule() {
        let error = SidecarName::parse("Bad_Name").unwrap_err().to_string();
        assert_eq!(
            error,
            format!("\"Bad_Name\" is not a sidecar name. A name holds only {NAME_RULE}.")
        );
    }
}
