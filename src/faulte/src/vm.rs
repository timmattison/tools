//! The counters of the virtual memory system, and what they say over an
//! interval.
//!
//! The header of the ranking gives the swap traffic, the size of the
//! compressor, and the swap in use. Those numbers say whether this Mac is
//! short of memory. On 2026-09-18 it counted 46,564 swap-ins in 20 seconds,
//! and it held 32 GB in the compressor.
//!
//! The per-process page-in counter does not count swap. Over the same 20
//! seconds, the per-process counters added up to about 1,000. Thus the swap
//! traffic comes from the system counters here, and the ranking of the
//! processes comes from the fault counters of `top`.
//!
//! The types here are plain values. The caller fills them from
//! `host_statistics64(HOST_VM_INFO64)` and `vm.swapusage`, which only macOS
//! has. Thus the rules below have tests on every platform.

/// The counters of the virtual memory system at one time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VmCounters {
    /// The pages that the system read in from swap since it started.
    pub swapins: u64,
    /// The pages that the system wrote out to swap since it started.
    pub swapouts: u64,
    /// The pages that the compressor holds now.
    pub compressor_pages: u64,
}

impl VmCounters {
    /// Gives the size of the compressor in bytes: the pages that it holds,
    /// times `page_size`.
    ///
    /// The page size of this Mac is 16,384 bytes, from `vm_page_size`.
    #[must_use]
    pub fn compressor_bytes(&self, page_size: u64) -> u64 {
        self.compressor_pages * page_size
    }
}

/// The swap traffic over an interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VmDelta {
    /// The pages that the system read in from swap over the interval.
    pub swapins: u64,
    /// The pages that the system wrote out to swap over the interval.
    pub swapouts: u64,
}

impl VmDelta {
    /// Gives the traffic from the counters `before` an interval to the
    /// counters `after` it.
    ///
    /// A counter that goes down gives no traffic. The counters count from the
    /// time that the system started, so a lower value means that the system
    /// started again between the two reads.
    #[must_use]
    pub fn between(before: VmCounters, after: VmCounters) -> Self {
        Self {
            swapins: after.swapins.saturating_sub(before.swapins),
            swapouts: after.swapouts.saturating_sub(before.swapouts),
        }
    }
}

/// The swap file of this Mac, from `vm.swapusage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SwapUsage {
    /// The size of the swap file, in bytes.
    pub total_bytes: u64,
    /// The part of the swap file in use, in bytes.
    pub used_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The traffic is the change of each counter over the interval. The
    /// values are the ones that this Mac counted in 20 seconds on
    /// 2026-09-18.
    #[test]
    fn the_traffic_is_the_change_of_each_counter() {
        let before = VmCounters {
            swapins: 1_000,
            swapouts: 2_000,
            compressor_pages: 2_097_152,
        };
        let after = VmCounters {
            swapins: 47_564,
            swapouts: 42_156,
            compressor_pages: 2_097_152,
        };

        assert_eq!(
            VmDelta::between(before, after),
            VmDelta {
                swapins: 46_564,
                swapouts: 40_156,
            }
        );
    }

    /// A counter that goes down gives no traffic. The counters count from the
    /// time that the system started, so they only go down when the system
    /// starts again between the two reads. A subtraction panics in a build
    /// with the checks on, and gives a very large number in a build without
    /// them.
    #[test]
    fn a_counter_that_goes_down_gives_no_traffic() {
        let before = VmCounters {
            swapins: 46_564,
            swapouts: 40_156,
            compressor_pages: 0,
        };
        let after = VmCounters {
            swapins: 3,
            swapouts: 0,
            compressor_pages: 0,
        };

        assert_eq!(VmDelta::between(before, after), VmDelta::default());
    }

    /// The size of the compressor is its pages times the size of a page. The
    /// values are 32 GiB in pages of 16,384 bytes, the state of this Mac on
    /// 2026-09-18.
    #[test]
    fn the_size_of_the_compressor_is_its_pages_times_the_size_of_a_page() {
        let counters = VmCounters {
            swapins: 0,
            swapouts: 0,
            compressor_pages: 2_097_152,
        };

        assert_eq!(counters.compressor_bytes(16_384), 34_359_738_368);
        assert_eq!(counters.compressor_bytes(4_096), 8_589_934_592);
        assert_eq!(VmCounters::default().compressor_bytes(16_384), 0);
    }

    /// A count of pages that no memory can hold gives the largest number, and
    /// not a panic. `host_statistics64` fills a field of 32 bits, so this
    /// state needs a caller that gives a page size which is wrong. The
    /// multiplication panics in a build with the checks on, and gives a small
    /// number in a build without them.
    #[test]
    fn a_size_of_the_compressor_that_does_not_fit_gives_the_largest_number() {
        let counters = VmCounters {
            swapins: 0,
            swapouts: 0,
            compressor_pages: u64::MAX,
        };

        assert_eq!(counters.compressor_bytes(16_384), u64::MAX);
        assert_eq!(counters.compressor_bytes(1), u64::MAX);
        assert_eq!(counters.compressor_bytes(0), 0);
    }
}
