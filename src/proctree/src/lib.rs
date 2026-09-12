//! The walk of the process ancestry that the tools of this workspace share.
//!
//! A tool asks one question here: does a process of this name stand above this
//! process. The answer names the transport that carries the session. `ic` asks
//! for `mosh-server` and for `etterminal`, because a picture travels
//! differently on each transport. `gsw` asks for `mosh-server`, because a
//! command that opens a browser opens it on the wrong machine.
//!
//! A multiplexer breaks the chain of parents. Zellij starts its server as a
//! daemon, so the server reparents to PID 1. The chain from this process stops
//! at PID 1 and never reaches a terminal. The Zellij client keeps that chain,
//! so the client stands in for this process during the search.
//! [`ZellijScan`] names the clients to search, and it names the clients of this
//! session only. A machine runs many Zellij sessions at the same time, and a
//! client of another session says nothing about how a user views this session.
//!
//! The crate does not answer this question for every multiplexer. tmux starts
//! its server as a daemon in the same way, and this crate holds no equivalent
//! of [`ZellijScan`] for tmux. A session that a user views through tmux
//! therefore reports the transport that stands above the tmux server, which is
//! the transport of the client that started that server. A user who attaches
//! later from another machine gets the wrong answer.
//!
//! The crate holds the mechanics. Each tool holds its own policy over the
//! answers, because each tool does something different with them.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

/// The number of levels that a walk up the process tree examines.
///
/// 64 levels is generous. A real process tree is rarely deeper than 20 levels.
const MAX_ANCESTOR_DEPTH: usize = 64;

/// The basename of the Zellij client program. The match must be exact, so
/// that `zellij-server` and a wrapper script such as `my-zellij-wrapper` do
/// not count as clients.
const ZELLIJ_PROGRAM_NAME: &str = "zellij";

/// A process id.
///
/// The newtype keeps a process id apart from every other number. A pid and a
/// ppid have the same type, and a loop counter does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pid(u32);

impl Pid {
    /// The process id that `pid` names.
    ///
    /// The function is `const`, so a test states a pid as a constant.
    #[must_use]
    pub const fn new(pid: u32) -> Self {
        Self(pid)
    }

    /// The process id of this process.
    #[must_use]
    pub fn current() -> Self {
        Self(std::process::id())
    }
}

/// The basename of a `comm` string.
///
/// On macOS, `ps -eo comm=` gives the full path of the executable, for example
/// `/usr/local/bin/mosh-server`. This function gives the last component of
/// that path, which is the component that an exact match reads.
#[must_use]
pub fn comm_basename(comm: &str) -> &str {
    Path::new(comm)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(comm)
}

/// Holds [`ClientPids`] so that its field stays private to this module. A
/// private tuple field is still reachable from every other line of the file
/// that declares it, and this crate is one file. So the field alone enforces
/// nothing, and the wall of this module is what makes [`ClientPids::new`] the
/// only door.
///
/// The invariant behind that door is that the list is never empty. A caller
/// asks whether *every* client of the session is a client of one transport,
/// and `all` over an empty list answers yes.
mod client_pids {
    use super::Pid;

    /// A list of Zellij clients that holds at least one client.
    ///
    /// [`ClientPids::new`] is the only way to build one, so the empty case gets
    /// an answer once instead of at every call site. The emptiness matters
    /// because a tool asks whether *every* client of the session is a client of
    /// one transport. `all` over an empty list answers yes, so a session with
    /// no named client reports that transport for no reason. That session
    /// belongs to [`super::ZellijScan::EveryClient`] instead.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ClientPids(Vec<Pid>);

    impl ClientPids {
        /// A list that holds `pids`, or `None` when there is no client to hold.
        #[must_use]
        pub fn new(pids: Vec<Pid>) -> Option<Self> {
            if pids.is_empty() {
                return None;
            }
            Some(Self(pids))
        }

