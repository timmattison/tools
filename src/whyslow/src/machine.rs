//! The machine that `whyslow` reads, behind one trait.
//!
//! Every fact of this Mac reaches `whyslow` through [`Machine`]: the process
//! table of `ps`, the CPU ticks, the counters of the virtual memory system,
//! the memory pressure, the load, the memory footprint of each process, the
//! Claude Code processes, the arguments of each process, the clock, and the
//! sleep. A test gives each fact as a plain value, so no test reads the real
//! machine. [`observe`] reads the facts in one fixed order and gives
//! [`Facts`].
//!
//! `macos::Mac` is the real reader. It is the one part of `whyslow` that runs
//! a command or calls the kernel. It calls the public readers of `faulte` for
//! the counters of the virtual memory system and for the run of a command, so
//! the two tools agree on each fact.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use faulte::duration::Span;
use faulte::machine::MachineError;
use faulte::pid::Pid;
use faulte::vm::{SwapUsage, VmCounters, VmDelta};

use crate::groups::{group_of, Group};
use crate::sample::{cpu_used, ProcessRow, SampleParseError};
use crate::tally::{tally, Memory, ProcessSample};
use crate::verdict::{CpuTicks, Facts, Pressure};
use crate::worktree::worktree_of;

#[cfg(target_os = "macos")]
pub mod macos;

/// The number of bytes in one KiB, the unit of the `rss` column of `ps`.
const BYTES_PER_KIB: u64 = 1024;

/// The call that a kernel read names when an error of `faulte` is one that
/// `whyslow` never expects.
const UNEXPECTED_CALL: &str = "faulte";

/// The reason why `whyslow` cannot read this Mac.
///
/// Each one of these makes `whyslow` print the reason and exit 2. A summary
/// that is empty because a source failed looks the same as a Mac that is
/// calm, so `whyslow` never prints one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
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
    /// The output of `ps` is not a process table.
    #[error(
        "the output of {} is not a process table: {source}",
        crate::sample::PROGRAM
    )]
    Table {
        /// What the parser refused.
        #[from]
        source: SampleParseError,
    },
    /// A read of the kernel failed.
    #[error("whyslow cannot read {call}: {reason}")]
    KernelRead {
        /// The call and the name that `whyslow` asked the kernel for.
        call: String,
        /// What the operating system said.
        reason: String,
    },
    /// The kernel gave a memory pressure level that `whyslow` does not know.
    #[error("the kernel gave the memory pressure level {level}, which whyslow does not know")]
    UnknownPressure {
        /// The level that the kernel gave.
        level: i32,
    },
    /// `ps` did not run, so the arguments of the compilers are not known.
    #[error("ps did not run, so whyslow cannot read the arguments of the compilers")]
    NoArguments,
    /// The CPU counters of the kernel gave the same value twice.
    #[error("the CPU counters of the kernel did not move over the interval")]
    CpuTicksDidNotMove,
}

impl From<MachineError> for SourceError {
    /// Maps each error of the readers of `faulte` to the error of `whyslow`.
    ///
    /// The message of a `faulte` kernel error says `faulte cannot read`, thus
    /// this function maps the error and does not wrap it. The three other
    /// variants of `faulte` belong to `top`, to its own table, and to signals.
    /// `whyslow` calls none of them, so they never reach this function. If one
    /// does, it becomes a kernel read with the call `faulte` and the text of
    /// the error, and no fact is lost.
    fn from(error: MachineError) -> Self {
        match error {
            MachineError::CommandDidNotStart { program, reason } => {
                Self::CommandDidNotStart { program, reason }
            }
            MachineError::CommandFailed {
                program,
                status,
                message,
            } => Self::CommandFailed {
                program,
                status,
                message,
            },
            MachineError::KernelRead { call, reason } => Self::KernelRead { call, reason },
            other @ (MachineError::TopOutput { .. }
            | MachineError::TableOutput { .. }
            | MachineError::SignalFailed { .. }) => Self::KernelRead {
                call: UNEXPECTED_CALL.to_owned(),
                reason: other.to_string(),
            },
        }
    }
}

