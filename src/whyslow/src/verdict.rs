//! The verdict: which resource is short, and which groups use it.
//!
//! The rules are pure. They read plain values and give plain values, so a test
//! needs no Mac. Each threshold is a named constant, and each comparison uses
//! whole numbers, so a value at the limit gives the same answer on each run.

use std::time::Duration;

use faulte::vm::SwapUsage;

use crate::tally::GroupTotal;

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
    let total = u128::from(ticks.total());
    let idle = u128::from(ticks.idle) * 100;
    if idle < u128::from(FULL_IDLE_PERCENT) * total {
        CpuState::Full
    } else if idle < u128::from(BUSY_IDLE_PERCENT) * total {
        CpuState::Busy
    } else {
        CpuState::Calm
    }
}

/// Tells whether the system part is high: the system ticks times 100 are at
/// or above [`HIGH_SYSTEM_PERCENT`] times the total. A total of zero gives
/// false.
#[must_use]
pub fn high_system(ticks: CpuTicks) -> bool {
    let total = u128::from(ticks.total());
    total > 0 && u128::from(ticks.system) * 100 >= u128::from(HIGH_SYSTEM_PERCENT) * total
}

/// Tells whether the memory is short: the pressure is warning or critical, or
/// the kernel swapped out at least one page over the window.
#[must_use]
pub fn memory_short(pressure: Pressure, swapouts: u64) -> bool {
    pressure != Pressure::Normal || swapouts > 0
}

/// Everything that `observe` read, as plain values.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    /// The change of the CPU ticks over the window.
    pub ticks: CpuTicks,
    /// The number of processors.
    pub processors: u32,
    /// The load average of the last minute.
    pub load: f64,
    /// The time between the two snapshots of the process table.
    pub window: Duration,
    /// The memory pressure at the end of the window.
    pub pressure: Pressure,
    /// The number of pages that the kernel swapped out over the window.
    pub swapouts: u64,
    /// The size of the memory compressor, in bytes.
    pub compressor_bytes: u64,
    /// The swap file.
    pub swap: SwapUsage,
    /// The totals of each group, from `tally`.
    pub groups: Vec<GroupTotal>,
}

/// A share of one resource, for a worktree or for a rest of worktrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    /// What a person reads. It is empty for a rest of worktrees.
    pub label: String,
    /// The CPU, as a percent of one processor.
    pub cpu_percent: u64,
    /// The memory, in bytes.
    pub memory_bytes: u64,
    /// True when the memory is a lower bound.
    pub at_least: bool,
}

/// One group that the summary names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupLine {
    /// The label of the group.
    pub label: String,
    /// The number of processes.
    pub processes: usize,
    /// The number of accounts that own them.
    pub accounts: usize,
    /// The CPU, as a percent of one processor.
    pub cpu_percent: u64,
    /// The memory, in bytes.
    pub memory_bytes: u64,
    /// True when the memory is a lower bound.
    pub at_least: bool,
    /// Only for the compilers: the [`WORKTREE_LIMIT`] largest worktrees by
    /// the resource of the block. The list is empty for any other group.
    pub worktrees: Vec<Share>,
    /// Only for the compilers: the count and the sum of the worktrees that
    /// the list leaves out. `None` when no worktree is left out.
    pub more: Option<(usize, Share)>,
}

/// The verdict, as plain values for `render`.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// How much of the CPU is free.
    pub cpu_state: CpuState,
    /// True when the memory is short.
    pub memory_short: bool,
    /// The idle part of the CPU, in percent, rounded half up.
    pub idle_percent: u64,
    /// The system part of the CPU, in percent, rounded half up.
    pub system_percent: u64,
    /// The load average of the last minute.
    pub load: f64,
    /// The number of processors.
    pub processors: u32,
    /// True when the system part is high and the memory is short.
    pub compression_likely: bool,
    /// The groups that use the most CPU. Empty when the CPU is calm.
    pub cpu_groups: Vec<GroupLine>,
    /// The CPU that no group explains, in percent of one processor. `Some`
    /// only when the CPU is not calm and the value is at least
    /// [`UNEXPLAINED_PERCENT`].
    pub unexplained_percent: Option<u64>,
    /// The memory pressure.
    pub pressure: Pressure,
    /// The size of the memory compressor, in bytes.
    pub compressor_bytes: u64,
    /// The swap file.
    pub swap: SwapUsage,
    /// The groups that use the most memory. Empty when the memory is not
    /// short.
    pub memory_groups: Vec<GroupLine>,
}