        /// The clients, in the order that `ps` reported them. Never empty.
        #[must_use]
        pub fn as_slice(&self) -> &[Pid] {
            &self.0
        }
    }
}

pub use client_pids::ClientPids;

/// Which Zellij clients stand in for the current process during the search.
///
/// Zellij starts its server as a daemon, so the chain from the current process
/// stops at PID 1 and never reaches a terminal. The client keeps that chain,
/// which is why the search reads the client instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZellijScan {
    /// Do not stand in for the current process. This is the scan outside
    /// Zellij, and the scan for a question about direct ancestry, which must
    /// not use the workaround.
    Off,
    /// Every process on the machine whose basename is exactly `zellij`.
    ///
    /// This is the careful answer for a session whose clients carry no name.
    /// It reports a transport that belongs to another session.
    EveryClient,
    /// The clients that are attached to this session, from
    /// [`zellij_client_pids`].
    ///
    /// [`ClientPids`] holds at least one client, so a tool asks whether *every*
    /// client is a client of one transport and gets an answer that a client
    /// stands behind. A session with no named client never arrives here. It
    /// uses [`ZellijScan::EveryClient`].
    Clients(ClientPids),
}

impl ZellijScan {
    /// The scan for a session.
    ///
    /// `session` is `None` outside Zellij. It is `Some("")` inside a Zellij
    /// that names no session, and such a session takes
    /// [`ZellijScan::EveryClient`], because no client carries the name.
    ///
    /// `ps_args_output` is the output of `ps -eo pid=,args=`.
    #[must_use]
    pub fn for_session(ps_args_output: &str, session: Option<&str>) -> Self {
        let Some(session) = session else {
            return Self::Off;
        };
        match ClientPids::new(zellij_client_pids(ps_args_output, session)) {
            Some(clients) => Self::Clients(clients),
            // The session named no client, so the careful answer is the one
            // that treats every Zellij client on the machine as a candidate.
            // It reports a transport of another session, and that mistake
            // hides a feature. The opposite mistake uses a feature that the
            // transport carries badly.
            None => Self::EveryClient,
        }
    }
}

/// The Zellij client processes that are attached to one Zellij session.
///
/// Zellij starts its server as a daemon, so the server reparents to PID 1 and
/// the chain from the current process to the terminal is broken. The client
/// process keeps that chain, so the client stands in for the current process
/// during the search.
///
/// A client must belong to *this* session. A machine runs many Zellij sessions
/// at the same time, and a client of another session says nothing about how a
/// user views this session.
///
/// `ps_args_output` is the output of `ps -eo pid=,args=`. A client is a process
/// whose `argv[0]` basename is exactly `zellij` and that has the session name
/// as a complete argument. The forms `zellij a NAME`, `zellij attach NAME`,
/// `zellij -s NAME`, and `zellij --session NAME` all match. The server process
/// (`zellij --server /path/.../NAME`) does not match, because the session name
/// is only a part of its socket path.
#[must_use]
pub fn zellij_client_pids(ps_args_output: &str, session: &str) -> Vec<Pid> {
    if session.is_empty() {
        return Vec::new();
    }

    let mut clients = Vec::new();

    for line in ps_args_output.lines() {
        let mut parts = line.split_whitespace();
        let pid = match parts.next().and_then(|s| s.parse::<u32>().ok()) {
            Some(p) => Pid(p),
            None => continue,
        };
        let Some(program) = parts.next() else {
            continue;
        };
        if comm_basename(program) != ZELLIJ_PROGRAM_NAME {
            continue;
        }
        // The remaining tokens are the arguments of the client. The session
        // name must be one complete argument.
        if parts.any(|arg| arg == session) {
            clients.push(pid);
        }
    }

    clients
}

/// The Zellij session of this process, from `ZELLIJ` and `ZELLIJ_SESSION_NAME`.
///
/// The answer is `None` outside Zellij. It is `Some("")` inside a Zellij that
/// names no session.
#[must_use]
pub fn zellij_session() -> Option<String> {
    std::env::var("ZELLIJ")
        .ok()
        .map(|_| std::env::var("ZELLIJ_SESSION_NAME").unwrap_or_default())
}

