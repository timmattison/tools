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
        let _ = (before, after);
        Self::default()
    }

    /// Gives the sum of the four counters. The sum saturates.
    #[must_use]
    pub fn total(self) -> u64 {
        0
    }
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
}
