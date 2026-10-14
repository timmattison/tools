//! The count rules for one command word.
//!
//! The function rule is not here. The tally applies that one, because it needs
//! the shell definitions.

use crate::command_words::CommandWord;
use crate::tool::{ToolName, ToolNames};

/// The separator of the components of a path.
const PATH_SEPARATOR: char = '/';

/// The directory that cargo builds into. A path with this component is a
/// development build.
const TARGET_DIRECTORY: &str = "target";

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
    if let Some(tool) = tools.get(&word.word) {
        return vec![Run::Use(tool.clone())];
    }
    if let Some((directory, name)) = word.word.rsplit_once(PATH_SEPARATOR) {
        return path_run(directory, name, tools).into_iter().collect();
    }
    Vec::new()
}

/// Gives the run of a command word that holds a path. `directory` is the text
/// before the last separator. `name` is the text after it.
fn path_run(directory: &str, name: &str, tools: &ToolNames) -> Option<Run> {
    let tool = tools.get(name)?.clone();
    if directory
        .split(PATH_SEPARATOR)
        .any(|component| component == TARGET_DIRECTORY)
    {
        Some(Run::Development(tool))
    } else {
        Some(Run::Use(tool))
    }
}