/// The output of `ps -eo pid=,ppid=,comm=`.
///
/// The answer is `None` when the machine cannot run `ps` at all.
#[must_use]
pub fn ps_snapshot() -> Option<String> {
    run_ps(&["-eo", "pid=,ppid=,comm="])
}

/// The output of `ps -eo pid=,args=`.
///
/// The answer is `None` when the machine cannot run `ps` at all.
#[must_use]
pub fn ps_arguments() -> Option<String> {
    run_ps(&["-eo", "pid=,args="])
}

/// Run `ps` with `args` and give back what it wrote to standard output.
///
/// The answer is `None` when the machine cannot run `ps` at all.
fn run_ps(args: &[&str]) -> Option<String> {
    std::process::Command::new("ps")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// A parsed snapshot of the process table.
///
/// The parse happens once, and every question below reads the parsed maps.
#[derive(Debug, Clone, Default)]
pub struct ProcessTree {
    parent_of: HashMap<Pid, Pid>,
    comm_of: HashMap<Pid, String>,
}

impl ProcessTree {
    /// The tree that the output of `ps -eo pid=,ppid=,comm=` describes.
    ///
    /// A line that carries no pid or no ppid is skipped.
    #[must_use]
    pub fn parse(ps_comm_output: &str) -> Self {
        let mut parent_of: HashMap<Pid, Pid> = HashMap::new();
        let mut comm_of: HashMap<Pid, String> = HashMap::new();

        for line in ps_comm_output.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let pid = match parts.next().and_then(|s| s.parse::<u32>().ok()) {
                Some(p) => Pid(p),
                None => continue,
            };
            let ppid = match parts.next().and_then(|s| s.parse::<u32>().ok()) {
                Some(p) => Pid(p),
                None => continue,
            };
            // Rejoin the remaining tokens, so that a path that holds a space
            // survives, for example "/Users/user/my apps/mosh-server". On
            // macOS, `ps -eo comm=` gives the full path of the executable. The
            // `p_comm` field of the kernel holds 16 characters only, but `ps`
            // reads the full path through libproc, so a cut name is not
            // expected. A zombie process and a thread of the kernel show a cut
            // name, and "mosh-server" is 11 characters, which fits.
            let comm: String = parts.collect::<Vec<&str>>().join(" ");
            parent_of.insert(pid, ppid);
            comm_of.insert(pid, comm);
        }

        Self { parent_of, comm_of }
    }

    /// The tree of this machine.
    ///
    /// The answer is `None` when the machine cannot run `ps` at all.
    #[must_use]
    pub fn from_machine() -> Option<Self> {
        ps_snapshot().map(|output| Self::parse(&output))
    }

    /// Whether a process whose basename is `target_name` stands above `pid`, or
    /// is `pid` itself.
    ///
    /// The search has two parts.
    ///
    /// 1. The walk goes up from `pid`. The comm of `pid` itself counts as a
    ///    match.
    /// 2. Zellij starts its server as a daemon, and the server reparents to
    ///    PID 1, which breaks the chain. `scan` names the clients that stand in
    ///    for `pid`, and the walk goes up from each one of them. The comm of a
    ///    client itself does not count as a match, because the client is the
    ///    stand-in and not the process in question.
    #[must_use]
    pub fn has_ancestor(&self, pid: Pid, scan: &ZellijScan, target_name: &str) -> bool {
        // Case 1: the walk goes up from the given process. The process itself
        // counts as a match.
        let mut ancestor = pid;
        for _ in 0..MAX_ANCESTOR_DEPTH {
            if let Some(comm) = self.comm_of.get(&ancestor) {
                if comm_basename(comm) == target_name {
                    return true;
                }
            }
            match self.parent_of.get(&ancestor) {
                Some(&ppid) if ppid != Pid(0) && ppid != ancestor => ancestor = ppid,
                _ => break,
            }
        }

        // Case 2: Zellij started its server as a daemon, which broke the chain
        // of parents. The walk goes up from each client that stands in for the
        // given process.
        let every_client: Vec<Pid>;
        let clients: &[Pid] = match scan {
            ZellijScan::Off => return false,
            ZellijScan::Clients(clients) => clients.as_slice(),
            // Every process whose basename is exactly "zellij", which is the
            // program of the client and not "zellij-server" or another name.
            ZellijScan::EveryClient => {
                every_client = self
                    .comm_of
                    .iter()
                    .filter(|(_, comm)| comm_basename(comm) == ZELLIJ_PROGRAM_NAME)
                    .map(|(&pid, _)| pid)
                    .collect();
                &every_client
            }
        };

        for &client in clients {
            let mut ancestor = client;
            for _ in 0..MAX_ANCESTOR_DEPTH {
                match self.parent_of.get(&ancestor) {
                    Some(&ppid) if ppid != Pid(0) && ppid != ancestor => {
                        if let Some(pcomm) = self.comm_of.get(&ppid) {
                            if comm_basename(pcomm) == target_name {
                                return true;
                            }
                        }
                        ancestor = ppid;
                    }
                    _ => break,
                }
            }
        }

        false
    }
}

