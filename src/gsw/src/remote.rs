//! Where the person who reads this screen sits.
//!
//! The `G` key of watch mode runs a command, and that command opens a browser.
//! A browser opens on the machine that runs gsw. On a remote shell, no person
//! sits at that machine, so the browser opens where no person looks. The key
//! must ask before it runs. This module answers the question that the key asks
//! first: is this shell a remote shell.
//!
//! The question has two halves, because two programs carry a remote session in
//! two different ways.
//!
//! `sshd` sets `SSH_CONNECTION`, `SSH_CLIENT` and `SSH_TTY` in the shell it
//! starts. One of those variables is sufficient to name a remote session, and a
//! read of the environment costs nothing.
//!
//! `mosh-server` sets none of those variables in the shell it starts. The
//! environment therefore says nothing about a mosh session. The test for mosh
//! is a walk of the process ancestry, and the `proctree` crate does that walk.
//!
//! # What this module does not answer
//!
//! A multiplexer that starts its server as a daemon breaks the chain of
//! parents. The server reparents to PID 1, so the walk from this shell stops
//! there and reaches no terminal. `proctree` repairs that chain for Zellij,
//! because the Zellij client keeps it and stands in for this process.
//! `proctree` holds no equivalent for tmux. A mosh session that a user views
//! through tmux thus reports a local shell.
//!
//! The environment half has the same gap under tmux. A tmux pane takes the
//! environment of the session that started the tmux server. A user who starts
//! the server locally and attaches later through ssh gets a pane that carries
//! no ssh variable, and a user who starts the server through ssh and attaches
//! later at the machine gets a pane that carries the variables of a connection
//! that ended.

use proctree::{Ancestry, Pid, ProcessTree, ZellijScan};

/// The program that mosh runs on the machine the user connected to.
const MOSH_SERVER: &str = "mosh-server";

/// The variables that `sshd` sets in the shell it starts.
///
/// `SSH_AUTH_SOCK` is deliberately not one of them: `ssh-agent` sets that
/// variable on a local desktop session too, so it names an agent and not a
/// remote shell.
const SSH_VARIABLES: [&str; 3] = ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"];

/// Whether the environment holds a variable that `sshd` sets.
///
/// A value rather than a read, so the classifier below takes the environment
/// as an argument. A test that set a variable would change what an unrelated
/// test on another thread sees.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SshEnvironment {
    /// The environment holds one of [`SSH_VARIABLES`] or more.
    Present,
    /// The environment holds none of [`SSH_VARIABLES`].
    Absent,
}

impl SshEnvironment {
    /// Read the environment of this process.
    pub(crate) fn read() -> Self {
        Self::of(|name| std::env::var_os(name).is_some())
    }

    /// Decide from `is_set`, which answers for one variable name.
    pub(crate) fn of(is_set: impl Fn(&str) -> bool) -> Self {
        if SSH_VARIABLES.iter().copied().any(is_set) {
            Self::Present
        } else {
            Self::Absent
        }
    }
}

/// Where the person who reads this screen sits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Session {
    /// The person sits at the machine that runs gsw.
    Local,
    /// The person sits at another machine.
    Remote,
}

impl Session {
    /// Decide from the environment and a process table.
    ///
    /// Everything arrives as an argument, so every rule is testable.
    ///
    /// Each half answers alone. `sshd` states a remote session in the
    /// environment, and `mosh-server` states one in the process ancestry. A
    /// session that either half names is a remote session.
    pub(crate) fn classify(
        ssh: SshEnvironment,
        tree: &ProcessTree,
        pid: Pid,
        scan: &ZellijScan,
    ) -> Self {
        if ssh == SshEnvironment::Present || tree.has_ancestor(pid, scan, MOSH_SERVER) {
            Self::Remote
        } else {
            Self::Local
        }
    }