/// Everything that `whyslow` reads from the operating system.
///
/// The trait is the whole boundary between the rules of `whyslow` and this
/// Mac. [`observe`] takes the values that these functions give, so a test
/// gives the values itself and reads no real process, no real command, and no
/// real clock.
pub trait Machine {
    /// Reads one row for each process of every account.
    ///
    /// # Errors
    ///
    /// Gives an error when `ps` does not start, ends with a status that is
    /// not success, or writes output that the parser refuses.
    fn process_table(&self) -> Result<Vec<ProcessRow>, SourceError>;

    /// Reads the CPU ticks of all processors, summed.
    ///
    /// # Errors
    ///
    /// Gives an error when the kernel refuses the read.
    fn cpu_ticks(&self) -> Result<CpuTicks, SourceError>;

    /// Reads the counters of the virtual memory system.
    ///
    /// # Errors
    ///
    /// Gives an error when the kernel refuses the read.
    fn vm_counters(&self) -> Result<VmCounters, SourceError>;

    /// Reads the size of the swap file and how much of it is in use.
    ///
    /// # Errors
    ///
    /// Gives an error when the kernel refuses the read.
    fn swap_usage(&self) -> Result<SwapUsage, SourceError>;

    /// Reads the memory pressure level.
    ///
    /// # Errors
    ///
    /// Gives an error when the kernel refuses the read, or gives a level that
    /// `whyslow` does not know.
    fn pressure(&self) -> Result<Pressure, SourceError>;

    /// Reads the load average of the last minute.
    ///
    /// # Errors
    ///
    /// Gives an error when the kernel refuses the read.
    fn load(&self) -> Result<f64, SourceError>;

    /// Reads the number of processors.
    ///
    /// # Errors
    ///
    /// Gives an error when the kernel refuses the read.
    fn processors(&self) -> Result<u32, SourceError>;

    /// Gives the size of a page of memory in bytes.
    fn page_size(&self) -> u64;

    /// Gives `ri_phys_footprint` of a process in bytes, or `None` when the
    /// kernel refuses the read.
    fn footprint(&self, pid: Pid) -> Option<u64>;

    /// Gives each PID that `occ` classifies as a session, an unreadable
    /// session, or a support process of Claude Code.
    fn claude_pids(&self) -> HashSet<Pid>;

    /// Gives the argument text of each process.
    ///
    /// # Errors
    ///
    /// Gives [`SourceError::NoArguments`] when `ps` did not run.
    fn arguments(&self) -> Result<HashMap<Pid, String>, SourceError>;

    /// Tells whether `directory` holds a `.git` entry, as a file or as a
    /// directory.
    fn holds_git(&self, directory: &Path) -> bool;

    /// Waits for `how_long`.
    fn sleep(&self, how_long: Duration);

    /// Gives a monotonic time since an origin of the machine. Only the
    /// difference of two readings has a meaning.
    fn clock(&self) -> Duration;
}

/// Reads this machine over `interval` and gives the facts.
///
/// The order of the reads is the design:
///
/// 1. the counters of the virtual memory system;
/// 2. the first process table, the clock, and the CPU ticks;
/// 3. the sleep of `interval`;
/// 4. the second process table, the clock, and the CPU ticks;
/// 5. the counters of the virtual memory system again;
/// 6. the pressure, the swap file, the load, and the processors;
/// 7. the Claude Code processes and the arguments.
///
/// The clock sits beside each process table, so the window is the time
/// between the two tables. The counters of the virtual memory system bracket
/// the window, so the swap-outs are the change over the window and a little
/// more.
///
/// # Errors
///
/// Gives the error of the first source that fails, and no facts. A summary
/// that a failed source left empty would look like a calm Mac. Gives
/// [`SourceError::CpuTicksDidNotMove`] when the CPU ticks are the same twice,
/// because a percent of nothing has no meaning.
pub fn observe(machine: &dyn Machine, interval: Span) -> Result<Facts, SourceError> {
    let vm_before = machine.vm_counters()?;
    let before = machine.process_table()?;
    let started = machine.clock();
    let ticks_before = machine.cpu_ticks()?;
    machine.sleep(Duration::from(interval));
    let after = machine.process_table()?;
    let finished = machine.clock();
    let ticks_after = machine.cpu_ticks()?;
    let vm_after = machine.vm_counters()?;
    let pressure = machine.pressure()?;
    let swap = machine.swap_usage()?;
    let load = machine.load()?;
    let processors = machine.processors()?;
    let claude = machine.claude_pids();
    let arguments = machine.arguments()?;

    let ticks = CpuTicks::between(ticks_before, ticks_after);
    if ticks.total() == 0 {
        return Err(SourceError::CpuTicksDidNotMove);
    }
    let samples = samples(machine, &before, &after, &claude, &arguments);
    Ok(Facts {
        ticks,
        processors,
        load,
        window: finished.saturating_sub(started),
        pressure,
        swapouts: VmDelta::between(vm_before, vm_after).swapouts,
        compressor_bytes: vm_after.compressor_bytes(machine.page_size()),
        swap,
        groups: tally(&samples),
    })
}

