//! The reader of this Mac: the one part of `whyslow` that runs a command or
//! calls the kernel.
//!
//! Each read here has a reason to be a command or a call:
//!
//! - `/bin/ps` reads the process table. It is set-user-ID root and carries
//!   the entitlement that gives the owner, the memory, and the CPU time of
//!   every process of every account. [`faulte::machine::macos::run`] starts
//!   it, so the child sheds the inherited git environment.
//! - `host_statistics(HOST_CPU_LOAD_INFO)` gives the CPU ticks. No command
//!   prints them as numbers that a parser can read.
//! - The counters of the virtual memory system, and the swap file, come from
//!   the readers of `faulte`. One reader of each fact keeps the two tools in
//!   agreement.
//! - `proc_pid_rusage` gives `ri_phys_footprint`, the memory that `Activity
//!   Monitor` shows. It counts compressed memory, and `rss` does not. Without
//!   root, the kernel refuses each process of another account, and PID 0.
//!   Measured on 2026-11-15. [`Machine::footprint`] then gives `None`, and
//!   the caller falls back to the resident size. The kernel tells a refusal
//!   (`EPERM`) apart from a process that exited (`ESRCH`), also measured on
//!   2026-11-15. A process that exited holds no memory, so it gives `Some(0)`.
//! - `occ` classifies the Claude Code processes, and `proctree` reads the
//!   arguments of each process.
//!
//! Nothing here is a pure function, so no unit test covers this module. The
//! test `tests/this-mac.rs` reads this Mac and holds the shape of the result.
//! Each rule that this module feeds is a pure function with tests of its own.

use std::collections::{HashMap, HashSet};
use std::mem::MaybeUninit;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use faulte::machine::macos::{read_sysctl, run};
use faulte::pid::Pid;
use faulte::vm::{SwapUsage, VmCounters};
use occ::{classify, gather_processes, Role};

use crate::machine::{Machine, SourceError};
use crate::sample::{self, ProcessRow};
use crate::verdict::{CpuTicks, Pressure};

/// The call that gives the CPU ticks.
const CPU_STATISTICS: &str = "host_statistics(HOST_CPU_LOAD_INFO)";

/// The `sysctl` that gives the memory pressure level.
const PRESSURE_LEVEL: &str = "kern.memorystatus_vm_pressure_level";

/// The `sysctl` that gives the number of processors.
const PROCESSOR_COUNT: &str = "hw.ncpu";

/// The call that gives the load average.
const LOAD_AVERAGE: &str = "getloadavg";

/// The size of a page of memory that `whyslow` takes when `sysconf` gives
/// none. It is the smallest page that any Mac uses, so a size that this
/// number makes is never larger than the truth.
const SMALLEST_PAGE: u64 = 4096;

/// The number of load averages that `getloadavg` fills. `whyslow` asks for
/// the first, which is the average of the last minute.
const LOAD_SAMPLES: usize = 3;

/// This Mac.
///
/// The value holds the origin of its clock, so the readings of
/// [`Machine::clock`] are monotonic times since [`Mac::new`].
pub struct Mac {
    /// The instant that the clock of this reader counts from.
    origin: Instant,
}

impl Mac {
    /// Makes the reader, and starts its clock.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for Mac {
    /// Gives the same value as [`Mac::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// Makes the error of a call that the kernel refused.
fn refused(call: &str, reason: String) -> SourceError {
    SourceError::KernelRead {
        call: call.to_owned(),
        reason,
    }
}

impl Machine for Mac {
    fn process_table(&self) -> Result<Vec<ProcessRow>, SourceError> {
        let output = run(sample::PROGRAM, &sample::arguments(), &sample::ENVIRONMENT)?;
        Ok(sample::parse(&output)?)
    }

