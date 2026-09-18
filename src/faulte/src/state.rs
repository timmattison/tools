//! The state of a Claude Code session, from its registry record.
//!
//! The registry file of a session gives its status and the time of the last
//! status change. The ranking shows the state on the row of the session, and
//! the rules of `faulte kill` read it. Only an idle session can be a
//! candidate, and only when it became idle long enough ago.
//!
//! The state is a pure function of the record and of the time now. Thus a
//! test gives both values, and no test reads a clock.

use std::time::{Duration, SystemTime};

use occ::{SessionRecord, SessionStatus};

/// The state of one session, as the ranking shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// The session runs a turn.
    Busy,
    /// The session waits for an answer from its user. A stop discards the
    /// question, so this state is active.
    Waiting,
    /// The session runs no turn. It waits for its next prompt.
    Idle {
        /// The time since the status last changed. `None` when the record
        /// gives no such time, or when that time is after the time now.
        for_: Option<Duration>,
    },
    /// A status that `occ` does not know, as the record gives it, for example
    /// `shell`. It is active, because only `idle` is safe to stop.
    Other(String),
    /// The record gives no status.
    Unknown,
}

impl SessionState {
    /// Gives the state of the session of `record` at the time `now`.
    #[must_use]
    pub fn from_record(record: &SessionRecord, now: SystemTime) -> Self {
        let _ = (record, now);
        Self::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use occ::SessionId;

    /// A session ID for the tests.
    const SESSION: &str = "d3b0d921-f0a1-41fc-b309-c11aa30c1173";

    /// Gives a record with `status`, changed at `changed_at`.
    fn record(status: Option<SessionStatus>, changed_at: Option<SystemTime>) -> SessionRecord {
        SessionRecord {
            session: SessionId::parse(SESSION).expect("the test ID is a UUID"),
            status,
            status_changed_at: changed_at,
            directory: None,
        }
    }

    /// Each status other than `idle` gives its state, whatever the time of
    /// the change. A record with no status gives `Unknown`.
    #[test]
    fn each_status_other_than_idle_gives_its_state() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let cases = [
            (Some(SessionStatus::Busy), SessionState::Busy),
            (Some(SessionStatus::Waiting), SessionState::Waiting),
            (
                Some(SessionStatus::Other("shell".to_owned())),
                SessionState::Other("shell".to_owned()),
            ),
            (None, SessionState::Unknown),
        ];

        for (status, state) in cases {
            for changed_at in [None, Some(now - Duration::from_secs(60))] {
                assert_eq!(
                    SessionState::from_record(&record(status.clone(), changed_at), now),
                    state,
                    "the status {status:?}, changed at {changed_at:?}"
                );
            }
        }
    }
}
