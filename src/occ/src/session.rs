//! Which recorded session a running process belongs to.
//!
//! Claude Code writes one transcript per session, at
//! `~/.claude/projects/<encoded working directory>/<session id>.jsonl`. A
//! session process does not hold that file open and does not publish its own
//! session id, so for most processes the link has to be reconstructed.
//!
//! Two things make the reconstruction safe. A process that was given its id on
//! the command line needs no guessing at all. Every other process is matched on
//! the pair (working directory, release), which is far narrower than the working
//! directory alone: one directory on a live machine held twenty-three sessions
//! across twelve releases.
//!
//! Where the pair still does not single out one transcript, this module reports
//! [`Session::Ambiguous`] rather than picking. Reporting one session's id
//! against another session's process would be the worst failure available here,
//! because nothing in the output would say so.

use crate::ClaudeVersion;

/// A Claude Code session id: a canonical UUID.
///
/// Validated on construction so that an id can be used to name a transcript
/// file without re-checking it. Rejecting anything else also guarantees no path
/// separator or traversal sequence rides through to a filesystem lookup.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// Parses a canonical UUID (`8-4-4-4-12` hex digits, case-insensitive).
    ///
    /// # Examples
    ///
    /// ```
    /// use occ::SessionId;
    ///
    /// assert!(SessionId::parse("d3b0d921-f0a1-41fc-b309-c11aa30c1173").is_some());
    /// assert!(SessionId::parse("not-a-session").is_none());
    /// ```
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        /// Hyphen positions in a canonical UUID, and its total length.
        const HYPHEN_POSITIONS: [usize; 4] = [8, 13, 18, 23];
        const UUID_LEN: usize = 36;

        if text.len() != UUID_LEN {
            return None;
        }
        let well_formed = text.bytes().enumerate().all(|(index, byte)| {
            if HYPHEN_POSITIONS.contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
        well_formed.then(|| Self(text.to_string()))
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One recorded transcript found on disk.
#[derive(Debug, Clone)]
pub struct Transcript {
    /// The session the transcript records.
    pub id: SessionId,
    /// The release that wrote the transcript, when it could be read.
    pub version: Option<ClaudeVersion>,
    /// When the transcript was created, in seconds since the epoch.
    ///
    /// Creation is what ties a transcript to a process, not modification. A
    /// process can only have written a transcript that came into being after the
    /// process itself did, and on a machine with months of transcripts in one
    /// directory that distinction removes most of the candidates.
    pub created_epoch_secs: u64,
}

/// The session a running process belongs to, and how firmly that is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    /// The command line names the session outright.
    Named(SessionId),
    /// Exactly one live transcript matches this process, and no other process
    /// competes for it.
    Matched(SessionId),
    /// One process fits several transcripts, and this is the newest of them.
    ///
    /// A session that is cleared starts a new transcript without starting a new
    /// process, so one process legitimately owns several. With no other process
    /// competing, the newest is the one it is writing now. The count travels
    /// with the id so the reader is told this is the newest of several rather
    /// than the only one.
    Likely {
        /// The newest transcript that fits.
        id: SessionId,
        /// How many transcripts fit in total.
        of: usize,
    },
    /// More than one transcript or more than one process fits, and nothing on
    /// the machine separates them.
    Ambiguous {
        /// Transcripts that fit this process.
        candidates: usize,
        /// Processes competing for those transcripts, this one included.
        peers: usize,
    },
    /// No transcript fits: the session has written nothing since it started.
    Unknown,
}

/// Reads a session id out of a command line.
///
/// `--session-id` wins over `--resume`, because a forked session carries both:
/// `--resume` names the transcript it was forked from and `--session-id` names
/// the session actually running. A `--resume` may name either an id or a
/// transcript path.
#[must_use]
pub fn session_id_from_arguments(argv: &[String]) -> Option<SessionId> {
    /// Reads the value of `flag`, accepting both `--flag value` and
    /// `--flag=value`.
    fn value_of<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
        let prefix = format!("{flag}=");
        for (index, argument) in argv.iter().enumerate() {
            if let Some(inline) = argument.strip_prefix(&prefix) {
                return Some(inline);
            }
            if argument == flag {
                return argv.get(index + 1).map(String::as_str);
            }
        }
        None
    }

    /// Reads an id given either directly or as a transcript path.
    fn as_session_id(value: &str) -> Option<SessionId> {
        SessionId::parse(value).or_else(|| {
            let file = std::path::Path::new(value).file_name()?.to_str()?;
            SessionId::parse(file.strip_suffix(".jsonl")?)
        })
    }

    // `--session-id` first: a forked session carries both flags, and only this
    // one names the session that is actually running.
    value_of(argv, "--session-id")
        .and_then(as_session_id)
        .or_else(|| value_of(argv, "--resume").and_then(as_session_id))
}

