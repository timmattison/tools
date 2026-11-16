//! The check that tells if the process of a session still runs.

use crate::state::EpochMillis;

/// Tell if a session is stale: its process is gone, or a new process reused
/// its pid.
///
/// `process_start_secs` is the start time of the process with that pid, in
/// whole seconds since the epoch. It is `None` when no such process runs.
#[must_use]
pub fn is_stale(started_at: EpochMillis, process_start_secs: Option<u64>) -> bool {
    let _ = (started_at, process_start_secs);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const STARTED_AT: EpochMillis = EpochMillis::new(1_794_823_262_488);

    #[test]
    fn no_process_is_stale() {
        assert!(is_stale(STARTED_AT, None));
    }

    #[test]
    fn a_process_that_started_before_the_session_is_not_stale() {
        assert!(!is_stale(STARTED_AT, Some(1_794_823_261)));
    }

    #[test]
    fn a_process_in_the_same_second_is_not_stale() {
        assert!(!is_stale(STARTED_AT, Some(1_794_823_262)));
    }

    #[test]
    fn a_process_that_started_one_second_later_is_stale() {
        assert!(is_stale(STARTED_AT, Some(1_794_823_263)));
    }

    #[test]
    fn the_comparison_does_not_overflow() {
        assert!(is_stale(STARTED_AT, Some(u64::MAX)));
    }
}
