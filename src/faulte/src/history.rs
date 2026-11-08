//! The history of the samples that the view of `faulte tui` shows.
//!
//! The history holds a fixed memory budget of compact frames and gaps. A frame
//! is the ranking and the memory counters of one sample. A gap is a time that
//! the view tried to measure and did not. The module holds no terminal, no
//! clock, and no process. A [`SystemTime`] arrives only as data.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use crate::machine::Observed;
use crate::pid::{Pid, Uid};
use crate::ranking::Ranking;
use crate::render::Accounts;

/// The memory that a history keeps, in bytes.
pub const BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// The most text of one command that the history keeps.
pub const STORED_COMMAND_BYTES: usize = 16 * 1024;

/// The names of an entry of the history. A name never repeats, so a holder of
/// a name can tell that its entry left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryId(u64);

/// The name of a process. A PID can return for another process, so the start
/// time is part of the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessKey {
    /// The PID of the process.
    pub pid: Pid,
    /// The start time of the process, in seconds since the Unix epoch. `None`
    /// for PID 0.
    pub started_at_epoch_secs: Option<u64>,
}

/// One sample, as the sampler hands it over.
pub struct SampledFrame {
    /// What the run read from the machine.
    pub observed: Observed,
    /// The time of the sample.
    pub at: SystemTime,
    /// The PIDs of `faulte` and of its child processes.
    pub own: BTreeSet<Pid>,
}

/// A time that the view tried to measure and did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    /// The time when the gap began.
    pub at: SystemTime,
    /// The length of the time that was lost.
    pub lost: Duration,
    /// Why the time was lost.
    pub reason: String,
}

/// What the sampler gives to the history.
pub enum Update {
    /// A sample.
    Frame(Box<SampledFrame>),
    /// A time with no sample.
    Gap(Gap),
}

/// The compact history. See the module doc.
pub struct History {
    budget: usize,
    accounts: Accounts,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    /// Makes an empty history with the budget [`BUDGET_BYTES`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_budget(BUDGET_BYTES)
    }

    /// Makes an empty history with a budget of `bytes`.
    #[must_use]
    pub fn with_budget(bytes: usize) -> Self {
        Self {
            budget: bytes,
            accounts: Accounts::new(),
        }
    }

    /// Adds an update, and gives its name.
    pub fn add(&mut self, update: Update) -> EntryId {
        let _ = (&update, self.budget);
        EntryId(0)
    }

    /// Tells whether the entry `id` is still in the history.
    #[must_use]
    pub fn contains(&self, _id: EntryId) -> bool {
        false
    }

    /// Gives the name of the oldest entry.
    #[must_use]
    pub fn oldest(&self) -> Option<EntryId> {
        None
    }

    /// Gives the bytes that the history holds.
    #[must_use]
    pub fn bytes_used(&self) -> usize {
        0
    }

    /// Gives the names of the accounts that the frames mentioned.
    #[must_use]
    pub fn accounts(&self) -> &Accounts {
        &self.accounts
    }
}

#[allow(dead_code, reason = "stub")]
fn _unused(_: Uid, _: Ranking) {}
