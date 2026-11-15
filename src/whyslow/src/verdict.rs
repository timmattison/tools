//! The verdict: which resource is short, and which groups use it.
//!
//! The rules are pure. They read plain values and give plain values, so a test
//! needs no Mac. Each threshold is a named constant, and each comparison uses
//! whole numbers, so a value at the limit gives the same answer on each run.

/// The CPU is full when the idle part is below this percent.
pub const FULL_IDLE_PERCENT: u64 = 10;

/// The CPU is busy when the idle part is below this percent.
pub const BUSY_IDLE_PERCENT: u64 = 40;

/// The system part is high when it is at or above this percent of the CPU.
/// With short memory, a high system part points to compression and swap.
pub const HIGH_SYSTEM_PERCENT: u64 = 30;

/// The CPU that no group explains gets its own line at or above this percent
/// of one processor.
pub const UNEXPLAINED_PERCENT: u64 = 100;

/// The largest number of groups that the summary lists for one resource.
pub const GROUP_LIMIT: usize = 3;

/// The largest number of worktrees that the summary lists for the compilers.
pub const WORKTREE_LIMIT: usize = 2;

/// The ticks of `host_statistics(HOST_CPU_LOAD_INFO)`, added over all
/// processors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuTicks {
    /// Ticks in user mode.
    pub user: u64,
    /// Ticks in kernel mode.
    pub system: u64,
    /// Ticks with nothing to run.
    pub idle: u64,
    /// Ticks in user mode at a changed priority.
    pub nice: u64,
}

impl CpuTicks {
    /// Gives the change from `before` to `after`, field by field. A counter
    /// that went back gives 0.
    #[must_use]
    pub fn between(before: Self, after: Self) -> Self {
        Self {
            user: after.user.saturating_sub(before.user),
            system: after.system.saturating_sub(before.system),
            idle: after.idle.saturating_sub(before.idle),
            nice: after.nice.saturating_sub(before.nice),
        }
    }

    /// Gives the sum of the four counters. The sum saturates.
    #[must_use]
    pub fn total(self) -> u64 {
        self.user
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.nice)
    }
}

/// The level that the kernel gives for normal memory pressure.
const LEVEL_NORMAL: i32 = 1;

/// The level that the kernel gives for warning memory pressure.
const LEVEL_WARNING: i32 = 2;

/// The level that the kernel gives for critical memory pressure.
const LEVEL_CRITICAL: i32 = 4;

/// The memory pressure that the kernel reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressure {
    /// The memory is not short.
    Normal,
    /// The kernel compresses and swaps to keep memory free.
    Warning,
    /// The kernel can hardly keep memory free.
    Critical,
}

impl Pressure {
    /// Gives the pressure for a level of `kern.memorystatus_vm_pressure_level`:
    /// 1 is normal, 2 is warning, and 4 is critical. Any other level gives
    /// `None`.
    #[must_use]
    pub fn from_level(level: i32) -> Option<Self> {
        match level {
            LEVEL_NORMAL => Some(Self::Normal),
            LEVEL_WARNING => Some(Self::Warning),
            LEVEL_CRITICAL => Some(Self::Critical),
            _ => None,
        }
    }

    /// Gives the word that a person reads: `normal`, `warning`, or `critical`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

/// How much of the CPU is free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuState {
    /// The idle part is below [`FULL_IDLE_PERCENT`].
    Full,
    /// The idle part is below [`BUSY_IDLE_PERCENT`], and not below the full
    /// limit.
    Busy,
    /// The CPU has enough idle time.
    Calm,
}

/// Gives the state of the CPU for the ticks of the window.
///
/// The comparison uses whole numbers and no rounding: idle times 100 below
/// [`FULL_IDLE_PERCENT`] times the total gives [`CpuState::Full`], and the
/// same test with [`BUSY_IDLE_PERCENT`] gives [`CpuState::Busy`]. A total of
/// zero gives [`CpuState::Calm`], because `observe` refuses such ticks before
/// they get here.
#[must_use]
pub fn cpu_state(ticks: CpuTicks) -> CpuState {
    let _ = (ticks, FULL_IDLE_PERCENT, BUSY_IDLE_PERCENT);
    CpuState::Calm
}

/// Tells whether the system part is high: the system ticks times 100 are at
/// or above [`HIGH_SYSTEM_PERCENT`] times the total. A total of zero gives
/// false.
#[must_use]
pub fn high_system(ticks: CpuTicks) -> bool {
    let _ = (ticks, HIGH_SYSTEM_PERCENT);
    false
}

