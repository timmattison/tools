//! The totals of each group.
//!
//! One process is small. The sum of a group is the number that explains a
//! slow Mac. This module adds the CPU and the memory of the processes of each
//! group, and, for the compilers, of each worktree.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use faulte::pid::Uid;

use crate::groups::Group;
use crate::sample::CpuTime;

/// The memory of one process, with the way that `whyslow` measured it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Memory {
    /// `ri_phys_footprint`, in bytes. It counts compressed memory.
    Footprint(u64),
    /// The resident set size in bytes. `whyslow` uses it when the kernel
    /// refuses the footprint. It is a lower bound.
    Resident(u64),
}

impl Memory {
    /// Gives the number of bytes, whatever the way of measure.
    #[must_use]
    pub fn bytes(self) -> u64 {
        match self {
            Self::Footprint(bytes) | Self::Resident(bytes) => bytes,
        }
    }

    /// Tells whether the number is a lower bound: true for [`Memory::Resident`].
    #[must_use]
    pub fn at_least(self) -> bool {
        matches!(self, Self::Resident(_))
    }
}

/// One process, with the numbers that `tally` adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSample {
    /// The account that owns the process.
    pub uid: Uid,
    /// The group of the process.
    pub group: Group,
    /// For a process of [`Group::Compilers`]: its worktree, or `None` when it
    /// builds for no worktree. `tally` ignores this field for any other
    /// group, and callers set it to `None` there.
    pub worktree: Option<PathBuf>,
    /// The CPU time that the process used over the window.
    pub cpu: CpuTime,
    /// The memory of the process.
    pub memory: Memory,
}

/// The totals of the compilers of one worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeTotal {
    /// The worktree. `None` means that the compilers build for no worktree.
    pub worktree: Option<PathBuf>,
    /// The number of processes.
    pub processes: usize,
    /// The sum of the CPU time.
    pub cpu: CpuTime,
    /// The sum of the memory, in bytes.
    pub memory_bytes: u64,
    /// True when a process of this worktree gave only a lower bound.
    pub at_least: bool,
}

/// The totals of one group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupTotal {
    /// The group.
    pub group: Group,
    /// The number of processes.
    pub processes: usize,
    /// The number of distinct accounts that own these processes.
    pub accounts: usize,
    /// The sum of the CPU time.
    pub cpu: CpuTime,
    /// The sum of the memory, in bytes.
    pub memory_bytes: u64,
    /// True when a process of this group gave only a lower bound
    /// ([`Memory::Resident`]).
    pub at_least: bool,
    /// The totals of each worktree. Only [`Group::Compilers`] has them. The
    /// list is empty for any other group.
    pub worktrees: Vec<WorktreeTotal>,
}

