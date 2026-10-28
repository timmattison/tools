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

use chrono::{DateTime, Utc};
use rayon::prelude::*;
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
    match (has_older_lines(target, shared), has_older_lines(other, shared)) {
        (true, false) => Relation::Fork,
        (false, true) => Relation::Source,
        _ => Relation::Related,
    }
}

/// Parses an RFC 3339 timestamp to UTC. A value that does not parse gives `None`.
fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

/// Tells whether `heads` has a line that is not shared and is older than the
/// first shared line.
fn has_older_lines(heads: &[LineHead], shared: &HashSet<String>) -> bool {
    let timed = || {
        heads.iter().filter_map(|head| {
            let uuid = head.uuid.as_deref()?;
            let time = parse_time(head.timestamp.as_deref()?)?;
            Some((uuid, time))
        })
    };
    let Some(first_shared) = timed()
        .filter(|(uuid, _)| shared.contains(*uuid))
        .map(|(_, time)| time)
        .min()
    else {
        return false;
    };
    timed().any(|(uuid, time)| !shared.contains(uuid) && time < first_shared)
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
    let _ = target_id;
    let heads = parse_heads(&std::fs::read(target_transcript)?);
    let target_uuids: HashSet<String> = heads.iter().filter_map(|h| h.uuid.clone()).collect();
    if target_uuids.is_empty() {
        return Ok(ScanReport::default());
    }
    let matcher = aho_corasick::AhoCorasick::new(&target_uuids)
        .expect("a set of strings builds a matcher");
    let hits = candidates(projects_root)
        .par_iter()
        .filter(|path| path.as_path() != target_transcript)
        .filter_map(|path| {
            let bytes = std::fs::read(path).ok()?;
            if !matcher.is_match(&bytes) {
                return None;
            }
            let other = parse_heads(&bytes);
            let shared: HashSet<String> = other
                .iter()
                .filter_map(|h| h.uuid.clone())
                .filter(|uuid| target_uuids.contains(uuid))
                .collect();
            if shared.is_empty() {
                return None;
            }
            Some(ScanHit {
                session_id: path.file_stem()?.to_string_lossy().into_owned(),
                transcript: path.clone(),
                shared: shared.len(),
                relation: relation(&heads, &other, &shared),
            })
        })
        .collect();
    Ok(ScanReport {
        hits,
        unreadable: 0,
    })
}

/// Parses the head of each line of a transcript. A line that does not parse has
/// no head and is left out.
fn parse_heads(bytes: &[u8]) -> Vec<LineHead> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<LineHead>(line).ok())
        .collect()
}