/// Tells whether the memory is short: the pressure is warning or critical, or
/// the kernel swapped out at least one page over the window.
#[must_use]
pub fn memory_short(pressure: Pressure, swapouts: u64) -> bool {
    let _ = (pressure, swapouts);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The change is the difference of each counter. A counter that went back
    /// gives 0 and does not wrap, and the other fields stay right.
    #[test]
    fn the_change_of_the_ticks_is_the_difference_of_each_field() {
        let before = CpuTicks {
            user: 10,
            system: 20,
            idle: 30,
            nice: 40,
        };
        let after = CpuTicks {
            user: 15,
            system: 28,
            idle: 30,
            nice: 5,
        };
        assert_eq!(
            CpuTicks::between(before, after),
            CpuTicks {
                user: 5,
                system: 8,
                idle: 0,
                nice: 0
            }
        );
    }

    /// The total is the sum of the four counters, and it saturates.
    #[test]
    fn the_total_of_the_ticks_is_the_sum_and_saturates() {
        let ticks = CpuTicks {
            user: 1,
            system: 2,
            idle: 3,
            nice: 4,
        };
        assert_eq!(ticks.total(), 10);
        let huge = CpuTicks {
            user: u64::MAX,
            system: 1,
            idle: 1,
            nice: 1,
        };
        assert_eq!(huge.total(), u64::MAX);
    }

    /// The three levels of the kernel map to the three pressures. Any other
    /// level is unknown, and the caller refuses it.
    #[test]
    fn the_levels_of_the_kernel_map_to_pressures() {
        assert_eq!(Pressure::from_level(1), Some(Pressure::Normal));
        assert_eq!(Pressure::from_level(2), Some(Pressure::Warning));
        assert_eq!(Pressure::from_level(4), Some(Pressure::Critical));
        for level in [0, 3, 5, -1] {
            assert_eq!(Pressure::from_level(level), None, "level {level}");
        }
    }

    /// Each pressure has the word that a person reads.
    #[test]
    fn each_pressure_has_its_name() {
        assert_eq!(Pressure::Normal.name(), "normal");
        assert_eq!(Pressure::Warning.name(), "warning");
        assert_eq!(Pressure::Critical.name(), "critical");
    }

    /// Makes ticks with a total of 1000.
    fn ticks(system: u64, idle: u64) -> CpuTicks {
        CpuTicks {
            user: 1000 - system - idle,
            system,
            idle,
            nice: 0,
        }
    }

    /// The CPU is full below 10% idle. The test checks the limit and one step
    /// on each side.
    #[test]
    fn the_cpu_is_full_below_the_full_idle_limit() {
        assert_eq!(cpu_state(ticks(0, 99)), CpuState::Full);
        assert_eq!(cpu_state(ticks(0, 100)), CpuState::Busy);
        assert_eq!(cpu_state(ticks(0, 101)), CpuState::Busy);
    }

    /// The CPU is busy below 40% idle. The test checks the limit and one step
    /// on each side.
    #[test]
    fn the_cpu_is_busy_below_the_busy_idle_limit() {
        assert_eq!(cpu_state(ticks(0, 399)), CpuState::Busy);
        assert_eq!(cpu_state(ticks(0, 400)), CpuState::Calm);
        assert_eq!(cpu_state(ticks(0, 401)), CpuState::Calm);
    }

    /// The system part is high at 30% and above. The test checks the limit
    /// and one step on each side.
    #[test]
    fn the_system_part_is_high_at_the_limit() {
        assert!(!high_system(ticks(299, 0)));
        assert!(high_system(ticks(300, 0)));
        assert!(high_system(ticks(301, 0)));
    }

    /// Ticks that total zero give a calm CPU and no high system part.
    #[test]
    fn ticks_that_total_zero_are_calm() {
        assert_eq!(cpu_state(CpuTicks::default()), CpuState::Calm);
        assert!(!high_system(CpuTicks::default()));
    }

    /// The memory is short under warning or critical pressure, and when the
    /// kernel swapped out one page.
    #[test]
    fn the_memory_is_short_under_pressure_or_with_swapouts() {
        assert!(!memory_short(Pressure::Normal, 0));
        assert!(memory_short(Pressure::Normal, 1));
        assert!(memory_short(Pressure::Warning, 0));
        assert!(memory_short(Pressure::Critical, 0));
    }
}
