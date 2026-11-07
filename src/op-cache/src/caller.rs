//! Who asked: the ancestors of this process and the Claude session.

use crate::audit;
use serde::Serialize;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// When a process started, in whole seconds since the Unix epoch.
///
/// The type keeps seconds apart from the milliseconds that a session file
/// holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessStart(u64);

impl ProcessStart {
    /// Wraps a start time in seconds. It gives `None` for 0, because sysinfo
    /// gives 0 when it cannot read the start time.
    pub(crate) fn from_epoch_seconds(seconds: u64) -> Option<Self> {
        (seconds != 0).then_some(Self(seconds))
    }

    /// True when this process started later than `started_at_ms`, a time in
    /// milliseconds since the Unix epoch. The process start is a whole second,
    /// so a start in the same second is not later.
    pub(crate) fn is_later_than_ms(self, started_at_ms: u64) -> bool {
        self.0.saturating_mul(1000) > started_at_ms
    }
}

/// One process in the chain above op-cache.
///
/// The `start` is never written to the audit record. It lets `claude_session`
/// reject a session file that a dead process left behind.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ancestor {
    pub(crate) pid: u32,
    pub(crate) name: String,
    pub(crate) command: Option<String>,
    #[serde(skip)]
    pub(crate) start: Option<ProcessStart>,
}

/// The Claude Code session that started the chain.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudeSession {
    pub(crate) pid: u32,
    pub(crate) session_id: Option<String>,
    pub(crate) name: Option<String>,
}

/// Who asked: the working directory, the Claude session and the ancestors.
pub(crate) struct Caller {
    pub(crate) cwd: Option<String>,
    pub(crate) claude: Option<ClaudeSession>,
    pub(crate) ancestors: Vec<Ancestor>,
}

/// Gathers the caller context. The environment values come from the caller,
/// so this function never reads the process environment.
pub(crate) fn gather(claude_config_dir_value: Option<&OsStr>, home: Option<&OsStr>) -> Caller {
    let ancestors = ancestors_of_this_process();
    let claude = claude_config_dir(claude_config_dir_value, home)
        .and_then(|dir| claude_session(&ancestors, &dir.join("sessions")));
    Caller {
        cwd: std::env::current_dir()
            .ok()
            .map(|dir| dir.to_string_lossy().into_owned()),
        claude,
        ancestors,
    }
}

/// What one lookup of one PID gives.
pub(crate) struct ProcessFacts {
    pub(crate) parent: Option<u32>,
    pub(crate) name: String,
    pub(crate) command: Option<String>,
    /// When the process started, or `None` when the system does not say.
    pub(crate) start: Option<ProcessStart>,
}

/// The most ancestors one record names.
const MAX_ANCESTORS: usize = 32;

/// The most bytes of one ancestor command line that a record keeps.
const COMMAND_MAX_BYTES: usize = 1024;

/// Walks up the process tree from `start`, which is not in the result.
///
/// `lookup(start)` gives the first parent. The result then holds that parent,
/// its parent, and so on. PID 1 is the last element when the walk reaches it.
/// The walk stops when a lookup gives `None`, when a parent is `None` or 0,
/// before a PID that it saw before (a cycle), and after 32 elements. A parent
/// whose own lookup gives `None` is not added, because there are no facts to
/// write. Each command is cut to 1024 bytes on a char boundary.
pub(crate) fn walk_ancestors(
    start: u32,
    mut lookup: impl FnMut(u32) -> Option<ProcessFacts>,
) -> Vec<Ancestor> {
    let mut chain = Vec::new();
    let mut seen = vec![start];
    let Some(mut parent) = lookup(start).and_then(|facts| facts.parent) else {
        return chain;
    };
    while chain.len() < MAX_ANCESTORS {
        if parent == 0 || seen.contains(&parent) {
            break;
        }
        let Some(facts) = lookup(parent) else {
            break;
        };
        seen.push(parent);
        chain.push(Ancestor {
            pid: parent,
            name: facts.name,
            command: facts.command.map(|command| {
                audit::truncate_at_char_boundary(&command, COMMAND_MAX_BYTES).to_owned()
            }),
            start: facts.start,
        });
        match facts.parent {
            Some(next) if parent != 1 => parent = next,
            _ => break,
        }
    }
    chain
}