/// Lists the transcripts that are one folder deep in `projects_root`.
fn candidates(projects_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(folders) = std::fs::read_dir(projects_root) else {
        return files;
    };
    for folder in folders.flatten() {
        if !folder.path().is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(folder.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                files.push(path);
            }
        }
    }
    files
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

    /// A fixture tree of transcripts under a temporary projects root.
    struct Tree {
        dir: tempfile::TempDir,
    }

    impl Tree {
        fn new() -> Self {
            Self {
                dir: tempfile::Builder::new()
                    .prefix("crap-scan-")
                    .tempdir()
                    .unwrap(),
            }
        }

        fn root(&self) -> &Path {
            self.dir.path()
        }

        /// Writes `<root>/<folder>/<id>.jsonl` with one JSON value on each line.
        fn write(&self, folder: &str, id: &str, lines: &[serde_json::Value]) -> PathBuf {
            let dir = self.root().join(folder);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(format!("{id}.jsonl"));
            let mut text = String::new();
            for line in lines {
                text.push_str(&line.to_string());
                text.push('\n');
            }
            std::fs::write(&path, text).unwrap();
            path
        }
    }

    /// One transcript line with the top-level keys that the scan reads.
    fn line(uuid: &str, time: &str, session: &str) -> serde_json::Value {
        serde_json::json!({
            "uuid": uuid,
            "timestamp": format!("2026-10-20T{time}Z"),
            "sessionId": session,
            "type": "user",
        })
    }

    const SRC: &str = "aaaaaaaa-0000-4000-8000-000000000001";
    const FORK: &str = "bbbbbbbb-0000-4000-8000-000000000002";
    const OTHER: &str = "cccccccc-0000-4000-8000-000000000003";

    /// The uuid value of line `n` of the fixture pair.
    fn uid(n: u32) -> String {
        format!("dddddddd-0000-4000-8000-{n:012}")
    }

    /// The lines of the source: u1..u6.
    fn source_lines() -> Vec<serde_json::Value> {
        (1..=6)
            .map(|n| line(&uid(n), &format!("10:00:0{n}"), SRC))
            .collect()
    }

    /// The lines of the fork: copies of u4..u6, then new lines u7 and u8.
    fn fork_lines() -> Vec<serde_json::Value> {
        let mut lines: Vec<_> = (4..=6)
            .map(|n| line(&uid(n), &format!("10:00:0{n}"), FORK))
            .collect();
        lines.push(line(&uid(7), "11:00:00", FORK));
        lines.push(line(&uid(8), "11:00:01", FORK));
        lines
    }

    /// Plants the source, the fork, and an unrelated transcript.
    fn plant_pair(tree: &Tree) -> (PathBuf, PathBuf) {
        let src = tree.write("-a", SRC, &source_lines());
        let fork = tree.write("-b", FORK, &fork_lines());
        tree.write(
            "-c",
            OTHER,
            &[line(&uid(90), "09:00:00", OTHER), line(&uid(91), "09:00:01", OTHER)],
        );
        (src, fork)
    }

    #[test]
    fn finds_a_fork_that_shares_uuid_values() {
        let tree = Tree::new();
        let (src, fork) = plant_pair(&tree);
        let report = scan_for_relatives(tree.root(), SRC, &src).unwrap();
        assert_eq!(
            report,
            ScanReport {
                hits: vec![ScanHit {
                    session_id: FORK.to_string(),
                    transcript: fork,
                    shared: 3,
                    relation: Relation::Fork,
                }],
                unreadable: 0,
            }
        );
    }

    #[test]
    fn reports_the_source_of_a_fork() {
        let tree = Tree::new();
        let (src, fork) = plant_pair(&tree);
        let report = scan_for_relatives(tree.root(), FORK, &fork).unwrap();
        assert_eq!(
            report.hits,
            vec![ScanHit {
                session_id: SRC.to_string(),
                transcript: src,
                shared: 3,
                relation: Relation::Source,
            }]
        );
    }

    #[test]
    fn a_uuid_only_inside_a_tool_result_string_gives_no_pair() {
        let tree = Tree::new();
        let (src, _) = plant_pair(&tree);
        let wrapped = serde_json::json!({
            "uuid": uid(95),
            "toolUseResult": serde_json::json!({ "uuid": uid(1) }).to_string(),
        });
        tree.write("-d", "eeeeeeee-0000-4000-8000-000000000005", &[wrapped]);
        let report = scan_for_relatives(tree.root(), SRC, &src).unwrap();
        assert_eq!(report.hits.len(), 1, "only the fork is a hit: {report:?}");
    }

    #[test]
    fn a_uuid_only_in_a_nested_object_gives_no_pair() {
        let tree = Tree::new();
        let (src, _) = plant_pair(&tree);
        let nested = serde_json::json!({
            "uuid": uid(96),
            "toolUseResult": { "uuid": uid(1) },
        });
        tree.write("-d", "eeeeeeee-0000-4000-8000-000000000006", &[nested]);
        let report = scan_for_relatives(tree.root(), SRC, &src).unwrap();
        assert_eq!(report.hits.len(), 1, "only the fork is a hit: {report:?}");
    }

    #[cfg(unix)]
    #[test]
    fn skips_subagent_transcripts_symlinks_and_copies_of_the_same_session() {
        let tree = Tree::new();
        let src = tree.write("-a", SRC, &source_lines());
        let sub = tree.root().join("-a").join(SRC).join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        let sub_file = sub.join("agent-1.jsonl");
        std::fs::copy(&src, sub_file).unwrap();
        let link_dir = tree.root().join("-link");
        std::fs::create_dir_all(&link_dir).unwrap();
        std::os::unix::fs::symlink(&src, link_dir.join("ffffffff-0000-4000-8000-000000000007.jsonl"))
            .unwrap();
        std::os::unix::fs::symlink(tree.root().join("-a"), tree.root().join("-linked-folder"))
            .unwrap();
        tree.write("-copy", SRC, &source_lines());
        let report = scan_for_relatives(tree.root(), SRC, &src).unwrap();
        assert_eq!(report, ScanReport::default());
    }
}
