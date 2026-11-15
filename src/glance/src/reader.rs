//! The reader of the kernel: the CPU tick counters and the memory pressure.

use std::fmt;

use crate::cpu::Ticks;
use crate::sample::Reading;

/// The reason that a read of the kernel failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError {
    /// The name of the kernel call that failed.
    call: &'static str,
    /// What went wrong, in words.
    reason: String,
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot read {}: {}", self.call, self.reason)
    }
}

impl std::error::Error for ReadError {}

/// A reader of the kernel values that `glance` shows.
#[derive(Debug)]
pub struct Reader {
    /// The port of this host.
    host: libc::mach_port_t,
}

impl Reader {
    /// Makes a reader.
    #[must_use]
    pub fn new() -> Reader {
        Reader { host: 0 }
    }

    /// Reads the CPU tick counters and the memory pressure.
    ///
    /// # Errors
    ///
    /// Returns an error if a kernel call fails or gives a value of the wrong size.
    pub fn read(&self) -> Result<Reading, ReadError> {
        let _ = self.host;
        Ok(Reading {
            ticks: Ticks::default(),
            pressure_level: 0,
            free_percent: -1,
        })
    }
}

impl Default for Reader {
    fn default() -> Reader {
        Reader::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The raw pressure levels of the kernel.
    const LEVELS: [i32; 3] = [1, 2, 4];

    #[test]
    fn two_reads_of_this_mac_give_levels_and_percentages_in_range() {
        let reader = Reader::new();
        for _ in 0..2 {
            let reading = reader.read().expect("the kernel answers");
            assert!(
                LEVELS.contains(&reading.pressure_level),
                "level {}",
                reading.pressure_level
            );
            assert!(
                (0..=100).contains(&reading.free_percent),
                "free {}",
                reading.free_percent
            );
        }
    }
}
