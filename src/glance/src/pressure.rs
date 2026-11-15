//! The memory pressure of the host.
//!
//! The kernel gives two values. One is the pressure level, and the other is the
//! percentage of the memory that is free.

/// The kernel value of `kern.memorystatus_vm_pressure_level` for normal pressure.
pub const RAW_NORMAL: i32 = 1;
/// The kernel value of `kern.memorystatus_vm_pressure_level` for warn pressure.
pub const RAW_WARN: i32 = 2;
/// The kernel value of `kern.memorystatus_vm_pressure_level` for critical pressure.
pub const RAW_CRITICAL: i32 = 4;

/// The name of the normal level.
pub const NAME_NORMAL: &str = "normal";
/// The name of the warn level.
pub const NAME_WARN: &str = "warn";
/// The name of the critical level.
pub const NAME_CRITICAL: &str = "critical";
/// The name of a level that the kernel gave and this tool does not know.
pub const NAME_UNKNOWN: &str = "unknown";

/// The largest percentage.
const PERCENT_MAX: i32 = 100;
/// The percentage in use if the conversion fails. It cannot fail after the clamp.
const PERCENT_FALLBACK: u8 = 100;

/// The pressure level of the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// The kernel has enough memory.
    Normal,
    /// The kernel starts to run short of memory.
    Warn,
    /// The kernel runs short of memory.
    Critical,
    /// A value that this tool does not know. It never reads as normal.
    Unknown(i32),
}

impl Level {
    /// Converts the kernel value into a level.
    #[must_use]
    pub fn from_raw(raw: i32) -> Level {
        let _ = raw;
        Level::Normal
    }

    /// Gives the kernel value back.
    #[must_use]
    pub fn raw(self) -> i32 {
        0
    }

    /// The name of the level, in lower case.
    #[must_use]
    pub fn name(self) -> &'static str {
        ""
    }
}

/// The pressure level and the percentage of the memory in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pressure {
    level: Level,
    percent_in_use: u8,
}

impl Pressure {
    /// Builds the pressure from the two kernel values.
    ///
    /// `level_raw` is `kern.memorystatus_vm_pressure_level`. `free_percent_raw`
    /// is `kern.memorystatus_level`. A free value outside 0 to 100 clamps.
    #[must_use]
    pub fn from_raw(level_raw: i32, free_percent_raw: i32) -> Pressure {
        let _ = (level_raw, free_percent_raw, PERCENT_MAX, PERCENT_FALLBACK);
        Pressure {
            level: Level::Normal,
            percent_in_use: 0,
        }
    }

    /// The pressure level.
    #[must_use]
    pub fn level(self) -> Level {
        self.level
    }

    /// The percentage of the memory in use, from 0 to 100.
    #[must_use]
    pub fn percent_in_use(self) -> u8 {
        self.percent_in_use
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_1_is_normal() {
        let level = Level::from_raw(1);
        assert_eq!(level, Level::Normal);
        assert_eq!(level.raw(), RAW_NORMAL);
        assert_eq!(level.name(), NAME_NORMAL);
    }

    #[test]
    fn raw_2_is_warn() {
        let level = Level::from_raw(2);
        assert_eq!(level, Level::Warn);
        assert_eq!(level.raw(), RAW_WARN);
        assert_eq!(level.name(), NAME_WARN);
    }

    #[test]
    fn raw_4_is_critical() {
        let level = Level::from_raw(4);
        assert_eq!(level, Level::Critical);
        assert_eq!(level.raw(), RAW_CRITICAL);
        assert_eq!(level.name(), NAME_CRITICAL);
    }

    #[test]
    fn an_unknown_raw_level_keeps_its_value_and_is_never_normal() {
        for raw in [3, 0, -1] {
            let level = Level::from_raw(raw);
            assert_eq!(level, Level::Unknown(raw));
            assert_eq!(level.raw(), raw);
            assert_eq!(level.name(), NAME_UNKNOWN);
        }
    }

    #[test]
    fn a_free_percentage_of_47_gives_53_in_use() {
        assert_eq!(Pressure::from_raw(1, 47).percent_in_use(), 53);
    }

    #[test]
    fn a_free_percentage_above_100_gives_0_in_use() {
        assert_eq!(Pressure::from_raw(1, 101).percent_in_use(), 0);
        assert_eq!(Pressure::from_raw(1, i32::MAX).percent_in_use(), 0);
    }

    #[test]
    fn a_free_percentage_below_0_gives_100_in_use() {
        assert_eq!(Pressure::from_raw(1, -1).percent_in_use(), 100);
        assert_eq!(Pressure::from_raw(1, i32::MIN).percent_in_use(), 100);
    }

    #[test]
    fn from_raw_keeps_the_level() {
        assert_eq!(Pressure::from_raw(4, 10).level(), Level::Critical);
    }
}
