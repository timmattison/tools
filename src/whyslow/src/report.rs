//! What the binary prints, and the exit code that it gives.

use faulte::duration::Span;

use crate::machine::Machine;

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
#[must_use]
pub fn report(_machine: &dyn Machine, _interval: Span) -> Outcome {
    Outcome {
        stdout: vec![],
        stderr: None,
        code: 0,
    }
}