/// Everything that the walk needs, read from the machine in one call.
///
/// This is the one-call door for a tool whose whole question is "does a process
/// of this name stand above me". A tool that holds its own policy over the two
/// snapshots reads them as text instead, through [`ps_snapshot`] and
/// [`ps_arguments`].
#[derive(Debug, Clone)]
pub struct Ancestry {
    tree: ProcessTree,
    scan: ZellijScan,
}

impl Ancestry {
    /// Read `ps` and the environment.
    ///
    /// The answer is `None` when the machine cannot run `ps` at all.
    ///
    /// The second `ps` call, for the arguments, happens inside Zellij only.
    /// Nothing else reads the arguments, and a session outside Zellij must not
    /// pay for that call.
    #[must_use]
    pub fn read() -> Option<Self> {
        let tree = ProcessTree::parse(&ps_snapshot()?);
        let session = zellij_session();
        // The comm snapshot deliberately treats every token after the ppid as
        // one path of an executable, so that a path that holds a space
        // survives. The arguments cannot come from that same line without
        // making the path ambiguous, so they need a second call.
        let arguments = if session.is_some() {
            ps_arguments().unwrap_or_default()
        } else {
            String::new()
        };
        let scan = ZellijScan::for_session(&arguments, session.as_deref());
        Some(Self { tree, scan })
    }

    /// Whether a process whose basename is `name` stands above this process.
    #[must_use]
    pub fn has_ancestor(&self, name: &str) -> bool {
        self.tree.has_ancestor(Pid::current(), &self.scan, name)
    }

    /// The parsed process table.
    #[must_use]
    pub fn tree(&self) -> &ProcessTree {
        &self.tree
    }

    /// The clients that stand in for this process.
    #[must_use]
    pub fn scan(&self) -> &ZellijScan {
        &self.scan
    }
}

#[cfg(test)]
mod tests {
    use super::{comm_basename, zellij_client_pids, ClientPids, Pid, ProcessTree, ZellijScan};

    /// Turn the older `in_zellij` flag into a scan. A session whose clients
    /// carry no name uses [`ZellijScan::EveryClient`], which is what
    /// `in_zellij` meant.
    fn scan_for_flag(in_zellij: bool) -> ZellijScan {
        if in_zellij {
            ZellijScan::EveryClient
        } else {
            ZellijScan::Off
        }
    }

    /// Whether Mosh stands above `current_pid`.
    fn has_mosh_in_process_tree(ps_output: &str, current_pid: Pid, in_zellij: bool) -> bool {
        ProcessTree::parse(ps_output).has_ancestor(
            current_pid,
            &scan_for_flag(in_zellij),
            "mosh-server",
        )
    }