    /// Read the machine.
    ///
    /// A machine whose `ps` does not run has no table to walk. It reports a
    /// local shell, from the environment alone. That answer is the behavior
    /// gsw has today, so a machine that cannot answer loses no key.
    pub(crate) fn read() -> Self {
        let ssh = SshEnvironment::read();
        let pid = Pid::current();
        match Ancestry::read() {
            Some(ancestry) => Self::classify(ssh, ancestry.tree(), pid, ancestry.scan()),
            None => Self::classify(ssh, &ProcessTree::default(), pid, &ZellijScan::Off),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Session, SshEnvironment};
    use proctree::{Pid, ProcessTree, ZellijScan};

    /// The process that asks the question, in every table below.
    const THIS_PROCESS: Pid = Pid::new(400);

    /// The name of the Zellij session, in every table below.
    const SESSION_NAME: &str = "work";

    /// An environment that holds no variable at all.
    fn clean_environment() -> SshEnvironment {
        SshEnvironment::of(|_| false)
    }

    /// An environment that holds `set` and no other variable.
    fn environment_with(set: &'static str) -> SshEnvironment {
        SshEnvironment::of(move |name| name == set)
    }

    /// A `ps -eo pid=,ppid=,comm=` table of a local session.
    ///
    /// `login` starts the shell, and the shell starts gsw.
    const PS_LOCAL: &str = "\
    1     0 /sbin/launchd
  200     1 /usr/bin/login
  300   200 /bin/zsh
  400   300 /usr/local/bin/gsw";

    /// A `ps -eo pid=,ppid=,comm=` table of a mosh session.
    ///
    /// `mosh-server` starts the shell, and the shell starts gsw.
    const PS_UNDER_MOSH: &str = "\
    1     0 /sbin/launchd
  200     1 /usr/local/bin/mosh-server
  300   200 /bin/zsh
  400   300 /usr/local/bin/gsw";

    /// A `ps -eo pid=,ppid=,comm=` table of a Zellij session under mosh.
    ///
    /// `mosh-server` started the Zellij client. The Zellij server is a daemon,
    /// so it reparented to PID 1, and the walk from process 400 reaches no
    /// terminal.
    const PS_ZELLIJ_UNDER_MOSH: &str = "\
    1     0 /sbin/launchd
  100     1 /usr/local/bin/mosh-server
  200   100 /Users/t/.local/bin/zellij
  300     1 /Users/t/.local/bin/zellij-server
  400   300 /bin/zsh";

    /// A `ps -eo pid=,ppid=,comm=` table of a local Zellij session.
    ///
    /// The table holds the same shape as [`PS_ZELLIJ_UNDER_MOSH`], and `login`
    /// stands where `mosh-server` stands there.
    const PS_ZELLIJ_LOCAL: &str = "\
    1     0 /sbin/launchd
  100     1 /usr/bin/login
  200   100 /Users/t/.local/bin/zellij
  300     1 /Users/t/.local/bin/zellij-server
  400   300 /bin/zsh";

    /// A `ps -eo pid=,args=` table that names the client of [`SESSION_NAME`].
    ///
    /// Process 200 is the client. Process 300 is the server, which carries the
    /// name of the session in its socket path and not as an argument of its
    /// own.
    const PS_ARGS_ZELLIJ: &str = "\
  200 zellij a work
  300 /Users/t/.local/bin/zellij --server /tmp/zellij-501/contract_version_1/work
  400 -zsh";

    /// The scan of the Zellij session that the tables above describe.
    fn zellij_scan() -> ZellijScan {
        ZellijScan::for_session(PS_ARGS_ZELLIJ, Some(SESSION_NAME))
    }

    #[test]
    fn a_clean_environment_and_an_empty_process_table_name_a_local_shell() {
        // This is the machine that cannot run `ps`, and it is also the plain
        // local shell. Both report a local shell, which is the behavior gsw
        // has today.
        assert_eq!(
            Session::classify(
                clean_environment(),
                &ProcessTree::default(),
                THIS_PROCESS,
                &ZellijScan::Off
            ),
            Session::Local,
        );
    }

    #[test]
    fn ssh_connection_alone_names_a_remote_shell() {
        assert_eq!(
            Session::classify(
                environment_with("SSH_CONNECTION"),
                &ProcessTree::parse(PS_LOCAL),
                THIS_PROCESS,
                &ZellijScan::Off
            ),
            Session::Remote,
        );
    }

    #[test]
    fn ssh_client_alone_names_a_remote_shell() {
        assert_eq!(
            Session::classify(
                environment_with("SSH_CLIENT"),
                &ProcessTree::parse(PS_LOCAL),
                THIS_PROCESS,
                &ZellijScan::Off
            ),
            Session::Remote,
        );
    }

    #[test]
    fn ssh_tty_alone_names_a_remote_shell() {
        assert_eq!(
            Session::classify(
                environment_with("SSH_TTY"),
                &ProcessTree::parse(PS_LOCAL),
                THIS_PROCESS,
                &ZellijScan::Off
            ),
            Session::Remote,
        );
    }

    #[test]
    fn ssh_auth_sock_alone_names_a_local_shell() {
        // `ssh-agent` sets SSH_AUTH_SOCK on a local desktop session too, so the
        // variable names an agent and not a remote shell.
        assert_eq!(
            Session::classify(
                environment_with("SSH_AUTH_SOCK"),
                &ProcessTree::parse(PS_LOCAL),
                THIS_PROCESS,
                &ZellijScan::Off
            ),
            Session::Local,
        );
    }

    #[test]
    fn a_mosh_server_above_this_process_names_a_remote_shell() {
        // `mosh-server` sets no ssh variable, so the environment says nothing
        // and the walk of the ancestry gives the whole answer.
        assert_eq!(
            Session::classify(
                clean_environment(),
                &ProcessTree::parse(PS_UNDER_MOSH),
                THIS_PROCESS,
                &ZellijScan::Off
            ),
            Session::Remote,
        );
    }

    #[test]
    fn a_process_table_with_no_mosh_server_names_a_local_shell() {
        assert_eq!(
            Session::classify(
                clean_environment(),
                &ProcessTree::parse(PS_LOCAL),
                THIS_PROCESS,
                &ZellijScan::Off
            ),
            Session::Local,
        );
    }

    #[test]
    fn a_mosh_client_of_this_zellij_session_names_a_remote_shell() {
        // The Zellij server is a daemon, and it reparented to PID 1. The chain
        // from this shell therefore reaches no terminal, and only the client
        // answers.
        assert_eq!(
            Session::classify(
                clean_environment(),
                &ProcessTree::parse(PS_ZELLIJ_UNDER_MOSH),
                THIS_PROCESS,
                &zellij_scan()
            ),
            Session::Remote,
        );
    }

    #[test]
    fn a_zellij_session_whose_client_is_not_a_mosh_client_names_a_local_shell() {
        // This test pins the limit that the header of this module states. The
        // client gives the whole answer, because the chain from this shell
        // stops at PID 1. A multiplexer for which `proctree` names no client
        // gets a local shell for that same reason, and tmux is such a
        // multiplexer.
        assert_eq!(
            Session::classify(
                clean_environment(),
                &ProcessTree::parse(PS_ZELLIJ_LOCAL),
                THIS_PROCESS,
                &zellij_scan()
            ),
            Session::Local,
        );
    }
}