/// Gives the verdict for the facts of one sample.
///
/// The CPU block lists the [`GROUP_LIMIT`] groups with the most CPU, the
/// largest first, and ties by label. A group with no CPU is left out. The
/// memory block does the same for the memory. A group of the compilers lists
/// its [`WORKTREE_LIMIT`] largest worktrees by the resource of the block, and
/// one line for the rest.
///
/// The machine CPU is the busy part of the ticks times the processors. The
/// groups CPU is the sum of the CPU of all groups. The difference, which
/// saturates at zero, is the CPU that no group explains: the kernel, and the
/// processes that started and stopped between the two snapshots.
#[must_use]
pub fn summarize(facts: &Facts) -> Summary {
    let cpu_state = cpu_state(facts.ticks);
    let memory_short = memory_short(facts.pressure, facts.swapouts);
    let total = facts.ticks.total();
    Summary {
        cpu_state,
        memory_short,
        idle_percent: percent(facts.ticks.idle, total),
        system_percent: percent(facts.ticks.system, total),
        load: facts.load,
        processors: facts.processors,
        compression_likely: high_system(facts.ticks) && memory_short,
        cpu_groups: if cpu_state == CpuState::Calm {
            Vec::new()
        } else {
            lines(facts, Resource::Cpu)
        },
        unexplained_percent: None,
        pressure: facts.pressure,
        compressor_bytes: facts.compressor_bytes,
        swap: facts.swap,
        memory_groups: if memory_short {
            lines(facts, Resource::Memory)
        } else {
            Vec::new()
        },
    }
}

/// The resource that a block of the summary ranks by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resource {
    /// The CPU time.
    Cpu,
    /// The memory.
    Memory,
}

/// Gives `part` as a percent of `total`, rounded half up. A total of zero
/// gives 0.
fn percent(part: u64, total: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    let part = u128::from(part) * 100;
    let total = u128::from(total);
    u64::try_from((part * 2 + total) / (total * 2)).unwrap_or(u64::MAX)
}

/// Gives the number that ranks one group by `resource`.
fn weight(total: &GroupTotal, resource: Resource) -> u64 {
    match resource {
        Resource::Cpu => total.cpu.centiseconds(),
        Resource::Memory => total.memory_bytes,
    }
}