/// Makes one sample for each process of the second table.
///
/// The CPU of a process is the change between the two tables. A process that
/// is in the second table only counts all of its time. A process in the first
/// table only has ended, and it has no sample. A process of the compilers
/// takes its worktree from its arguments, and any other process takes none.
/// The memory is the footprint when the kernel gives one, and the resident
/// size otherwise.
fn samples(
    machine: &dyn Machine,
    before: &[ProcessRow],
    after: &[ProcessRow],
    claude: &HashSet<Pid>,
    arguments: &HashMap<Pid, String>,
) -> Vec<ProcessSample> {
    after
        .iter()
        .zip(cpu_used(before, after))
        .map(|(row, cpu)| {
            let group = group_of(row.name(), claude.contains(&row.pid));
            let worktree = if group == Group::Compilers {
                arguments
                    .get(&row.pid)
                    .and_then(|text| worktree_of(text, |directory| machine.holds_git(directory)))
            } else {
                None
            };
            let memory = machine.footprint(row.pid).map_or_else(
                || Memory::Resident(row.rss_kib.saturating_mul(BYTES_PER_KIB)),
                Memory::Footprint,
            );
            ProcessSample {
                uid: row.uid,
                group,
                worktree,
                cpu,
                memory,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel error keeps its call and its reason, and its message names
    /// `whyslow`. The text of `faulte` would say `faulte cannot read`, which
    /// is the wrong tool for the person who reads it.
    #[test]
    fn a_kernel_read_keeps_its_call_and_says_whyslow() {
        let error = SourceError::from(MachineError::KernelRead {
            call: "hw.ncpu".to_owned(),
            reason: "no".to_owned(),
        });
        assert_eq!(
            error,
            SourceError::KernelRead {
                call: "hw.ncpu".to_owned(),
                reason: "no".to_owned(),
            }
        );
        assert!(error.to_string().starts_with("whyslow cannot read hw.ncpu"));
        assert!(!error.to_string().contains("faulte"));
    }

    /// A command that did not start keeps its program and its reason.
    #[test]
    fn a_command_that_did_not_start_keeps_its_fields() {
        let error = SourceError::from(MachineError::CommandDidNotStart {
            program: "/bin/ps".to_owned(),
            reason: "not found".to_owned(),
        });
        assert_eq!(
            error,
            SourceError::CommandDidNotStart {
                program: "/bin/ps".to_owned(),
                reason: "not found".to_owned(),
            }
        );
    }

    /// A command that failed keeps its program, its status, and its message.
    #[test]
    fn a_command_that_failed_keeps_its_fields() {
        let error = SourceError::from(MachineError::CommandFailed {
            program: "/bin/ps".to_owned(),
            status: "exit status: 1".to_owned(),
            message: "bad".to_owned(),
        });
        assert_eq!(
            error,
            SourceError::CommandFailed {
                program: "/bin/ps".to_owned(),
                status: "exit status: 1".to_owned(),
                message: "bad".to_owned(),
            }
        );
    }

    /// The error of the process table parser converts to the table error.
    #[test]
    fn a_parse_error_of_the_table_becomes_a_table_error() {
        let error = SourceError::from(SampleParseError::NoRows);
        assert_eq!(
            error,
            SourceError::Table {
                source: SampleParseError::NoRows
            }
        );
    }

    /// An error of `faulte` that `whyslow` never expects still becomes a
    /// kernel read with the text of the error, so no fact is lost.
    #[test]
    fn an_unexpected_error_of_faulte_becomes_a_kernel_read_with_its_text() {
        let source = MachineError::SignalFailed {
            call: "kill(1, SIGTERM)".to_owned(),
            reason: "no".to_owned(),
        };
        let text = source.to_string();
        assert_eq!(
            SourceError::from(source),
            SourceError::KernelRead {
                call: "faulte".to_owned(),
                reason: text,
            }
        );
    }
}