    /// Whether Eternal Terminal stands above `current_pid`. The name to find is
    /// `etterminal`, the worker of one session, and not `etserver`, the daemon,
    /// because `etterminal` is the direct parent of the shell of the user.
    fn has_et_in_process_tree(ps_output: &str, current_pid: Pid, in_zellij: bool) -> bool {
        ProcessTree::parse(ps_output).has_ancestor(
            current_pid,
            &scan_for_flag(in_zellij),
            "etterminal",
        )
    }

    // =========================================================================
    // Tests for Pid
    // =========================================================================

    #[test]
    fn a_pid_keeps_the_number_that_built_it() {
        const CURRENT: Pid = Pid::new(63481);
        assert_eq!(CURRENT, Pid::new(63481));
        assert_ne!(CURRENT, Pid::new(63482));
        assert_eq!(
            format!("{CURRENT:?}"),
            "Pid(63481)",
            "a pid must carry the number it was built from"
        );
    }

    // =========================================================================
    // Tests for comm_basename
    // =========================================================================

    #[test]
    fn comm_basename_full_path() {
        assert_eq!(comm_basename("/usr/local/bin/mosh-server"), "mosh-server");
    }

    #[test]
    fn comm_basename_bare_name() {
        assert_eq!(comm_basename("mosh-server"), "mosh-server");
    }

    #[test]
    fn comm_basename_path_with_spaces() {
        // Paths with spaces are reconstructed by the join(" ") in parsing
        assert_eq!(
            comm_basename("/Users/user/my apps/mosh-server"),
            "mosh-server"
        );
    }

    #[test]
    fn comm_basename_empty_string() {
        assert_eq!(comm_basename(""), "");
    }

    // =========================================================================
    // Tests for has_mosh_in_process_tree
    // =========================================================================

    #[test]
    fn mosh_detected_bare_session() {
        // Process tree: mosh-server(100) -> bash(200) -> ic(300)
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /bin/bash
  300   200 /usr/local/bin/ic";
        assert!(has_mosh_in_process_tree(ps_output, Pid::new(300), false));
    }

    #[test]
    fn mosh_detected_bare_name() {
        // comm is just the bare name, no path
        let ps_output = "\
  100     1 mosh-server
  200   100 bash
  300   200 ic";
        assert!(has_mosh_in_process_tree(ps_output, Pid::new(300), false));
    }

    #[test]
    fn mosh_detected_in_zellij() {
        // mosh-server(100) -> zellij CLI(200), but current process(400)
        // is child of zellij-server(300) which reparented to PID 1
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /usr/bin/zellij
  300     1 /usr/bin/zellij-server
  400   300 /bin/bash";
        assert!(has_mosh_in_process_tree(ps_output, Pid::new(400), true));
    }

    #[test]
    fn no_mosh_in_normal_session() {
        let ps_output = "\
    1     0 /sbin/launchd
  500     1 /usr/sbin/sshd
  600   500 /bin/bash
  700   600 /usr/local/bin/ic";
        assert!(!has_mosh_in_process_tree(ps_output, Pid::new(700), false));
    }

    #[test]
    fn mosh_empty_ps_output() {
        assert!(!has_mosh_in_process_tree("", Pid::new(1), false));
    }

    #[test]
    fn mosh_malformed_lines_skipped() {
        let ps_output = "\
not_a_number  1 /bin/bash
  100     1 /usr/bin/mosh-server
  abc   def /foo/bar
  200   100 /bin/bash";
        assert!(has_mosh_in_process_tree(ps_output, Pid::new(200), false));
    }

    #[test]
    fn mosh_zellij_exact_match_no_false_positive() {
        // "my-zellij-wrapper" and "zellij-server" must NOT match as zellij CLI
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /usr/local/bin/my-zellij-wrapper
  300     1 /usr/local/bin/zellij-server
  400   300 /bin/bash";
        assert!(!has_mosh_in_process_tree(ps_output, Pid::new(400), true));
    }

