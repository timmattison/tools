//! Who asked: the ancestors of this process and the Claude session.

use crate::audit;
use serde::Serialize;

/// One process in the chain above op-cache.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ancestor {
    pub(crate) pid: u32,
    pub(crate) name: String,
    pub(crate) command: Option<String>,
}

/// The Claude Code session that started the chain.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudeSession {
    pub(crate) pid: u32,
    pub(crate) session_id: Option<String>,
    pub(crate) name: Option<String>,
}

/// What one lookup of one PID gives.
pub(crate) struct ProcessFacts {
    pub(crate) parent: Option<u32>,
    pub(crate) name: String,
    pub(crate) command: Option<String>,
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
        });
        match facts.parent {
            Some(next) if parent != 1 => parent = next,
            _ => break,
        }
    }
    chain
}

/// The real chain of this process, one PID lookup at a time.
pub(crate) fn ancestors_of_this_process() -> Vec<Ancestor> {
    vec![]
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
        }
    }

    fn walk(start: u32, table: &HashMap<u32, ProcessFacts>) -> Vec<Ancestor> {
        walk_ancestors(start, |pid| {
            table.get(&pid).map(|f| ProcessFacts {
                parent: f.parent,
                name: f.name.clone(),
                command: f.command.clone(),
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
                    command: Some("nine --y".into())
                },
                Ancestor {
                    pid: 8,
                    name: "eight".into(),
                    command: None
                },
                Ancestor {
                    pid: 1,
                    name: "init".into(),
                    command: Some("/sbin/init".into())
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
        assert!(chain.len() <= 32);
        let mut seen = chain.iter().map(|a| a.pid).collect::<Vec<_>>();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), chain.len(), "a pid repeats");
    }
}