/// The real chain of this process.
///
/// It asks the system about one PID at a time, and never scans all processes.
pub(crate) fn ancestors_of_this_process() -> Vec<Ancestor> {
    let mut system = System::new();
    walk_ancestors(std::process::id(), |pid| {
        let pid = Pid::from_u32(pid);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            false,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
        );
        let process = system.process(pid)?;
        let command = process
            .cmd()
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        Some(ProcessFacts {
            parent: process.parent().map(Pid::as_u32),
            name: process.name().to_string_lossy().into_owned(),
            command: (!command.is_empty()).then_some(command),
            start: ProcessStart::from_epoch_seconds(process.start_time()),
        })
    })
}

/// The Claude config directory: a non-empty `CLAUDE_CONFIG_DIR`, else
/// `$HOME/.claude`, else `None`. Pure.
pub(crate) fn claude_config_dir(
    claude_config_dir: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Option<PathBuf> {
    if let Some(dir) = claude_config_dir.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = home.filter(|value| !value.is_empty())?;
    Some(Path::new(home).join(".claude"))
}

/// The nearest ancestor that has `<sessions_dir>/<pid>.json`.
///
/// A missing file moves on to the next ancestor. A file that cannot be read,
/// or is not JSON, still names the ancestor, with no fields. The executable
/// name is never matched.
///
/// A file whose `startedAt` (milliseconds since the Unix epoch) is earlier
/// than the start of the ancestor is stale. A Claude process that dies without
/// cleanup leaves its file, and the kernel can give the PID to a new process.
/// A stale file moves on to the next ancestor, like a missing file. A file with
/// no `startedAt`, a `startedAt` that is not a non-negative integer, or an
/// ancestor with an unknown start is accepted.
pub(crate) fn claude_session(chain: &[Ancestor], sessions_dir: &Path) -> Option<ClaudeSession> {
    for ancestor in chain {
        let file = sessions_dir.join(format!("{}.json", ancestor.pid));
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Some(session_without_fields(ancestor.pid)),
        };
        let Ok(serde_json::Value::Object(fields)) = serde_json::from_str(&text) else {
            return Some(session_without_fields(ancestor.pid));
        };
        let started_at_ms = fields.get("startedAt").and_then(serde_json::Value::as_u64);
        if let (Some(start), Some(started_at_ms)) = (ancestor.start, started_at_ms) {
            if start.is_later_than_ms(started_at_ms) {
                continue;
            }
        }
        let text_field = |key: &str| {
            fields
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        return Some(ClaudeSession {
            pid: ancestor.pid,
            session_id: text_field("sessionId"),
            name: text_field("name"),
        });
    }
    None
}

fn session_without_fields(pid: u32) -> ClaudeSession {
    ClaudeSession {
        pid,
        session_id: None,
        name: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn facts(parent: Option<u32>, name: &str, command: Option<&str>) -> ProcessFacts {
        ProcessFacts {
            parent,
            name: name.to_owned(),
            command: command.map(str::to_owned),
            start: None,
        }
    }

    fn walk(start: u32, table: &HashMap<u32, ProcessFacts>) -> Vec<Ancestor> {
        walk_ancestors(start, |pid| {
            table.get(&pid).map(|f| ProcessFacts {
                parent: f.parent,
                name: f.name.clone(),
                command: f.command.clone(),
                start: f.start,
            })
        })
    }

    fn pids(chain: &[Ancestor]) -> Vec<u32> {
        chain.iter().map(|a| a.pid).collect()
    }

    #[test]
    fn walk_gives_the_parents_up_to_pid_1_without_the_start() {
        let table = HashMap::from([
            (10, facts(Some(9), "ten", Some("ten --x"))),
            (9, facts(Some(8), "nine", Some("nine --y"))),
            (8, facts(Some(1), "eight", None)),
            (1, facts(None, "init", Some("/sbin/init"))),
        ]);
        let chain = walk(10, &table);
        assert_eq!(
            chain,
            vec![
                Ancestor {
                    pid: 9,
                    name: "nine".into(),
                    command: Some("nine --y".into()),
                    start: None
                },
                Ancestor {
                    pid: 8,
                    name: "eight".into(),
                    command: None,
                    start: None
                },
                Ancestor {
                    pid: 1,
                    name: "init".into(),
                    command: Some("/sbin/init".into()),
                    start: None
                },
            ]
        );
    }

    #[test]
    fn walk_ends_at_pid_1_even_when_its_facts_name_a_parent() {
        let table = HashMap::from([
            (10, facts(Some(1), "ten", None)),
            (1, facts(Some(5), "init", None)),
            (5, facts(None, "five", None)),
        ]);
        assert_eq!(pids(&walk(10, &table)), vec![1]);
    }

    #[test]
    fn walk_stops_at_a_parent_that_cannot_be_looked_up() {
        let table = HashMap::from([
            (10, facts(Some(9), "ten", None)),
            (9, facts(Some(8), "nine", None)),
        ]);
        assert_eq!(pids(&walk(10, &table)), vec![9]);
    }

    #[test]
    fn walk_stops_when_the_parent_is_none_or_zero() {
        let none = HashMap::from([(10, facts(None, "ten", None))]);
        assert!(walk(10, &none).is_empty());
        let zero = HashMap::from([
            (10, facts(Some(9), "ten", None)),
            (9, facts(Some(0), "nine", None)),
        ]);
        assert_eq!(pids(&walk(10, &zero)), vec![9]);
    }

    #[test]
    fn walk_stops_before_a_repeat() {
        let table = HashMap::from([
            (10, facts(Some(9), "ten", None)),
            (9, facts(Some(8), "nine", None)),
            (8, facts(Some(9), "eight", None)),
        ]);
        assert_eq!(pids(&walk(10, &table)), vec![9, 8]);
    }

    #[test]
    fn walk_gives_at_most_32_ancestors() {
        let table: HashMap<u32, ProcessFacts> = (100..=140)
            .map(|pid| (pid, facts(Some(pid + 1), "p", None)))
            .collect();
        let chain = walk(100, &table);
        assert_eq!(chain.len(), 32);
        assert_eq!(chain[0].pid, 101);
    }

    #[test]
    fn walk_cuts_a_command_to_1024_bytes_on_a_char_boundary() {
        let long = "é".repeat(1000);
        let table = HashMap::from([
            (10, facts(Some(9), "ten", None)),
            (9, facts(None, "nine", Some(&long))),
        ]);
        let chain = walk(10, &table);
        let command = chain[0].command.as_deref().expect("a command");
        assert!(command.len() <= 1024, "{} bytes", command.len());
        assert!(!command.is_empty());
        assert!(long.starts_with(command));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn this_process_chain_starts_at_the_real_parent() {
        let chain = ancestors_of_this_process();
        let parent = std::os::unix::process::parent_id();
        assert_eq!(chain.first().map(|a| a.pid), Some(parent));
        assert!(!chain[0].name.is_empty());
        assert!(chain[0].start.is_some(), "no start time");
        assert!(chain.len() <= 32);
        let mut seen = chain.iter().map(|a| a.pid).collect::<Vec<_>>();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), chain.len(), "a pid repeats");
    }

    #[test]
    fn config_dir_prefers_a_non_empty_claude_config_dir() {
        let dir = claude_config_dir(Some(OsStr::new("/c")), Some(OsStr::new("/h")));
        assert_eq!(dir, Some(PathBuf::from("/c")));
    }

    #[test]
    fn config_dir_falls_back_to_home_when_empty_or_unset() {
        let home = Some(OsStr::new("/h"));
        let expected = Some(PathBuf::from("/h/.claude"));
        assert_eq!(claude_config_dir(Some(OsStr::new("")), home), expected);
        assert_eq!(claude_config_dir(None, home), expected);
    }

    #[test]
    fn config_dir_is_none_without_a_usable_source() {
        assert_eq!(claude_config_dir(None, None), None);
        assert_eq!(
            claude_config_dir(Some(OsStr::new("")), Some(OsStr::new(""))),
            None
        );
    }

    fn chain_100_200_300() -> Vec<Ancestor> {
        chain_100_200_300_started(None)
    }

    /// The start of every ancestor of the reused-PID tests, in seconds.
    const START: u64 = 1_794_091_714;

    fn chain_100_200_300_started(start: Option<ProcessStart>) -> Vec<Ancestor> {
        [100, 200, 300]
            .into_iter()
            .map(|pid| Ancestor {
                pid,
                name: format!("p{pid}"),
                command: None,
                start,
            })
            .collect()
    }

    fn started_chain() -> Vec<Ancestor> {
        chain_100_200_300_started(ProcessStart::from_epoch_seconds(START))
    }

    fn session_json(started_at_ms: u64, tag: &str) -> String {
        format!(r#"{{"sessionId":"s{tag}","name":"n{tag}","startedAt":{started_at_ms}}}"#)
    }

    fn stale_ms() -> u64 {
        (START - 10) * 1000
    }

    #[test]
    fn process_start_is_none_for_zero_and_some_otherwise() {
        assert_eq!(ProcessStart::from_epoch_seconds(0), None);
        assert!(ProcessStart::from_epoch_seconds(START).is_some());
    }

    #[test]
    fn walk_keeps_the_start_of_each_ancestor() {
        let start = ProcessStart::from_epoch_seconds(START);
        let mut table = HashMap::from([
            (10, facts(Some(9), "ten", None)),
            (9, facts(Some(8), "nine", None)),
            (8, facts(None, "eight", None)),
        ]);
        table.get_mut(&9).expect("nine").start = start;
        let chain = walk(10, &table);
        assert_eq!(chain[0].start, start);
        assert_eq!(chain[1].start, None);
    }

    #[test]
    fn a_stale_file_at_the_nearest_ancestor_gives_way_to_the_next() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(dir.path(), 200, &session_json(stale_ms(), "200"));
        write_session(dir.path(), 300, &session_json(START * 1000, "300"));
        let found = claude_session(&started_chain(), dir.path());
        assert_eq!(
            found,
            Some(ClaudeSession {
                pid: 300,
                session_id: Some("s300".into()),
                name: Some("n300".into())
            })
        );
    }

    #[test]
    fn a_stale_file_alone_gives_no_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(dir.path(), 200, &session_json(stale_ms(), "200"));
        assert_eq!(claude_session(&started_chain(), dir.path()), None);
    }

    #[test]
    fn a_file_started_in_the_same_second_or_later_is_accepted() {
        let dir = tempfile::tempdir().expect("tempdir");
        for started_at in [START * 1000, START * 1000 + 999] {
            write_session(dir.path(), 200, &session_json(started_at, "200"));
            let found = claude_session(&started_chain(), dir.path());
            assert_eq!(found.map(|s| s.pid), Some(200), "startedAt {started_at}");
        }
    }

    #[test]
    fn a_file_without_started_at_is_accepted() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(dir.path(), 200, r#"{"sessionId":"s200"}"#);
        let found = claude_session(&started_chain(), dir.path());
        assert_eq!(found.map(|s| s.pid), Some(200));
    }

    #[test]
    fn an_ancestor_without_a_start_accepts_an_old_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(dir.path(), 200, &session_json(1, "200"));
        let found = claude_session(&chain_100_200_300(), dir.path());
        assert_eq!(found.map(|s| s.pid), Some(200));
    }

    #[test]
    fn a_started_at_that_is_not_a_number_is_accepted() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(dir.path(), 200, r#"{"sessionId":"s200","startedAt":"1"}"#);
        let found = claude_session(&started_chain(), dir.path());
        assert_eq!(found.map(|s| s.pid), Some(200));
    }

    fn write_session(dir: &Path, pid: u32, text: &str) {
        std::fs::write(dir.join(format!("{pid}.json")), text).expect("write session file");
    }

    #[test]
    fn session_comes_from_the_nearest_ancestor_with_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(dir.path(), 200, r#"{"sessionId":"s200","name":"n200"}"#);
        write_session(dir.path(), 300, r#"{"sessionId":"s300","name":"n300"}"#);
        let found = claude_session(&chain_100_200_300(), dir.path());
        assert_eq!(
            found,
            Some(ClaudeSession {
                pid: 200,
                session_id: Some("s200".into()),
                name: Some("n200".into())
            })
        );
        write_session(dir.path(), 100, r#"{"sessionId":"s100","name":"n100"}"#);
        let found = claude_session(&chain_100_200_300(), dir.path());
        assert_eq!(found.map(|s| s.pid), Some(100));
    }

    #[test]
    fn session_is_none_without_any_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(claude_session(&chain_100_200_300(), dir.path()), None);
    }

    #[test]
    fn session_with_bad_json_keeps_the_pid_and_no_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(dir.path(), 200, "not json");
        let found = claude_session(&chain_100_200_300(), dir.path());
        assert_eq!(
            found,
            Some(ClaudeSession {
                pid: 200,
                session_id: None,
                name: None
            })
        );
    }

    #[test]
    fn session_without_a_name_has_no_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(dir.path(), 200, r#"{"sessionId":"s200"}"#);
        let found = claude_session(&chain_100_200_300(), dir.path());
        assert_eq!(
            found,
            Some(ClaudeSession {
                pid: 200,
                session_id: Some("s200".into()),
                name: None
            })
        );
    }

    #[test]
    fn session_field_of_the_wrong_type_is_none_for_that_field_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(dir.path(), 200, r#"{"sessionId":5,"name":"n200"}"#);
        let found = claude_session(&chain_100_200_300(), dir.path());
        assert_eq!(
            found,
            Some(ClaudeSession {
                pid: 200,
                session_id: None,
                name: Some("n200".into())
            })
        );
    }
}
