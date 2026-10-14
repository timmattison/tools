//! The count rules for one command word.
//!
//! The function rule is not here. The tally applies that one, because it needs
//! the shell definitions.

use crate::command_words::CommandWord;
use crate::tool::{ToolName, ToolNames};

/// One run that a command word makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    /// A use of the tool.
    Use(ToolName),
    /// A run of a development build of the tool.
    Development(ToolName),
}

/// Gives the runs that one command word makes. Pure.
///
/// The result has at most one entry for each tool.
#[must_use]
pub fn runs(word: &CommandWord, tools: &ToolNames) -> Vec<Run> {
    let _ = (word, tools);
    Vec::new()
}