/// Gives one total for each group that has a process.
///
/// The totals come in ascending order of [`Group`], and the worktrees of a
/// group come in ascending order of their path, with the entry for no
/// worktree first. `tally` does not sort by size. `summarize` ranks the
/// totals. A process is in one group only, so the counts of the groups add up
/// to the number of processes.
#[must_use]
pub fn tally(processes: &[ProcessSample]) -> Vec<GroupTotal> {
    let mut by_group: BTreeMap<Group, (GroupTotal, BTreeSet<Uid>)> = BTreeMap::new();
    for process in processes {
        let (total, accounts) = by_group.entry(process.group.clone()).or_insert_with(|| {
            (
                GroupTotal {
                    group: process.group.clone(),
                    processes: 0,
                    accounts: 0,
                    cpu: CpuTime::default(),
                    memory_bytes: 0,
                    at_least: false,
                    worktrees: Vec::new(),
                },
                BTreeSet::new(),
            )
        });
        total.processes += 1;
        total.cpu = total.cpu + process.cpu;
        total.memory_bytes = total.memory_bytes.saturating_add(process.memory.bytes());
        total.at_least |= process.memory.at_least();
        accounts.insert(process.uid);
    }
    by_group
        .into_values()
        .map(|(mut total, accounts)| {
            total.accounts = accounts.len();
            total
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each variant gives its bytes. Only the resident size is a lower bound.
    #[test]
    fn memory_gives_its_bytes_and_tells_if_it_is_a_lower_bound() {
        assert_eq!(Memory::Footprint(4096).bytes(), 4096);
        assert!(!Memory::Footprint(4096).at_least());
        assert_eq!(Memory::Resident(8192).bytes(), 8192);
        assert!(Memory::Resident(8192).at_least());
    }

    /// Makes a sample of one process.
    fn sample(uid: u32, group: Group, cpu: u64, memory: Memory) -> ProcessSample {
        ProcessSample {
            uid: Uid::new(uid),
            group,
            worktree: None,
            cpu: CpuTime::from_centiseconds(cpu),
            memory,
        }
    }

    /// Finds the total of one group.
    fn total_of<'a>(totals: &'a [GroupTotal], group: &Group) -> &'a GroupTotal {
        totals
            .iter()
            .find(|total| &total.group == group)
            .expect("the group has a total")
    }

    /// The processes of three groups give three totals in the order of the
    /// groups. Each total has its own count, CPU sum, and memory sum. No
    /// process is in two groups.
    #[test]
    fn three_groups_give_three_totals_in_group_order() {
        let node = Group::Named("node".to_owned());
        let processes = vec![
            sample(501, node.clone(), 7, Memory::Footprint(100)),
            sample(501, Group::ClaudeCode, 10, Memory::Footprint(1000)),
            sample(501, Group::Spotlight, 3, Memory::Footprint(30)),
            sample(501, Group::ClaudeCode, 20, Memory::Footprint(2000)),
            sample(501, node.clone(), 1, Memory::Footprint(5)),
            sample(501, Group::ClaudeCode, 30, Memory::Footprint(3000)),
        ];
        let totals = tally(&processes);

        let order: Vec<&Group> = totals.iter().map(|total| &total.group).collect();
        assert_eq!(order, vec![&Group::ClaudeCode, &Group::Spotlight, &node]);
        let claude = total_of(&totals, &Group::ClaudeCode);
        assert_eq!(claude.processes, 3);
        assert_eq!(claude.cpu, CpuTime::from_centiseconds(60));
        assert_eq!(claude.memory_bytes, 6000);
        let named = total_of(&totals, &node);
        assert_eq!(named.processes, 2);
        assert_eq!(named.cpu, CpuTime::from_centiseconds(8));
        assert_eq!(named.memory_bytes, 105);
        let spotlight = total_of(&totals, &Group::Spotlight);
        assert_eq!(spotlight.processes, 1);
        let counted: usize = totals.iter().map(|total| total.processes).sum();
        assert_eq!(counted, processes.len());
    }

    /// The count of accounts is the count of distinct UIDs: two UIDs give
    /// two, and one UID twice gives one.
    #[test]
    fn the_accounts_of_a_group_are_distinct_uids() {
        let two = tally(&[
            sample(501, Group::ClaudeCode, 1, Memory::Footprint(1)),
            sample(502, Group::ClaudeCode, 1, Memory::Footprint(1)),
        ]);
        assert_eq!(two.len(), 1);
        assert_eq!(two[0].accounts, 2);
        let one = tally(&[
            sample(501, Group::ClaudeCode, 1, Memory::Footprint(1)),
            sample(501, Group::ClaudeCode, 1, Memory::Footprint(1)),
        ]);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].accounts, 1);
    }

    /// A group is a lower bound when one process gave only the resident
    /// size. A group of footprints is exact.
    #[test]
    fn a_group_is_a_lower_bound_when_one_process_is_resident() {
        let mixed = tally(&[
            sample(501, Group::ClaudeCode, 1, Memory::Footprint(1)),
            sample(502, Group::ClaudeCode, 1, Memory::Resident(1)),
        ]);
        assert_eq!(mixed.len(), 1);
        assert!(mixed[0].at_least);
        let exact = tally(&[
            sample(501, Group::Spotlight, 1, Memory::Footprint(1)),
            sample(501, Group::Spotlight, 1, Memory::Footprint(1)),
        ]);
        assert_eq!(exact.len(), 1);
        assert!(!exact[0].at_least);
    }
}