    #[test]
    fn mosh_server_exact_match_no_false_positive() {
        // "mosh-server-wrapper" must NOT match as mosh-server
        let ps_output = "\
  100     1 /usr/bin/mosh-server-wrapper
  200   100 /bin/bash";
        assert!(!has_mosh_in_process_tree(ps_output, Pid::new(200), false));
    }

    #[test]
    fn mosh_comm_path_with_spaces() {
        // Paths with spaces are reconstructed by join(" "),
        // and comm_basename extracts the correct filename
        let ps_output = "\
  100     1 /Users/user/my apps/mosh-server
  200   100 /bin/bash";
        assert!(has_mosh_in_process_tree(ps_output, Pid::new(200), false));
    }

    #[test]
    fn mosh_current_pid_not_in_table() {
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /bin/bash";
        // PID 999 is not in the table
        assert!(!has_mosh_in_process_tree(ps_output, Pid::new(999), false));
    }

    #[test]
    fn mosh_cycle_does_not_loop_forever() {
        // A process whose parent is itself should not cause infinite loop
        let ps_output = "\
  100   100 /bin/bash";
        assert!(!has_mosh_in_process_tree(ps_output, Pid::new(100), false));
    }

    #[test]
    fn mosh_not_detected_when_zellij_env_unset() {
        // Even though a zellij process exists under mosh-server,
        // Case 2 should not trigger when in_zellij is false
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /usr/bin/zellij
  300     1 /usr/bin/zellij-server
  400   300 /bin/bash";
        // current PID 400 is not an ancestor of mosh-server via Case 1,
        // and in_zellij=false disables Case 2
        assert!(!has_mosh_in_process_tree(ps_output, Pid::new(400), false));
    }

    // =========================================================================
    // Tests for the walk above a named client, and for the depth limit
    // =========================================================================

    #[test]
    fn a_named_client_stands_in_for_the_current_process() {
        // mosh-server(100) -> zellij client(200). The current process(400) is
        // a child of the server, which reparented to PID 1.
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /usr/bin/zellij
  300     1 /usr/bin/zellij-server
  400   300 /bin/bash";
        let clients =
            ClientPids::new(vec![Pid::new(200)]).expect("a list with clients is accepted");
        assert!(ProcessTree::parse(ps_output).has_ancestor(
            Pid::new(400),
            &ZellijScan::Clients(clients),
            "mosh-server"
        ));
    }

    #[test]
    fn the_walk_above_a_client_does_not_read_the_client_itself() {
        // The client is the stand-in for the current process, so the comm of
        // the client answers nothing about what stands above it.
        let ps_output = "\
  200     1 /usr/bin/mosh-server
  400     1 /bin/bash";
        let clients =
            ClientPids::new(vec![Pid::new(200)]).expect("a list with clients is accepted");
        assert!(!ProcessTree::parse(ps_output).has_ancestor(
            Pid::new(400),
            &ZellijScan::Clients(clients),
            "mosh-server"
        ));
    }

    /// A chain of `length` processes. Process 1 is the deepest one, process
    /// `length` is the shallowest one, and `mosh-server` stands above them all.
    fn chain_under_mosh(length: u32) -> String {
        let mut table = String::from("  1000     1 /usr/bin/mosh-server\n");
        for pid in 1..=length {
            let parent = if pid == length { 1000 } else { pid + 1 };
            table.push_str(&format!("  {pid}   {parent} /bin/bash\n"));
        }
        table
    }

    #[test]
    fn the_walk_reads_the_last_process_inside_the_depth_limit() {
        // The walk reads 64 processes: 63 shells and then mosh-server.
        let ps_output = chain_under_mosh(63);
        assert!(has_mosh_in_process_tree(&ps_output, Pid::new(1), false));
    }

