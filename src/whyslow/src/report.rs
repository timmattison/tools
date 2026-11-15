//! What the binary prints, and the exit code that it gives.

use faulte::duration::Span;

use crate::machine::{observe, Machine};
use crate::render::render;
use crate::verdict::summarize;

/// The exit code when a source of this Mac failed.
pub const EXIT_SOURCE_FAILED: u8 = 2;

/// What one run of `whyslow` prints and how it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The lines for the standard output.
    pub stdout: Vec<String>,
    /// The message for the error output, when a source failed.
    pub stderr: Option<String>,
    /// The exit code.
    pub code: u8,
}

/// Reads `machine` and gives what the binary prints.
///
/// A source that fails gives its reason and no summary, because a summary with
/// a missing source can look like a quiet Mac.
#[must_use]
pub fn report(machine: &dyn Machine, interval: Span) -> Outcome {
    match observe(machine, interval) {
        Ok(facts) => Outcome {
            stdout: render(&summarize(&facts)),
            stderr: None,
            code: 0,
        },
        Err(error) => Outcome {
            stdout: Vec::new(),
            stderr: Some(format!("whyslow: {error}")),
            code: EXIT_SOURCE_FAILED,
        },
    }
}
