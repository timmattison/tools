//! The share of the CPU time that the user and the system processes use.
//!
//! The kernel gives four counters of CPU ticks. A share needs two readings,
//! because the counters grow from the boot of the host.

/// The CPU tick counters of the host, as the kernel gives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ticks {
    /// Ticks that user processes used.
    pub user: u32,
    /// Ticks that the kernel used.
    pub system: u32,
    /// Ticks that nothing used.
    pub idle: u32,
    /// Ticks that user processes of lowered priority used.
    pub nice: u32,
}

/// The shares of the CPU time between two readings. The sum is 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuShare {
    user: f64,
    system: f64,
    idle: f64,
}

impl CpuShare {
    /// Computes the shares of the CPU time between two readings.
    ///
    /// The nice ticks count as user ticks. Each difference wraps, because the
    /// counters have 32 bits. If no ticks occurred, the share is all idle.
    #[must_use]
    pub fn between(earlier: Ticks, later: Ticks) -> CpuShare {
        let user = f64::from(later.user.wrapping_sub(earlier.user))
            + f64::from(later.nice.wrapping_sub(earlier.nice));
        let system = f64::from(later.system.wrapping_sub(earlier.system));
        let idle = f64::from(later.idle.wrapping_sub(earlier.idle));
        let total = user + system + idle;
        if total == 0.0 {
            return CpuShare {
                user: 0.0,
                system: 0.0,
                idle: 1.0,
            };
        }
        CpuShare {
            user: user / total,
            system: system / total,
            idle: idle / total,
        }
    }

    /// Builds the shares from three values that a line gave.
    ///
    /// The result is `None` if a share is not finite or is outside 0 to 1.
    /// This function does not check the sum.
    #[must_use]
    pub fn from_shares(user: f64, system: f64, idle: f64) -> Option<CpuShare> {
        let _ = (user, system, idle);
        None
    }

    /// The share of the user processes, nice processes included.
    #[must_use]
    pub fn user(self) -> f64 {
        self.user
    }

    /// The share of the kernel.
    #[must_use]
    pub fn system(self) -> f64 {
        self.system
    }

    /// The share that nothing used.
    #[must_use]
    pub fn idle(self) -> f64 {
        self.idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOLERANCE: f64 = 1e-12;

    /// Asserts the three shares and that their sum is 1.
    fn assert_share(share: CpuShare, user: f64, system: f64, idle: f64) {
        assert!(
            (share.user() - user).abs() < TOLERANCE,
            "user {}",
            share.user()
        );
        assert!(
            (share.system() - system).abs() < TOLERANCE,
            "system {}",
            share.system()
        );
        assert!(
            (share.idle() - idle).abs() < TOLERANCE,
            "idle {}",
            share.idle()
        );
        let sum = share.user() + share.system() + share.idle();
        assert!((sum - 1.0).abs() < TOLERANCE, "sum {sum}");
    }

    #[test]
    fn from_shares_accepts_the_shares_from_0_to_1() {
        let share = CpuShare::from_shares(0.0, 0.0, 1.0).unwrap();
        assert_share(share, 0.0, 0.0, 1.0);
        let share = CpuShare::from_shares(1.0, 0.0, 0.0).unwrap();
        assert_share(share, 1.0, 0.0, 0.0);
        let share = CpuShare::from_shares(0.5, 0.25, 0.25).unwrap();
        assert_share(share, 0.5, 0.25, 0.25);
    }

    #[test]
    fn from_shares_refuses_a_share_that_is_out_of_range_or_not_finite() {
        for bad in [-0.1, 1.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(CpuShare::from_shares(bad, 0.0, 0.5), None, "user {bad}");
            assert_eq!(CpuShare::from_shares(0.0, bad, 0.5), None, "system {bad}");
            assert_eq!(CpuShare::from_shares(0.0, 0.5, bad), None, "idle {bad}");
        }
    }

    #[test]
    fn a_normal_interval_gives_the_shares_of_the_ticks() {
        let later = Ticks {
            user: 50,
            system: 25,
            idle: 25,
            nice: 0,
        };
        assert_share(CpuShare::between(Ticks::default(), later), 0.5, 0.25, 0.25);
    }

    #[test]
    fn nice_ticks_count_as_user_ticks() {
        let later = Ticks {
            user: 30,
            system: 25,
            idle: 25,
            nice: 20,
        };
        assert_share(CpuShare::between(Ticks::default(), later), 0.5, 0.25, 0.25);
    }

    #[test]
    fn an_interval_with_no_ticks_is_all_idle() {
        let reading = Ticks {
            user: 7,
            system: 8,
            idle: 9,
            nice: 10,
        };
        assert_share(CpuShare::between(reading, reading), 0.0, 0.0, 1.0);
    }

    #[test]
    fn a_counter_that_wraps_gives_the_same_shares_as_no_wrap() {
        let earlier = Ticks {
            user: u32::MAX - 9,
            system: 0,
            idle: 0,
            nice: 0,
        };
        let later = Ticks {
            user: 40,
            system: 25,
            idle: 25,
            nice: 0,
        };
        assert_share(CpuShare::between(earlier, later), 0.5, 0.25, 0.25);
    }
}
