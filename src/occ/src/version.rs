//! The version of a running Claude Code binary.
//!
//! Claude Code installs each release as a single executable named for its
//! version (`~/.local/share/claude/versions/2.1.232`). macOS records the
//! basename of the executed file as the process accounting name, so a running
//! session reports its own version through the kernel. That name survives the
//! deletion of the version file it came from, which is why an upgrade that
//! prunes old releases does not blind this tool to the processes still running
//! them.

use std::cmp::Ordering;

/// A parsed Claude Code version, ordered oldest to newest.
///
/// The type is a newtype over the release numbering rather than over a string:
/// two versions compare by their numeric components, so `2.1.99` correctly
/// sorts before `2.1.232` where a string comparison would not.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClaudeVersion {
    /// The version exactly as the operating system reported it.
    text: String,
    /// The dot-separated numeric components, most significant first.
    components: Vec<u64>,
}

impl ClaudeVersion {
    /// Parses a version made of one or more dot-separated decimal components.
    ///
    /// Returns `None` for any other shape. Parsing fails closed on purpose: the
    /// accounting name of an arbitrary process is an arbitrary string, and this
    /// parse is the test that decides whether a name is a version at all.
    ///
    /// # Examples
    ///
    /// ```
    /// use occ::ClaudeVersion;
    ///
    /// assert!(ClaudeVersion::parse("2.1.232").is_some());
    /// assert!(ClaudeVersion::parse("ugrep").is_none());
    /// ```
    #[must_use]
    pub fn parse(_text: &str) -> Option<Self> {
        None
    }

    /// The version as the operating system reported it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Display for ClaudeVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl Ord for ClaudeVersion {
    /// Compares component by component, treating a missing component as zero.
    ///
    /// The zero fill makes `2.1` older than `2.1.1` and equal to `2.1.0`, which
    /// is what a release numbering means.
    fn cmp(&self, _other: &Self) -> Ordering {
        Ordering::Equal
    }
}

impl PartialOrd for ClaudeVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::ClaudeVersion;

    fn version(text: &str) -> ClaudeVersion {
        ClaudeVersion::parse(text).expect("test version should parse")
    }

    #[test]
    fn parses_a_three_component_release() {
        assert_eq!(version("2.1.232").as_str(), "2.1.232");
    }

    #[test]
    fn rejects_names_that_are_not_versions() {
        // The accounting name of a process spawned by Claude Code is an
        // ordinary program name, and must not be mistaken for a version.
        for name in ["ugrep", "claude", "", "2.1.", ".1.2", "2..1", "2.1.x", "v2.1.2", "+2.1.2"] {
            assert!(
                ClaudeVersion::parse(name).is_none(),
                "{name:?} must not parse as a version"
            );
        }
    }

    #[test]
    fn orders_numerically_not_lexically() {
        // The bug this test exists to prevent: as strings, "2.1.99" > "2.1.232".
        assert!(version("2.1.99") < version("2.1.232"));
        assert!(version("2.1.9") < version("2.1.10"));
        assert!(version("2.1.196") < version("2.1.197"));
    }

    #[test]
    fn orders_across_major_and_minor_components() {
        assert!(version("1.9.9") < version("2.0.0"));
        assert!(version("2.0.9") < version("2.1.0"));
    }

    #[test]
    fn treats_a_missing_component_as_zero() {
        assert_eq!(version("2.1"), version("2.1.0"));
        assert!(version("2.1") < version("2.1.1"));
    }

    #[test]
    fn sorts_a_real_release_range_oldest_first() {
        let mut found = ["2.1.232", "2.1.196", "2.1.99", "2.1.210", "2.1.204"]
            .map(version)
            .to_vec();
        found.sort();
        let ordered: Vec<&str> = found.iter().map(ClaudeVersion::as_str).collect();
        assert_eq!(
            ordered,
            ["2.1.99", "2.1.196", "2.1.204", "2.1.210", "2.1.232"]
        );
    }
}
