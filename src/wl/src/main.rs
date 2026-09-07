use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener as StdTcpListener};

/// Show which program is listening on a given port
#[derive(Parser, Debug)]
#[clap(author, version = version_string!(), about)]
struct Args {
    /// The port number to check
    port: u16,

    /// Show detailed socket information
    #[clap(long, short)]
    verbose: bool,
}

/// Note shown to users who lack privileges to enumerate other users' sockets.
///
/// On macOS and Linux, `proc_pidinfo` / `/proc/<pid>/fd` inspection for
/// processes owned by other users requires root. Without it, the underlying
/// `listeners` crate silently skips those processes, producing a partial view
/// that looks identical to "nothing is listening".
#[cfg(unix)]
fn non_root_privilege_note(euid: u32) -> Option<&'static str> {
    if euid == 0 {
        None
    } else {
        Some(
            "note: running without root; processes owned by other users are not visible. \
             Re-run with sudo (e.g. `sudo -E wl <port>`) for complete results.",
        )
    }
}

#[cfg(not(unix))]
fn non_root_privilege_note(_euid: u32) -> Option<&'static str> {
    None
}

#[cfg(unix)]
fn current_euid() -> u32 {
    // SAFETY: `geteuid` is a POSIX syscall with no preconditions; it always
    // succeeds and returns the effective UID of the calling process.
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn current_euid() -> u32 {
    0
}

/// What a bind probe was able to establish about a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortProbe {
    /// Some process holds the port. A bind returned `EADDRINUSE`, which is
    /// proof, even though we can't identify the holder.
    InUse,
    /// Nothing holds the port. The probe bound it itself, or the address the
    /// probe asked for does not exist on this host.
    Free,
    /// The probe could not find out. The kernel refused the bind before it
    /// could reach the question, so a listener may be hiding behind the
    /// refusal.
    Unknown,
}

/// Classify one bind attempt.
///
/// `EADDRINUSE` is the only proof of a holder, and `EADDRNOTAVAIL` is proof of
/// the opposite: the host does not have that address, so nothing can be
/// listening on it. Every other refusal — `EACCES` above all, which is what an
/// unprivileged process gets for a port below the platform's privileged
/// threshold — tells us nothing about the port, only about us.
fn classify_bind_error(kind: io::ErrorKind) -> PortProbe {
    match kind {
        io::ErrorKind::AddrInUse => PortProbe::InUse,
        io::ErrorKind::AddrNotAvailable => PortProbe::Free,
        _ => PortProbe::Unknown,
    }
}

/// Combine the answers from several candidate addresses.
///
/// Precedence is `InUse` > `Unknown` > `Free`: one address that reports a
/// holder settles the question, and short of that, one address the kernel
/// refused keeps the whole answer uncertain — a holder could be sitting behind
/// exactly that refusal.
fn combine_probes(probes: impl IntoIterator<Item = PortProbe>) -> PortProbe {
    let mut result = PortProbe::Free;

    for probe in probes {
        match probe {
            PortProbe::InUse => return PortProbe::InUse,
            PortProbe::Unknown => result = PortProbe::Unknown,
            PortProbe::Free => {}
        }
    }

    result
}

/// Probe whether any process is listening on `port` locally by attempting to
/// bind it ourselves, across the four common local addresses.
///
/// The three answers mean:
///
/// - [`PortProbe::InUse`] — a bind returned `EADDRINUSE`, so some process holds
///   the port, even though this probe can't say which.
/// - [`PortProbe::Free`] — every candidate either bound successfully or does not
///   exist on this host, so nothing is listening on any of them.
/// - [`PortProbe::Unknown`] — the kernel refused at least one bind and no other
///   candidate proved the port is held. A refusal answers a question about the
///   caller's privileges, not about the port, so a listener may hide behind it.
///
/// [`PortProbe::Unknown`] is the common case for an unprivileged process on a
/// privileged port: macOS refuses the loopback binds for a port under 1024, and
/// Linux (`net.ipv4.ip_unprivileged_port_start`, 1024 by default) refuses the
/// wildcard binds too. There is one further gap that no answer covers: a
/// listener bound only to a specific non-loopback address, such as a LAN
/// interface, is not among the candidates and may go undetected.
fn tcp_port_probe(port: u16) -> PortProbe {
    let candidates: [SocketAddr; 4] = [
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ];

    combine_probes(
        candidates
            .iter()
            .map(|addr| match StdTcpListener::bind(addr) {
                Ok(_listener) => PortProbe::Free,
                Err(e) => classify_bind_error(e.kind()),
            }),
    )
}