/// Attributes a session to one process.
///
/// `candidates` are the transcripts recorded for the process's working
/// directory. `peers` counts the session processes sharing that working
/// directory and release, this one included.
#[must_use]
pub fn attribute(
    argv: &[String],
    version: Option<&ClaudeVersion>,
    start_time_epoch_secs: u64,
    candidates: &[Transcript],
    peers: usize,
) -> Session {
    if let Some(named) = session_id_from_arguments(argv) {
        return Session::Named(named);
    }

    let fitting: Vec<&Transcript> = candidates
        .iter()
        .filter(|transcript| {
            // A transcript that existed before this process did records a
            // session this process cannot have opened.
            if transcript.created_epoch_secs < start_time_epoch_secs {
                return false;
            }
            // The release narrows the field, but only when both releases are
            // known. An unknown release must not silently exclude anything.
            match (version, transcript.version.as_ref()) {
                (Some(running), Some(recorded)) => running == recorded,
                _ => true,
            }
        })
        .collect();

    match (fitting.as_slice(), peers) {
        ([], _) => Session::Unknown,
        ([only], 1) => Session::Matched(only.id.clone()),
        _ => Session::Ambiguous {
            candidates: fitting.len(),
            peers,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{attribute, session_id_from_arguments, Session, SessionId, Transcript};
    use crate::ClaudeVersion;

    const SESSION_A: &str = "d3b0d921-f0a1-41fc-b309-c11aa30c1173";
    const SESSION_B: &str = "ed84c8c7-0117-4670-936c-98e0f0d2c80b";
    const SESSION_C: &str = "478e7d63-cde6-47ed-a0de-41915e493a45";

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_string()).collect()
    }

    fn id(text: &str) -> SessionId {
        SessionId::parse(text).expect("test id should parse")
    }

    fn version(text: &str) -> ClaudeVersion {
        ClaudeVersion::parse(text).expect("test version should parse")
    }

    fn transcript(session: &str, release: &str, created: u64) -> Transcript {
        Transcript {
            id: id(session),
            version: Some(version(release)),
            created_epoch_secs: created,
        }
    }

    #[test]
    fn accepts_a_canonical_session_id() {
        assert_eq!(id(SESSION_A).as_str(), SESSION_A);
    }

    #[test]
    fn rejects_anything_that_is_not_a_uuid() {
        for text in ["", "not-a-session", "d3b0d921f0a141fcb309c11aa30c1173", "../etc/passwd"] {
            assert!(SessionId::parse(text).is_none(), "{text:?} must not parse");
        }
    }

    #[test]
    fn reads_the_session_id_from_the_command_line() {
        let found = session_id_from_arguments(&argv(&["claude", "--session-id", SESSION_B]));
        assert_eq!(found, Some(id(SESSION_B)));
    }

    #[test]
    fn reads_a_resumed_session_id() {
        let found = session_id_from_arguments(&argv(&["claude", "--resume", SESSION_B]));
        assert_eq!(found, Some(id(SESSION_B)));
    }

    #[test]
    fn reads_a_resumed_transcript_path() {
        let path = format!("/Users/u/.claude/projects/-work/{SESSION_C}.jsonl");
        let found = session_id_from_arguments(&argv(&["claude", "--resume", &path]));
        assert_eq!(found, Some(id(SESSION_C)));
    }

    #[test]
    fn a_forked_session_reports_the_session_it_runs_not_the_one_it_forked_from() {
        // Observed on a live machine: `--resume` names the transcript the fork
        // started from, and `--session-id` names the session actually running.
        let path = format!("/Users/u/.claude/projects/-work/{SESSION_C}.jsonl");
        let found = session_id_from_arguments(&argv(&[
            "claude",
            "--session-id",
            SESSION_B,
            "--fork-session",
            "--resume",
            &path,
        ]));
        assert_eq!(found, Some(id(SESSION_B)));
    }

    #[test]
    fn a_bare_resume_names_no_session() {
        // `--resume` with nothing after it opens a picker.
        assert_eq!(session_id_from_arguments(&argv(&["claude", "--resume"])), None);
        assert_eq!(session_id_from_arguments(&argv(&["claude", "--continue"])), None);
    }

    #[test]
    fn a_named_session_needs_no_transcript() {
        let found = attribute(
            &argv(&["claude", "--session-id", SESSION_B]),
            Some(&version("2.1.197")),
            1_000,
            &[],
            9,
        );
        assert_eq!(found, Session::Named(id(SESSION_B)));
    }

    #[test]
    fn a_lone_process_matches_its_lone_transcript() {
        let found = attribute(
            &argv(&["claude"]),
            Some(&version("2.1.205")),
            1_000,
            &[transcript(SESSION_A, "2.1.205", 2_000)],
            1,
        );
        assert_eq!(found, Session::Matched(id(SESSION_A)));
    }

    #[test]
    fn the_release_separates_transcripts_that_share_a_directory() {
        // The real case this rule exists for: one worktree, many sessions, each
        // pinned to the release it started on.
        let candidates = [
            transcript(SESSION_A, "2.1.205", 2_000),
            transcript(SESSION_B, "2.1.202", 2_000),
            transcript(SESSION_C, "2.1.218", 2_000),
        ];
        let found = attribute(
            &argv(&["claude"]),
            Some(&version("2.1.202")),
            1_000,
            &candidates,
            1,
        );
        assert_eq!(found, Session::Matched(id(SESSION_B)));
    }

    #[test]
    fn a_transcript_created_before_the_process_started_is_not_a_candidate() {
        // That transcript records a session that already existed before this
        // process did, so this process cannot be the one that opened it.
        let found = attribute(
            &argv(&["claude"]),
            Some(&version("2.1.205")),
            5_000,
            &[transcript(SESSION_A, "2.1.205", 4_999)],
            1,
        );
        assert_eq!(found, Session::Unknown);
    }

    #[test]
    fn an_older_transcript_still_being_written_is_not_a_candidate() {
        // The distinction that removes most false candidates on a busy machine:
        // a long-lived process makes every transcript touched since it started
        // look plausible, and only creation time rules them out.
        let older = Transcript {
            id: id(SESSION_A),
            version: Some(version("2.1.205")),
            created_epoch_secs: 500,
        };
        let mine = transcript(SESSION_B, "2.1.205", 1_500);

        let found = attribute(
            &argv(&["claude"]),
            Some(&version("2.1.205")),
            1_000,
            &[older, mine],
            1,
        );

        assert_eq!(found, Session::Matched(id(SESSION_B)));
    }

    #[test]
    fn a_lone_process_owning_several_transcripts_reports_the_newest() {
        // A cleared session starts a new transcript under the same process, so
        // one process owns several. With nothing else competing, the newest is
        // the one it is writing now.
        let candidates = [
            transcript(SESSION_A, "2.1.202", 2_000),
            transcript(SESSION_B, "2.1.202", 3_000),
        ];
        let found = attribute(
            &argv(&["claude"]),
            Some(&version("2.1.202")),
            1_000,
            &candidates,
            1,
        );
        assert_eq!(
            found,
            Session::Likely {
                id: id(SESSION_B),
                of: 2
            }
        );
    }

    #[test]
    fn several_processes_sharing_transcripts_stay_ambiguous() {
        // With more than one process competing, the newest transcript belongs to
        // whichever of them wrote last, and nothing here says which. A guess
        // would report one session's id against another session's process.
        let candidates = [
            transcript(SESSION_A, "2.1.202", 2_000),
            transcript(SESSION_B, "2.1.202", 3_000),
        ];
        let found = attribute(
            &argv(&["claude"]),
            Some(&version("2.1.202")),
            1_000,
            &candidates,
            2,
        );
        assert_eq!(found, Session::Ambiguous { candidates: 2, peers: 2 });
    }

    #[test]
    fn a_lone_transcript_contested_by_peers_stays_ambiguous() {
        // Three processes of one release in one directory cannot all be the one
        // transcript. Naming it for each of them would report one session's id
        // against two processes that are not it.
        let found = attribute(
            &argv(&["claude"]),
            Some(&version("2.1.210")),
            1_000,
            &[transcript(SESSION_A, "2.1.210", 2_000)],
            3,
        );
        assert_eq!(found, Session::Ambiguous { candidates: 1, peers: 3 });
    }

    #[test]
    fn no_transcript_at_all_is_unknown() {
        let found = attribute(&argv(&["claude"]), Some(&version("2.1.221")), 1_000, &[], 1);
        assert_eq!(found, Session::Unknown);
    }

    #[test]
    fn an_unreadable_release_does_not_narrow_the_candidates() {
        // With no release to match on, every live transcript in the directory
        // stays a candidate rather than being excluded.
        let candidates = [
            transcript(SESSION_A, "2.1.202", 2_000),
            transcript(SESSION_B, "2.1.218", 3_000),
        ];
        let found = attribute(&argv(&["claude"]), None, 1_000, &candidates, 1);
        assert_eq!(
            found,
            Session::Likely {
                id: id(SESSION_B),
                of: 2
            }
        );
    }
}
