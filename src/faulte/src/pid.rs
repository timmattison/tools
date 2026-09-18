//! The numbers that name a process and an account.
//!
//! A PID and a UID are both a `u32`, and a function that takes two `u32`
//! values accepts them in the wrong order without an error. A newtype for each
//! one makes the compiler refuse that mistake.

use std::fmt;

/// The ID of a process, as `top` and `ps` print it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pid(u32);

impl Pid {
    /// Makes the PID `value`.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Gives the number of this PID.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Pid {
    /// Writes the bare number, for example `45646`.
    ///
    /// The number obeys the width and the alignment of the format, the same
    /// as a bare `u32`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

/// The ID of an account, as `ps` prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Uid(u32);

impl Uid {
    /// Makes the UID `value`.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Gives the number of this UID.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Uid {
    /// Writes the bare number, for example `501`.
    ///
    /// The number obeys the width and the alignment of the format, the same
    /// as a bare `u32`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pid_gives_back_its_number_and_prints_it_bare() {
        let pid = Pid::new(45_646);

        assert_eq!(pid.get(), 45_646);
        assert_eq!(pid.to_string(), "45646");
        assert_eq!(Pid::new(u32::MAX).get(), u32::MAX);
    }

    #[test]
    fn a_uid_gives_back_its_number_and_prints_it_bare() {
        let uid = Uid::new(501);

        assert_eq!(uid.get(), 501);
        assert_eq!(uid.to_string(), "501");
        assert_eq!(Uid::new(0).to_string(), "0");
    }

    /// A table of processes lines up its columns with a width and an
    /// alignment. The number obeys both, the same as a bare `u32`.
    #[test]
    fn a_pid_and_a_uid_obey_the_width_and_the_alignment() {
        assert_eq!(format!("{:>7}|", Pid::new(42)), "     42|");
        assert_eq!(format!("{:<7}|", Pid::new(42)), "42     |");
        assert_eq!(format!("{:>5}|", Uid::new(501)), "  501|");
    }

    /// The order is the order of the numbers, so a sort by PID is a sort by
    /// number.
    #[test]
    fn pids_order_by_their_numbers() {
        let mut pids = [Pid::new(20), Pid::new(3), Pid::new(100)];
        pids.sort();

        assert_eq!(pids, [Pid::new(3), Pid::new(20), Pid::new(100)]);
    }
}
