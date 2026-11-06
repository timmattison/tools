//! Who asked: the ancestors of this process and the Claude session.

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
pub(crate) fn walk_ancestors(
    start: u32,
    lookup: impl FnMut(u32) -> Option<ProcessFacts>,
) -> Vec<Ancestor> {
    let _ = (start, lookup, MAX_ANCESTORS, COMMAND_MAX_BYTES);
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
}