    #[test]
    fn the_walk_stops_above_the_depth_limit() {
        // mosh-server is the 65th process, which is one above the limit.
        let ps_output = chain_under_mosh(64);
        assert!(!has_mosh_in_process_tree(&ps_output, Pid::new(1), false));
    }

    // =========================================================================
    // Tests for zellij_client_pids (session-scoped client discovery)
    // =========================================================================

    /// A `ps -eo pid=,args=` table with two Zellij sessions and one server.
    const PS_ARGS_TWO_SESSIONS: &str = "\
  51648 /Users/t/.local/bin/zellij --server /tmp/zellij-501/contract_version_1/ic-test
  57053 zellij a ic-test
  32269 zellij a meshtastic
  56666 -zsh";

    #[test]
    fn zellij_client_pids_finds_the_client_of_this_session() {
        assert_eq!(
            zellij_client_pids(PS_ARGS_TWO_SESSIONS, "ic-test"),
            vec![Pid::new(57053)]
        );
    }

    #[test]
    fn zellij_client_pids_ignores_another_sessions_client() {
        let found = zellij_client_pids(PS_ARGS_TWO_SESSIONS, "ic-test");
        assert!(!found.contains(&Pid::new(32269)));
    }

    #[test]
    fn zellij_client_pids_ignores_the_server_of_this_session() {
        // The server has the session name in its socket path, not as an
        // argument of its own. It is not a client.
        let found = zellij_client_pids(PS_ARGS_TWO_SESSIONS, "ic-test");
        assert!(!found.contains(&Pid::new(51648)));
    }

    #[test]
    fn zellij_client_pids_accepts_every_attach_form() {
        let ps_args = "\
  100 zellij a work
  200 zellij attach work
  300 zellij -s work
  400 zellij --session work";
        assert_eq!(
            zellij_client_pids(ps_args, "work"),
            vec![Pid::new(100), Pid::new(200), Pid::new(300), Pid::new(400)]
        );
    }

    #[test]
    fn zellij_client_pids_requires_a_whole_argument_match() {
        // "work" must not match the session named "work-tree".
        let ps_args = "  100 zellij a work-tree";
        assert!(zellij_client_pids(ps_args, "work").is_empty());
    }

    #[test]
    fn zellij_client_pids_requires_an_exact_program_name() {
        // A wrapper script named "my-zellij-wrapper" is not the Zellij CLI.
        let ps_args = "\
  100 /usr/local/bin/my-zellij-wrapper a work
  200 /usr/local/bin/zellij-server a work";
        assert!(zellij_client_pids(ps_args, "work").is_empty());
    }

    #[test]
    fn zellij_client_pids_is_empty_for_an_unknown_session() {
        assert!(zellij_client_pids(PS_ARGS_TWO_SESSIONS, "no-such-session").is_empty());
    }

    #[test]
    fn zellij_client_pids_handles_empty_input() {
        assert!(zellij_client_pids("", "ic-test").is_empty());
    }

    #[test]
    fn zellij_client_pids_skips_malformed_lines() {
        let ps_args = "\
not_a_number zellij a work
  100 zellij a work";
        assert_eq!(zellij_client_pids(ps_args, "work"), vec![Pid::new(100)]);
    }

    #[test]
    fn zellij_client_pids_ignores_an_empty_session_name() {
        // ZELLIJ_SESSION_NAME is unset or empty. No client can be identified.
        assert!(zellij_client_pids(PS_ARGS_TWO_SESSIONS, "").is_empty());
    }

    // =========================================================================
    // Tests for ClientPids (the client list that cannot be empty)
    // =========================================================================

    #[test]
    fn client_pids_refuses_an_empty_list() {
        // An empty list would answer "every client is a Mosh client" for no
        // reason, so it must not be possible to build one.
        assert!(ClientPids::new(Vec::new()).is_none());
    }

    #[test]
    fn client_pids_keeps_a_non_empty_list_in_order() {
        let clients = ClientPids::new(vec![Pid::new(57053), Pid::new(32269)])
            .expect("a list with clients is accepted");
        assert_eq!(clients.as_slice(), [Pid::new(57053), Pid::new(32269)]);
    }