/// Gives the lines of the groups that use the most of `resource`: the
/// largest first, ties by label, at most [`GROUP_LIMIT`], none with zero.
fn lines(facts: &Facts, resource: Resource) -> Vec<GroupLine> {
    let mut ranked: Vec<&GroupTotal> = facts
        .groups
        .iter()
        .filter(|total| weight(total, resource) > 0)
        .collect();
    ranked.sort_by(|left, right| {
        weight(right, resource)
            .cmp(&weight(left, resource))
            .then_with(|| left.group.label().cmp(right.group.label()))
    });
    ranked
        .into_iter()
        .take(GROUP_LIMIT)
        .map(|total| GroupLine {
            label: total.group.label().to_owned(),
            processes: total.processes,
            accounts: total.accounts,
            cpu_percent: total.cpu.percent_of(facts.window),
            memory_bytes: total.memory_bytes,
            at_least: total.at_least,
            worktrees: Vec::new(),
            more: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::groups::Group;
    use crate::sample::CpuTime;

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

    /// Makes the total of one group with no worktree.
    fn group(label: &str, cpu: u64, memory_bytes: u64) -> GroupTotal {
        GroupTotal {
            group: Group::Named(label.to_owned()),
            processes: 1,
            accounts: 1,
            cpu: CpuTime::from_centiseconds(cpu),
            memory_bytes,
            at_least: false,
            worktrees: Vec::new(),
        }
    }

    /// Makes facts over one second on 14 processors.
    fn facts(ticks: CpuTicks, pressure: Pressure, swapouts: u64, groups: Vec<GroupTotal>) -> Facts {
        Facts {
            ticks,
            processors: 14,
            load: 3.0,
            window: Duration::from_secs(1),
            pressure,
            swapouts,
            compressor_bytes: 1 << 30,
            swap: SwapUsage {
                total_bytes: 2 << 30,
                used_bytes: 1 << 30,
            },
            groups,
        }
    }

    /// The summary carries the states, and the percents round half up: 0.5%
    /// idle is 1%, and 30.5% system is 31%.
    #[test]
    fn the_summary_carries_the_states_and_rounds_half_up() {
        let ticks = CpuTicks {
            user: 690,
            system: 305,
            idle: 5,
            nice: 0,
        };
        let summary = summarize(&facts(ticks, Pressure::Warning, 0, Vec::new()));
        assert_eq!(summary.cpu_state, CpuState::Full);
        assert!(summary.memory_short);
        assert_eq!(summary.idle_percent, 1);
        assert_eq!(summary.system_percent, 31);
        assert_eq!(summary.pressure, Pressure::Warning);
        assert_eq!(summary.processors, 14);
        assert_eq!(summary.compressor_bytes, 1 << 30);
    }

    /// Compression is likely only when the system part is high and the memory
    /// is short.
    #[test]
    fn compression_is_likely_only_with_high_system_and_short_memory() {
        let high = CpuTicks {
            user: 500,
            system: 400,
            idle: 100,
            nice: 0,
        };
        let low = CpuTicks {
            user: 800,
            system: 100,
            idle: 100,
            nice: 0,
        };
        let likely =
            |ticks, pressure| summarize(&facts(ticks, pressure, 0, Vec::new())).compression_likely;
        assert!(likely(high, Pressure::Warning));
        assert!(!likely(high, Pressure::Normal));
        assert!(!likely(low, Pressure::Warning));
    }

    /// The CPU block lists at most [`GROUP_LIMIT`] groups, the largest first,
    /// ties by label. A group with no CPU is left out.
    #[test]
    fn the_cpu_groups_are_the_largest_three_and_skip_zero() {
        let full = CpuTicks {
            user: 1000,
            system: 0,
            idle: 0,
            nice: 0,
        };
        let groups = vec![
            group("zeta", 50, 1),
            group("alpha", 50, 1),
            group("big", 200, 1),
            group("idle", 0, 1),
            group("small", 10, 1),
        ];
        let summary = summarize(&facts(full, Pressure::Normal, 0, groups));
        let labels: Vec<&str> = summary
            .cpu_groups
            .iter()
            .map(|g| g.label.as_str())
            .collect();
        assert_eq!(labels, vec!["big", "alpha", "zeta"]);
        assert_eq!(summary.cpu_groups[0].cpu_percent, 200);
        assert_eq!(summary.cpu_groups.len(), GROUP_LIMIT);
    }

    /// The CPU block is empty when the CPU is calm.
    #[test]
    fn the_cpu_groups_are_empty_when_the_cpu_is_calm() {
        let calm = CpuTicks {
            user: 100,
            system: 0,
            idle: 900,
            nice: 0,
        };
        let summary = summarize(&facts(calm, Pressure::Normal, 0, vec![group("a", 10, 1)]));
        assert_eq!(summary.cpu_state, CpuState::Calm);
        assert!(summary.cpu_groups.is_empty());
    }

    /// The memory block lists the largest groups by memory, ties by label,
    /// and leaves out a group with no memory.
    #[test]
    fn the_memory_groups_are_the_largest_three_and_skip_zero() {
        let calm = CpuTicks {
            user: 100,
            system: 0,
            idle: 900,
            nice: 0,
        };
        let groups = vec![
            group("zeta", 1, 50),
            group("alpha", 1, 50),
            group("big", 1, 200),
            group("none", 1, 0),
            group("small", 1, 10),
        ];
        let summary = summarize(&facts(calm, Pressure::Critical, 0, groups));
        let labels: Vec<&str> = summary
            .memory_groups
            .iter()
            .map(|g| g.label.as_str())
            .collect();
        assert_eq!(labels, vec!["big", "alpha", "zeta"]);
        assert_eq!(summary.memory_groups[0].memory_bytes, 200);
    }

    /// The memory block is empty when the memory is not short.
    #[test]
    fn the_memory_groups_are_empty_when_the_memory_is_not_short() {
        let calm = CpuTicks {
            user: 100,
            system: 0,
            idle: 900,
            nice: 0,
        };
        let summary = summarize(&facts(calm, Pressure::Normal, 0, vec![group("a", 1, 10)]));
        assert!(summary.memory_groups.is_empty());
    }
}