    fn cpu_ticks(&self) -> Result<CpuTicks, SourceError> {
        // The kernel counts in units of `integer_t`, and it refuses a count
        // that is smaller than the structure that it fills.
        let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
        // A structure of zeros is a valid value of every field.
        let mut info = MaybeUninit::<libc::host_cpu_load_info>::zeroed();
        #[expect(
            deprecated,
            reason = "libc asks for the mach2 crate, which this workspace does not take. The call is the one entrance to the CPU ticks"
        )]
        // SAFETY: `mach_host_self` gives the port of this host, which is the
        // port that `host_statistics` takes. The pointer names one whole
        // structure of this function, and `count` states its size in the
        // units that the call reads. The call writes no more than that.
        let answer = unsafe {
            libc::host_statistics(
                libc::mach_host_self(),
                libc::HOST_CPU_LOAD_INFO,
                info.as_mut_ptr().cast::<libc::integer_t>(),
                &mut count,
            )
        };
        if answer != libc::KERN_SUCCESS {
            return Err(refused(
                CPU_STATISTICS,
                format!("the call ended with {answer}"),
            ));
        }
        // SAFETY: the call answered with success, and every field that it did
        // not write holds the zero that this function wrote first.
        let info = unsafe { info.assume_init() };
        let tick = |state: libc::c_int| {
            // The states are the four constants of the kernel, which are
            // indices of an array of four. `get` refuses an index that is
            // not there.
            usize::try_from(state)
                .ok()
                .and_then(|index| info.cpu_ticks.get(index))
                .copied()
                .map_or(0, u64::from)
        };
        Ok(CpuTicks {
            user: tick(libc::CPU_STATE_USER),
            system: tick(libc::CPU_STATE_SYSTEM),
            idle: tick(libc::CPU_STATE_IDLE),
            nice: tick(libc::CPU_STATE_NICE),
        })
    }

    fn vm_counters(&self) -> Result<VmCounters, SourceError> {
        Ok(faulte::machine::macos::vm_counters()?)
    }

    fn swap_usage(&self) -> Result<SwapUsage, SourceError> {
        Ok(faulte::machine::macos::swap_usage()?)
    }

    fn pressure(&self) -> Result<Pressure, SourceError> {
        // SAFETY: the pressure level is one `c_int`, which holds no pointer
        // and no padding.
        let level: libc::c_int = unsafe { read_sysctl(PRESSURE_LEVEL) }?;
        Pressure::from_level(level).ok_or(SourceError::UnknownPressure { level })
    }

    fn load(&self) -> Result<f64, SourceError> {
        let mut averages = [0.0_f64; LOAD_SAMPLES];
        // SAFETY: the pointer names an array of three `f64`, and the call
        // writes no more than the one value that it is asked for.
        let answer = unsafe { libc::getloadavg(averages.as_mut_ptr(), 1) };
        if answer < 1 {
            return Err(refused(
                LOAD_AVERAGE,
                format!("the call ended with {answer}"),
            ));
        }
        Ok(averages[0])
    }

    fn processors(&self) -> Result<u32, SourceError> {
        // SAFETY: the count of processors is one `c_int`, which holds no
        // pointer and no padding.
        let count: libc::c_int = unsafe { read_sysctl(PROCESSOR_COUNT) }?;
        u32::try_from(count)
            .ok()
            .filter(|count| *count > 0)
            .ok_or_else(|| {
                refused(
                    PROCESSOR_COUNT,
                    format!("the kernel gave the count {count}, which is not a count"),
                )
            })
    }

    fn page_size(&self) -> u64 {
        // SAFETY: `sysconf` reads one number of the operating system. It takes
        // no pointer and writes no memory of this process.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        u64::try_from(page).unwrap_or(SMALLEST_PAGE)
    }

    fn footprint(&self, pid: Pid) -> Option<u64> {
        // The kernel counts a PID as a signed number. A number that does not
        // fit is not a PID.
        let target = libc::c_int::try_from(pid.get()).ok()?;
        // A structure of zeros is a valid value of every field.
        let mut usage = MaybeUninit::<libc::rusage_info_v4>::zeroed();
        // SAFETY: the pointer names one whole `rusage_info_v4` of this
        // function, which is the structure that the flavor `RUSAGE_INFO_V4`
        // makes the call write. The call writes no more than that.
        let answer = unsafe {
            libc::proc_pid_rusage(target, libc::RUSAGE_INFO_V4, usage.as_mut_ptr().cast())
        };
        if answer != 0 {
            // Read the error number at once, before any other call changes it.
            let exited = std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            return exited.then_some(0);
        }
        // SAFETY: the call answered with success, and every field that it did
        // not write holds the zero that this function wrote first.
        Some(unsafe { usage.assume_init() }.ri_phys_footprint)
    }

    fn claude_pids(&self) -> HashSet<Pid> {
        gather_processes()
            .iter()
            .filter(|fact| {
                matches!(
                    classify(fact),
                    Role::Session | Role::Unreadable | Role::Support(_)
                )
            })
            .map(|fact| Pid::new(fact.pid))
            .collect()
    }

    fn arguments(&self) -> Result<HashMap<Pid, String>, SourceError> {
        let text = proctree::ps_arguments().ok_or(SourceError::NoArguments)?;
        Ok(proctree::parse_arguments(&text)
            .into_iter()
            .map(|(pid, arguments)| (Pid::new(pid.as_u32()), arguments))
            .collect())
    }

    fn holds_git(&self, directory: &Path) -> bool {
        directory.join(".git").symlink_metadata().is_ok()
    }

    fn sleep(&self, how_long: Duration) {
        thread::sleep(how_long);
    }

    fn clock(&self) -> Duration {
        self.origin.elapsed()
    }
}
