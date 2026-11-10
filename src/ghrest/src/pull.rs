use std::fmt;

use serde::Deserialize;

/// The number of one pull request.
///
/// A newtype keeps a pull request number apart from other integers. It reads
/// from a bare JSON number and it displays with a leading `#`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(transparent)]
pub struct PullNumber(u64);

impl PullNumber {
    /// Wraps the number of a pull request.
    #[must_use]
    pub const fn new(number: u64) -> Self {
        Self(number)
    }

    /// The number as a plain integer.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for PullNumber {
    /// Writes the number in the form that GitHub shows, for example `#547`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_number_displays_with_a_hash() {
        assert_eq!(PullNumber::new(547).to_string(), "#547");
    }

    #[test]
    fn pull_number_reads_from_a_bare_json_number() {
        let number: PullNumber = serde_json::from_str("548").unwrap();
        assert_eq!(number, PullNumber::new(548));
        assert_eq!(number.get(), 548);
    }
}