/// Print the privilege note, when this run has earned one.
///
/// The note applies exactly when `wl` named no listening process. A process
/// the current user cannot see is a live explanation for every answer in which
/// `wl` named nobody — including a failure to enumerate listeners at all. When
/// `wl` did name a process the user already has an answer, and the advice is
/// noise after it.
fn print_privilege_note(note: Option<&str>) {
    if let Some(note) = note {
        eprintln!("{note}");
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let privilege_note = non_root_privilege_note(current_euid());

    match listeners::get_all() {
        Ok(listeners) => {
            let mut found_matches = false;

            for listener in &listeners {
                // Parse the socket address to get the port
                let socket_str = format!("{}", listener.socket);
                if let Ok(socket_addr) = socket_str.parse::<SocketAddr>() {
                    if socket_addr.port() == args.port {
                        found_matches = true;

                        if args.verbose {
                            println!(
                                "PID: {} Process: {} Socket: {} Full: {:?}",
                                listener.process.pid,
                                listener.process.name,
                                listener.socket,
                                listener
                            );
                        } else {
                            println!(
                                "PID: {} Process: {} Socket: {}",
                                listener.process.pid, listener.process.name, listener.socket
                            );
                        }
                    }
                } else {
                    // Handle cases where socket format might not parse as SocketAddr
                    // Look for port number in the socket string
                    if socket_str.contains(&format!(":{}", args.port)) {
                        found_matches = true;

                        if args.verbose {
                            println!(
                                "PID: {} Process: {} Socket: {} Full: {:?}",
                                listener.process.pid,
                                listener.process.name,
                                listener.socket,
                                listener
                            );
                        } else {
                            println!(
                                "PID: {} Process: {} Socket: {}",
                                listener.process.pid, listener.process.name, listener.socket
                            );
                        }
                    }
                }
            }

            if !found_matches {
                match tcp_port_probe(args.port) {
                    PortProbe::InUse => println!(
                        "Port {} is in use, but no owning process is visible to the current user.",
                        args.port
                    ),
                    PortProbe::Free => {
                        println!("No processes listening on port {}", args.port);
                    }
                    PortProbe::Unknown => println!(
                        "Cannot tell whether port {} is in use: the operating system refused the check. Re-run as root for a definite answer.",
                        args.port
                    ),
                }

                print_privilege_note(privilege_note);
            }
        }
        Err(e) => {
            eprintln!("Error getting listeners: {}", e);
            // A failed enumeration names nobody either, and on Unix it most
            // often fails for want of privileges — the one moment the note
            // explains what the user is looking at.
            print_privilege_note(privilege_note);
            std::process::exit(1);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn emits_note_for_non_root_euid() {
        assert!(non_root_privilege_note(1000).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn no_note_for_root_euid() {
        assert!(non_root_privilege_note(0).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn note_mentions_sudo() {
        let note = non_root_privilege_note(1000).expect("non-root should produce a note");
        assert!(
            note.contains("sudo"),
            "expected note to tell users to re-run with sudo, got: {note}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn note_does_not_tell_users_to_pass_dash_e() {
        let note = non_root_privilege_note(1000).expect("non-root should produce a note");
        assert!(
            !note.contains("-E"),
            "wl reads no environment variable, so `sudo -E` adds nothing and a \
             sudoers policy without the SETENV tag refuses it; the note should \
             name the plain command, got: {note}"
        );
    }

    #[test]
    fn detects_held_port_as_in_use() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("should be able to bind an ephemeral port");
        let port = listener
            .local_addr()
            .expect("bound listener must have a local address")
            .port();

        assert_eq!(
            tcp_port_probe(port),
            PortProbe::InUse,
            "port {port} is held by this test but the probe did not report it in use"
        );

        drop(listener);
    }

    #[test]
    fn reports_a_released_port_as_free() {
        // The kernel picks the port, so two copies of this test that run at the
        // same time never ask about the same one.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("should be able to bind an ephemeral port");
        let port = listener
            .local_addr()
            .expect("bound listener must have a local address")
            .port();
        drop(listener);

        assert_eq!(
            tcp_port_probe(port),
            PortProbe::Free,
            "port {port} was released by this test but the probe did not report it free"
        );
    }

    #[test]
    fn classifies_every_bind_error_kind() {
        let cases = [
            (io::ErrorKind::AddrInUse, PortProbe::InUse),
            (io::ErrorKind::AddrNotAvailable, PortProbe::Free),
            (io::ErrorKind::PermissionDenied, PortProbe::Unknown),
            (io::ErrorKind::Other, PortProbe::Unknown),
            (io::ErrorKind::InvalidInput, PortProbe::Unknown),
        ];

        for (kind, expected) in cases {
            assert_eq!(
                classify_bind_error(kind),
                expected,
                "{kind:?} should classify as {expected:?}"
            );
        }
    }

    #[test]
    fn in_use_outranks_every_other_answer() {
        assert_eq!(
            combine_probes([PortProbe::Free, PortProbe::Unknown, PortProbe::InUse]),
            PortProbe::InUse
        );
        assert_eq!(
            combine_probes([PortProbe::InUse, PortProbe::Unknown]),
            PortProbe::InUse
        );
    }

    #[test]
    fn one_refusal_makes_the_whole_answer_unknown() {
        assert_eq!(
            combine_probes([PortProbe::Free, PortProbe::Unknown, PortProbe::Free]),
            PortProbe::Unknown
        );
    }

    #[test]
    fn all_free_candidates_answer_free() {
        assert_eq!(
            combine_probes([PortProbe::Free, PortProbe::Free]),
            PortProbe::Free
        );
    }

    #[test]
    fn no_candidates_answer_free() {
        assert_eq!(combine_probes([]), PortProbe::Free);
    }
}
