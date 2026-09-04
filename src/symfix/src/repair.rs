//! The repair: which new target to try, whether the operating system will
//! resolve it, and how the new link takes the place of the old one.

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::Path;

use crate::Options;

/// Which strategy built a new target. It decides the message the run prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// The new target is the old one with a string in front of it.
    Prepend,
    /// The new target is the old one with a prefix taken off the front.
    Remove,
}

impl Strategy {
    /// The phrase that names this strategy in the report.
    ///
    /// The report says `Fixed symlink by {phrase}` for a repair that happened,
    /// so the wording of a strategy lives with the strategy and not at the line
    /// that prints it.
    #[must_use]
    pub fn phrase(self) -> &'static str {
        match self {
            Self::Prepend => "prepending",
            Self::Remove => "removing prefix",
        }
    }
}

/// A repair the tool can make to one broken link.
#[derive(Debug, Clone)]
pub struct Repair {
    /// The exact bytes that go into the new link.
    pub target: OsString,
    /// The strategy that built [`Repair::target`].
    pub strategy: Strategy,
}

/// Builds the first candidate target that resolves, or gives back `None`.
pub fn plan(
    _options: &Options,
    _link: &Path,
    _target: &OsStr,
    _err: &mut dyn Write,
) -> Option<Repair> {
    None
}

/// Replaces the symlink at `link` so that it holds `target`, in one step.
///
/// # Errors
///
/// Gives back the error the operating system raised while it made the new link
/// or while the new link took the place of the old one.
pub fn apply(_link: &Path, _target: &OsStr) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symfix cannot replace a symlink yet",
    ))
}
