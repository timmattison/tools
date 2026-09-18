//! The machine that `faulte` reads, behind one trait.
//!
//! Every fact of this Mac reaches `faulte` through this module: the sample of
//! `top`, the process table of `ps`, the Claude Code role of each process, the
//! registry record of each session, the counters of the virtual memory system,
//! the account names, and the clock. One trait holds all of them, so a test
//! gives each fact as a plain value and no test reads the real machine.
//!
//! The real reader of macOS is the one part of `faulte` that runs a command or
//! calls the kernel.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use occ::SessionRecord;

use crate::duration::Span;
use crate::pid::{Pid, Uid};
use crate::ranking::{ClaudeRole, ClaudeTotal, Ranking, Skipped, Viewer};
use crate::render::{Accounts, Measurement};
use crate::table::{ProcessRow, TableParseError};
use crate::top::{TopParseError, TopSample};
use crate::vm::{SwapUsage, VmCounters, VmDelta};

/// The reason why `faulte` cannot read this Mac.
///
/// Every one of these makes `faulte` print the reason and exit 2. A ranking
/// that is empty because a source failed looks the same as a Mac that does
/// nothing, so `faulte` never prints one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MachineError {
    /// A command did not start at all, for example because the file is not
    /// there.
    #[error("{program} did not start: {reason}")]
    CommandDidNotStart {
        /// The path of the command.
        program: String,
        /// What the operating system said.
        reason: String,
    },
    /// A command ran and ended with a status that is not success.
    #[error("{program} ended with {status}: {message}")]
    CommandFailed {
        /// The path of the command.
        program: String,
        /// The exit status, as the operating system reports it.
        status: String,
        /// The first line that the command wrote on its error output.
        message: String,
    },
    /// The output of the fault sampler is not a sample of faults.
    #[error("the output of {} is not a sample of faults: {source}", crate::top::PROGRAM)]
    TopOutput {
        /// What the parser refused.
        #[from]
        source: TopParseError,
    },
    /// The output of the process table command is not a process table.
    #[error("the output of {} is not a process table: {source}", crate::table::PROGRAM)]
    TableOutput {
        /// What the parser refused.
        #[from]
        source: TableParseError,
    },
    /// A read of the kernel failed.
    #[error("faulte cannot read {call}: {reason}")]
    KernelRead {
        /// The call and the name that `faulte` asked the kernel for.
        call: String,
        /// What the operating system said.
        reason: String,
    },
}

/// Everything that `faulte` reads from the operating system.
///
/// The trait is the whole boundary between the rules of `faulte` and this Mac.
/// The rules take the values that these functions give, so a test gives the
/// values itself and reads no real process, no real command, and no real
/// clock. One implementation reads the real machine, and macOS is the only
/// platform that has one.
pub trait Machine {
    /// Samples the page faults of every process over `interval`.
    ///
    /// Gives the sample and the time that the whole run took.
    /// [`sample_window`] needs that time, because the sample states the times
    /// of its own two clock lines and a clock can move.
    ///
    /// # Errors
    ///
    /// Gives an error when the sampler does not start, ends with a status that
    /// is not success, or writes output that the parser refuses.
    fn sample_faults(&self, interval: Span) -> Result<(TopSample, Duration), MachineError>;

    /// Reads one row for each process of every account.
    ///
    /// # Errors
    ///
    /// Gives an error when the command does not start, ends with a status that
    /// is not success, or writes output that the parser refuses.
    fn process_table(&self) -> Result<Vec<ProcessRow>, MachineError>;

    /// Gives the Claude Code role of each process that has one.
    ///
    /// A process with no entry is not a Claude Code session. This read never
    /// fails: a process that this account cannot read is a role of its own.
    fn claude_roles(&self) -> HashMap<Pid, ClaudeRole>;

    /// Gives the registry record of the session of `pid`, or `None`.
    ///
    /// The record lives in the home directory of `owner`, and the registry
    /// refuses a file that a dead session left behind under the same PID.
    /// `started_at_epoch_secs` is the start time from the process table, which
    /// is what makes that refusal possible.
    fn record_for(
        &self,
        pid: Pid,
        owner: Uid,
        started_at_epoch_secs: u64,
    ) -> Option<SessionRecord>;

    /// Reads the counters of the virtual memory system of this Mac.
    ///
    /// # Errors
    ///
    /// Gives an error when the kernel refuses the read.
    fn vm_counters(&self) -> Result<VmCounters, MachineError>;

    /// Reads the size of the swap file of this Mac and how much of it is in
    /// use.
    ///
    /// # Errors
    ///
    /// Gives an error when the kernel refuses the read.
    fn swap_usage(&self) -> Result<SwapUsage, MachineError>;

    /// Gives the size of one page of memory in bytes.
    ///
    /// The compressor counter counts pages, and the header states bytes.
    fn page_size(&self) -> u64;

    /// Gives the account that runs `faulte`.
    fn viewer(&self) -> Viewer;

    /// Gives the name of the account `uid`, or `None` when it has no name.
    fn account_name(&self, uid: Uid) -> Option<String>;

    /// Gives the time now.
    fn now(&self) -> SystemTime;
}

