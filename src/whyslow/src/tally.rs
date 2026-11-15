//! The totals of each group.
//!
//! One process is small. The sum of a group is the number that explains a
//! slow Mac. This module adds the CPU and the memory of the processes of each
//! group, and, for the compilers, of each worktree.

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
}
