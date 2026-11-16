//! Finds the Zellij server above a process.

use proctree::{comm_basename, Pid, ProcessTree};

/// The base name of the Zellij program.
const ZELLIJ_PROGRAM: &str = "zellij";

/// Makes the argv that lists each process with its parent and its command.
#[must_use]
pub fn ps_argv() -> Vec<String> {
    ["ps", "-eo", "pid=,ppid=,comm="]
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// Finds the Zellij server above `pid`.
///
/// The server is the first ancestor whose command has the base name `zellij`.
/// The function never uses the name of the session, because `zellij a <prefix>`
/// attaches to a session by a prefix of its name.
#[must_use]
pub fn zellij_server_of(tree: &ProcessTree, pid: Pid) -> Option<Pid> {
    tree.ancestors(pid).into_iter().find(|ancestor| {
        tree.comm(*ancestor)
            .is_some_and(|comm| comm_basename(comm) == ZELLIJ_PROGRAM)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> ProcessTree {
        ProcessTree::parse(include_str!("../fixtures/ps-tree.txt"))
    }

    #[test]
    fn the_ps_argv_asks_for_pid_ppid_and_comm() {
        assert_eq!(ps_argv(), vec!["ps", "-eo", "pid=,ppid=,comm="]);
    }

    #[test]
    fn claude_in_zellij_has_the_server_above_it() {
        assert_eq!(zellij_server_of(&tree(), Pid::new(29130)), Some(Pid::new(42617)));
    }

    #[test]
    fn claude_outside_zellij_has_no_server() {
        assert_eq!(zellij_server_of(&tree(), Pid::new(51234)), None);
    }

    #[test]
    fn an_unknown_pid_has_no_server() {
        assert_eq!(zellij_server_of(&tree(), Pid::new(99999)), None);
    }
}
