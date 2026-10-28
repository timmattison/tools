//! The fork scan: find the transcripts that share lines with one transcript.
//!
//! When Claude Code forks a session, it copies the chain of lines into the new
//! transcript. Each copied line keeps its top-level `uuid` and `timestamp`, and
//! it gets the new `sessionId`. A top-level `uuid` value that two transcripts
//! share is thus the mark of a fork. The fork log does not hold forks that were
//! made before the log existed, or that another tool made. The scan finds them.
//!
//! The scan reads the transcript of the target and collects its `uuid` values.
//! It then lists the transcripts of the projects tree, one folder deep. It skips
//! subagent transcripts, symlinks, and copies of the target session. A
//! multi-pattern substring search on the raw bytes of each file removes the
//! files that cannot share a value. The search cannot miss a related file,
//! because a UUID needs no JSON escape. Only the files that pass are parsed.
//!
//! The parse reads the top-level keys of each line only. A `uuid` inside a tool
//! result or another nested value is thus never a shared value.
//!
//! The direction comes from the timestamps. A fork has no line that is older
//! than its first shared line, because Claude copies the chain from its start
//! and then adds new lines. The source of the fork can have older lines that the
//! fork does not hold. The rule is [`relation`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// How another transcript relates to the target transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// The other transcript is a fork of the target.
    Fork,
    /// The other transcript is the source of the target.
    Source,
    /// The two transcripts share lines, but the data does not show the direction.
    Related,
}

impl Relation {
    /// The word that the report prints: `fork`, `source`, or `related`.
    pub fn as_str(self) -> &'static str {
        match self {
            Relation::Fork => "fork",
            Relation::Source => "source",
            Relation::Related => "related",
        }
    }
}

impl std::fmt::Display for Relation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// The top-level keys of one transcript line that the scan reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct LineHead {
    /// The top-level `uuid` of the line.
    pub uuid: Option<String>,
    /// The top-level `timestamp` of the line, in RFC 3339.
    pub timestamp: Option<String>,
}

/// One transcript that shares `uuid` values with the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanHit {
    /// The session id of the transcript: the stem of its file name.
    pub session_id: String,
    /// The path of the transcript.
    pub transcript: PathBuf,
    /// The count of distinct `uuid` values that the transcript shares with the target.
    pub shared: usize,
    /// How the transcript relates to the target.
    pub relation: Relation,
}

/// The result of one scan.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Each transcript that shares a value, most shared values first.
    pub hits: Vec<ScanHit>,
    /// The count of transcripts and folders that the scan could not read.
    pub unreadable: usize,
}

/// Decides how `other` relates to `target`, from the lines they share.
///
/// A transcript has older lines when it has a line whose `uuid` is not in
/// `shared` and whose timestamp is earlier than its first shared line. When only
/// the target has older lines, the other transcript is a fork. When only the
/// other transcript has older lines, it is the source. In each other case the
/// result is [`Relation::Related`]. A line without a `uuid`, or with a
/// timestamp that does not parse, does not count.
pub fn relation(target: &[LineHead], other: &[LineHead], shared: &HashSet<String>) -> Relation {
    let _ = (target, other, shared);
    Relation::Related
}

/// Scans `projects_root` for transcripts that share `uuid` values with the target.
///
/// # Errors
///
/// Returns the error when `target_transcript` cannot be read.
pub fn scan_for_relatives(
    projects_root: &Path,
    target_id: &str,
    target_transcript: &Path,
) -> std::io::Result<ScanReport> {
    let _ = (projects_root, target_id, target_transcript);
    Ok(ScanReport::default())
}

/// Formats the report of a scan for the terminal.
pub fn format_scan_report(id: &str, projects_root: &Path, report: &ScanReport) -> String {
    let _ = (id, projects_root, report);
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line head with a uuid and a time on one fixed day.
    fn head(uuid: &str, time: &str) -> LineHead {
        LineHead {
            uuid: Some(uuid.to_string()),
            timestamp: Some(format!("2026-10-20T{time}Z")),
        }
    }

    fn set(values: &[&str]) -> HashSet<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    /// The source: u1..u6 at 10:00..10:05.
    fn source_heads() -> Vec<LineHead> {
        (1..=6)
            .map(|n| head(&format!("u{n}"), &format!("10:00:0{n}")))
            .collect()
    }

    /// The fork: copies of u4..u6, then new lines u7 and u8.
    fn fork_heads() -> Vec<LineHead> {
        vec![
            head("u4", "10:00:04"),
            head("u5", "10:00:05"),
            head("u6", "10:00:06"),
            head("u7", "11:00:00"),
            head("u8", "11:00:01"),
        ]
    }

    #[test]
    fn relation_names_the_other_transcript_a_fork_when_only_the_target_has_older_lines() {
        let shared = set(&["u4", "u5", "u6"]);
        assert_eq!(
            relation(&source_heads(), &fork_heads(), &shared),
            Relation::Fork
        );
    }

    #[test]
    fn relation_names_the_other_transcript_the_source_when_only_it_has_older_lines() {
        let shared = set(&["u4", "u5", "u6"]);
        assert_eq!(
            relation(&fork_heads(), &source_heads(), &shared),
            Relation::Source
        );
    }

    #[test]
    fn relation_is_related_when_both_transcripts_start_with_the_shared_lines() {
        let a = vec![head("u1", "10:00:01"), head("u2", "10:00:02"), head("a", "12:00:00")];
        let b = vec![head("u1", "10:00:01"), head("u2", "10:00:02"), head("b", "13:00:00")];
        let shared = set(&["u1", "u2"]);
        assert_eq!(relation(&a, &b, &shared), Relation::Related);
    }

    #[test]
    fn relation_is_related_without_timestamps() {
        let bare = |u: &str| LineHead {
            uuid: Some(u.to_string()),
            timestamp: None,
        };
        let a = vec![bare("x"), bare("u1")];
        let b = vec![bare("u1")];
        assert_eq!(relation(&a, &b, &set(&["u1"])), Relation::Related);
    }
}
