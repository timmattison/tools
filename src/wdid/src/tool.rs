//! The names of the tools of the workspace.
//!
//! A tool is a binary target of the workspace. These types keep a tool name
//! apart from any other text, such as a shell word.

use std::borrow::Borrow;
use std::collections::BTreeSet;
use std::fmt;

/// The name of a binary target of the workspace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct ToolName(String);

impl ToolName {
    /// Makes a tool name from text.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// Gives the name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Borrow<str> for ToolName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// The set of tool names of the workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolNames(BTreeSet<ToolName>);

impl ToolNames {
    /// Gives the tool name that equals `name`, or `None` when the workspace
    /// has no such tool.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ToolName> {
        self.0.get(name)
    }

    /// Gives the number of tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// True when the set holds no tool.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Goes through the tool names in sorted order.
    pub fn iter(&self) -> impl Iterator<Item = &ToolName> {
        self.0.iter()
    }
}

impl FromIterator<ToolName> for ToolNames {
    fn from_iter<I: IntoIterator<Item = ToolName>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_finds_a_member_and_only_a_member() {
        let names: ToolNames = ["prcp", "cwt"].into_iter().map(ToolName::new).collect();
        assert_eq!(names.get("prcp"), Some(&ToolName::new("prcp")));
        assert_eq!(names.get("ls"), None);
        assert_eq!(names.len(), 2);
        assert!(!names.is_empty());
        assert!(ToolNames::default().is_empty());
    }

    #[test]
    fn iter_gives_sorted_names() {
        let names: ToolNames = ["prcp", "cwt"].into_iter().map(ToolName::new).collect();
        let listed: Vec<&str> = names.iter().map(ToolName::as_str).collect();
        assert_eq!(listed, ["cwt", "prcp"]);
    }

    #[test]
    fn a_tool_name_displays_as_plain_text() {
        let name = ToolName::new("日本語");
        assert_eq!(name.to_string(), "日本語");
        assert_eq!(name.as_str(), "日本語");
    }
}