    // =========================================================================
    // Tests for has_et_in_process_tree (Eternal Terminal detection)
    // =========================================================================

    #[test]
    fn et_detected_bare_session() {
        // Process tree: etterminal(100) -> bash(200) -> ic(300)
        let ps_output = "\
  100     1 /usr/bin/etterminal
  200   100 /bin/bash
  300   200 /usr/local/bin/ic";
        assert!(has_et_in_process_tree(ps_output, Pid::new(300), false));
    }

    #[test]
    fn et_detected_bare_name() {
        // comm is just the bare name, no path
        let ps_output = "\
  100     1 etterminal
  200   100 bash
  300   200 ic";
        assert!(has_et_in_process_tree(ps_output, Pid::new(300), false));
    }

    #[test]
    fn et_detected_in_zellij() {
        // etterminal(100) -> zellij CLI(200), but current process(400)
        // is child of zellij-server(300) which reparented to PID 1
        let ps_output = "\
  100     1 /usr/bin/etterminal
  200   100 /usr/bin/zellij
  300     1 /usr/bin/zellij-server
  400   300 /bin/bash";
        assert!(has_et_in_process_tree(ps_output, Pid::new(400), true));
    }

    #[test]
    fn no_et_in_normal_session() {
        let ps_output = "\
    1     0 /sbin/launchd
  500     1 /usr/sbin/sshd
  600   500 /bin/bash
  700   600 /usr/local/bin/ic";
        assert!(!has_et_in_process_tree(ps_output, Pid::new(700), false));
    }

    #[test]
    fn et_not_confused_with_mosh() {
        // mosh-server present but no etterminal
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /bin/bash
  300   200 /usr/local/bin/ic";
        assert!(!has_et_in_process_tree(ps_output, Pid::new(300), false));
    }

    #[test]
    fn mosh_not_confused_with_et() {
        // etterminal present but no mosh-server
        let ps_output = "\
  100     1 /usr/bin/etterminal
  200   100 /bin/bash
  300   200 /usr/local/bin/ic";
        assert!(!has_mosh_in_process_tree(ps_output, Pid::new(300), false));
    }

    #[test]
    fn et_exact_match_no_false_positive() {
        // "etterminal-wrapper" must NOT match as etterminal
        let ps_output = "\
  100     1 /usr/bin/etterminal-wrapper
  200   100 /bin/bash";
        assert!(!has_et_in_process_tree(ps_output, Pid::new(200), false));
    }

    // =========================================================================
    // Tests for ZellijScan::for_session
    // =========================================================================

    #[test]
    fn a_session_outside_zellij_scans_no_client() {
        assert_eq!(
            ZellijScan::for_session(PS_ARGS_TWO_SESSIONS, None),
            ZellijScan::Off
        );
    }

    #[test]
    fn a_session_with_a_client_scans_that_client() {
        let clients =
            ClientPids::new(vec![Pid::new(57053)]).expect("a list with clients is accepted");
        assert_eq!(
            ZellijScan::for_session(PS_ARGS_TWO_SESSIONS, Some("ic-test")),
            ZellijScan::Clients(clients)
        );
    }

    #[test]
    fn a_session_with_no_client_scans_every_client() {
        // The session names no client, so the careful answer reads every
        // Zellij client on the machine.
        assert_eq!(
            ZellijScan::for_session(PS_ARGS_TWO_SESSIONS, Some("no-such-session")),
            ZellijScan::EveryClient
        );
    }

    #[test]
    fn a_session_of_no_name_scans_every_client() {
        // ZELLIJ is set and ZELLIJ_SESSION_NAME is not. No client carries the
        // name of the session, so the careful answer stands.
        assert_eq!(
            ZellijScan::for_session(PS_ARGS_TWO_SESSIONS, Some("")),
            ZellijScan::EveryClient
        );
    }
}
