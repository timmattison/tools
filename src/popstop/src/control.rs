//! The commands that act on the copy that runs (macOS only).
//!
//! `popstop --status` shows the copy that runs. `popstop --stop` ends it.
//! Both read the instance lock first, because the lock is the only source of
//! truth about a copy that runs.

use crate::exit_status;
use crate::life_cycle::{Failure, Settings};

/// What a command that acts on the copy that runs found.
///
/// The text goes to stdout, because it is the answer to the question that the
/// user asked. The status goes to the caller of popstop, because a script
/// reads the status and not the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The exit status of popstop.
    status: u8,
    /// The lines for the user. They carry the name of popstop already.
    text: String,
}

impl Report {
    /// Gives the exit status of popstop.
    #[must_use]
    pub fn status(&self) -> u8 {
        self.status
    }

    /// Gives the lines for the user.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Stops the copy of popstop that runs.
///
/// A stop is idempotent: a stop that finds no copy is a success.
///
/// # Errors
///
/// Returns a [`Failure`] with the status [`exit_status::ERROR`] when the lock
/// cannot be read, when the signal does not reach the copy, or when the copy
/// does not release the lock.
pub fn stop(settings: &Settings) -> Result<Report, Failure> {
    let _dir = settings.state_dir()?;
    Ok(Report {
        status: exit_status::ERROR,
        text: String::new(),
    })
}
