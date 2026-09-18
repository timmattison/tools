//! The state of a Claude Code session, from its registry record.
//!
//! The registry file of a session gives its status and the time of the last
//! status change. The ranking shows the state on the row of the session, and
//! the rules of `faulte kill` read it. Only an idle session can be a
//! candidate, and only when it became idle long enough ago.
//!
//! The state is a pure function of the record and of the time now. Thus a
//! test gives both values, and no test reads a clock.

use std::fmt;
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
    ///
    /// The idle time is `now` minus the time of the last status change. A
    /// change after `now` gives no idle time, not a guess. The clock of this
    /// Mac can move back, and a session can write its file between the time
    /// that the caller reads `now` and the read of the file.
    #[must_use]
    pub fn from_record(record: &SessionRecord, now: SystemTime) -> Self {
        match &record.status {
            Some(SessionStatus::Idle) => Self::Idle {
                for_: record
                    .status_changed_at
                    .and_then(|changed_at| now.duration_since(changed_at).ok()),
            },
            Some(SessionStatus::Busy) => Self::Busy,
            Some(SessionStatus::Waiting) => Self::Waiting,
            Some(SessionStatus::Other(text)) => Self::Other(text.clone()),
            None => Self::Unknown,
        }
    }
}

/// The text of [`SessionState::Busy`].
const BUSY: &str = "busy";

/// The text of [`SessionState::Waiting`].
const WAITING: &str = "waiting";

/// The first word of the text of [`SessionState::Idle`].
const IDLE: &str = "idle";

/// The text in place of an idle time that the record does not give.
const UNKNOWN_IDLE_TIME: &str = "?";

/// The text of [`SessionState::Unknown`].
const UNKNOWN: &str = "unknown";

impl fmt::Display for SessionState {
    /// Writes the state as the ranking shows it: `busy`, `waiting`,
    /// `idle 3h 12m`, `idle ?`, the text of another status as is, or
    /// `unknown`.
    ///
    /// The idle time is in the format of `occ`, so the two tools print an age
    /// the same way. That format drops the part of a second.
    ///
    /// The text obeys the width and the alignment of the format, the same as
    /// a bare `str`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let idle;
        let text = match self {
            Self::Busy => BUSY,
            Self::Waiting => WAITING,
            Self::Idle { for_: Some(for_) } => {
                idle = format!("{IDLE} {}", occ::format_uptime(for_.as_secs()));
                &idle
            }
            Self::Idle { for_: None } => {
                idle = format!("{IDLE} {UNKNOWN_IDLE_TIME}");
                &idle
            }
            Self::Other(text) => text,
            Self::Unknown => UNKNOWN,
        };
        formatter.pad(text)
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

    /// The idle time is the time from the last change of the status to now.
    /// A record with no time of the change, and a change after now, give no
    /// idle time. A change at now gives zero.
    #[test]
    fn the_idle_time_is_the_time_since_the_status_changed() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let three_hours_twelve = Duration::from_secs(3 * 3_600 + 12 * 60);
        let cases = [
            (Some(now - three_hours_twelve), Some(three_hours_twelve)),
            (Some(now), Some(Duration::ZERO)),
            (Some(now + Duration::from_secs(5)), None),
            (None, None),
        ];

        for (changed_at, for_) in cases {
            assert_eq!(
                SessionState::from_record(&record(Some(SessionStatus::Idle), changed_at), now),
                SessionState::Idle { for_ },
                "changed at {changed_at:?}"
            );
        }
    }

    /// The text of each state: the name of the status, the idle time as `occ`
    /// prints an age, `?` for an unknown idle time, and the text of a status
    /// that `occ` does not know, as is.
    #[test]
    fn each_state_prints_as_the_ranking_shows_it() {
        let cases = [
            (SessionState::Busy, "busy"),
            (SessionState::Waiting, "waiting"),
            (
                SessionState::Idle {
                    for_: Some(Duration::from_secs(3 * 3_600 + 12 * 60 + 59)),
                },
                "idle 3h 12m",
            ),
            (
                SessionState::Idle {
                    for_: Some(Duration::from_secs(2 * 86_400 + 5 * 3_600)),
                },
                "idle 2d 5h",
            ),
            (
                SessionState::Idle {
                    for_: Some(Duration::from_millis(45_900)),
                },
                "idle 45s",
            ),
            (SessionState::Idle { for_: None }, "idle ?"),
            (SessionState::Other("shell".to_owned()), "shell"),
            (SessionState::Unknown, "unknown"),
        ];

        for (state, text) in cases {
            assert_eq!(state.to_string(), text, "the state {state:?}");
        }
    }

    /// A table of processes lines up its columns with a width and an
    /// alignment. The text obeys both, the same as a bare `str`.
    #[test]
    fn a_state_obeys_the_width_and_the_alignment() {
        assert_eq!(format!("{:<9}|", SessionState::Busy), "busy     |");
        assert_eq!(
            format!("{:>8}|", SessionState::Idle { for_: None }),
            "  idle ?|"
        );
        assert_eq!(
            format!("{:^9}|", SessionState::Other("shell".to_owned())),
            "  shell  |"
        );
    }
}