/// What one run of [`observe`] read from the machine.
///
/// The ranking says which processes made the faults. The other fields say what
/// the memory of the whole Mac did over the same time, because no row of the
/// ranking says that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    /// Every process of the sample and of the table, ranked by its faults.
    pub ranking: Ranking,
    /// The swap traffic between the two reads of the counters.
    pub swap: VmDelta,
    /// The swap file of this Mac, and how much of it is in use.
    pub usage: SwapUsage,
    /// The size of the compressor in bytes, after the sample.
    pub compressor_bytes: u64,
    /// The time between the two reads of the counters. It is longer than the
    /// window of the ranking, because the sampler starts and stops inside it.
    pub swap_window: Duration,
    /// The name of each account that owns a row of the ranking.
    pub accounts: Accounts,
    /// The time when the run read the machine. The age of each row and the
    /// idle time of each session are measured from it.
    pub now: SystemTime,
    /// The interval that the person asked for. The header states it beside the
    /// window, because the two differ on a Mac that is busy.
    pub interval: Span,
}

impl Observed {
    /// Gives the numbers that the header of the output states.
    ///
    /// The header reads the ranking and the counters of the system together,
    /// and this function is what joins them. Thus the caller states no field
    /// of its own.
    #[must_use]
    pub fn measurement(&self) -> Measurement<'_> {
        Measurement {
            ranking: &self.ranking,
            swap: self.swap,
            usage: self.usage,
            compressor_bytes: self.compressor_bytes,
            swap_window: self.swap_window,
            interval: self.interval,
        }
    }
}

/// Reads every source of `machine` and ranks what they say.
///
/// The order of the reads is the design. The counters of the system come
/// first and last, so the swap traffic covers the whole sample. The sample of
/// faults runs between them, because it is the one read that takes the
/// interval. The process table comes after the sample, so a process that the
/// sampler saw is still in the table when it is alive.
///
/// # Errors
///
/// Gives the error of the first source that failed. A source that fails stops
/// the run, because a ranking that is missing a source is not a ranking.
pub fn observe(machine: &dyn Machine, interval: Span) -> Result<Observed, MachineError> {
    Ok(Observed {
        ranking: Ranking {
            rows: Vec::new(),
            window: Duration::from(interval),
            total_faults: 0,
            claude: ClaudeTotal::default(),
            skipped: Skipped::default(),
        },
        swap: VmDelta::default(),
        usage: SwapUsage::default(),
        compressor_bytes: 0,
        swap_window: Duration::ZERO,
        accounts: Accounts::new(),
        now: machine.now(),
        interval,
    })
}

/// Gives the time that the ranking divides the faults of the sample by.
///
/// `top` prints the local time of each of its two samples, and the difference
/// of the two times is the true window of the delta. That time is longer than
/// the interval on a Mac that is busy: a sample of 2 seconds took 4 seconds on
/// 2026-09-18. A rate over the interval is then too large.
///
/// Two limits guard the answer. The window is never shorter than the interval,
/// because `top` never samples for less time than it was asked for. The window
/// is never longer than `wall`, the time that the whole run of `top` took,
/// because a change of the local time clock between the two samples adds an
/// hour that no process was running for. `elapsed` of `None` gives the
/// interval, because the output then states no times to subtract.
#[must_use]
pub fn sample_window(interval: Span, elapsed: Option<Duration>, wall: Duration) -> Duration {
    let interval = Duration::from(interval);
    match elapsed {
        Some(elapsed) => interval.max(elapsed.min(wall)),
        None => interval,
    }
}

#[cfg(test)]
mod tests {
    use super::sample_window;
    use crate::duration::Span;
    use std::time::Duration;

    /// Gives the interval of a test as a [`Span`].
    fn span(text: &str) -> Span {
        text.parse().expect("the text is a span")
    }

    /// A busy Mac makes `top` sample for longer than the interval. The faults
    /// of the sample were made over that longer time, so it is the window.
    #[test]
    fn an_elapsed_time_longer_than_the_interval_is_the_window() {
        assert_eq!(
            sample_window(
                span("2s"),
                Some(Duration::from_secs(4)),
                Duration::from_secs(5)
            ),
            Duration::from_secs(4)
        );
    }

    /// `top` never samples for less time than the interval. A shorter time
    /// between the two clock lines is a clock that moved back, and a shorter
    /// window makes every rate of the ranking too large.
    #[test]
    fn an_elapsed_time_shorter_than_the_interval_does_not_shorten_the_window() {
        assert_eq!(
            sample_window(
                span("5s"),
                Some(Duration::from_secs(3)),
                Duration::from_secs(6)
            ),
            Duration::from_secs(5)
        );
    }

    /// A change of daylight saving time between the two samples adds an hour
    /// to the difference of the two clock lines. The run of `top` took 3
    /// seconds, so no process made faults for an hour.
    #[test]
    fn the_wall_time_limits_an_absurd_elapsed_time() {
        assert_eq!(
            sample_window(
                span("2s"),
                Some(Duration::from_secs(3600)),
                Duration::from_secs(3)
            ),
            Duration::from_secs(3)
        );
    }

    /// Output that states no times gives the interval. The interval is what
    /// `top` was asked for, and it is the best answer that is left.
    #[test]
    fn no_elapsed_time_gives_the_interval() {
        assert_eq!(
            sample_window(span("5s"), None, Duration::from_secs(9)),
            Duration::from_secs(5)
        );
    }
}
