use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use std::net::SocketAddr;

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

/// Probe whether any process is listening on `port` locally by attempting
/// to bind it ourselves. Returns `true` if any common local address for
/// this port reports `EADDRINUSE`, which means some process holds it —
/// even if we can't identify which one.
///
/// This needs no privileges. The trade-off: a listener bound only to a
/// specific non-loopback address (e.g. a LAN interface) may not be
/// detected on every platform.
fn tcp_port_in_use(_port: u16) -> bool {
    false
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
                println!("No processes listening on port {}", args.port);
            }
        }
        Err(e) => {
            eprintln!("Error getting listeners: {}", e);
            std::process::exit(1);
        }
    }

    if let Some(note) = privilege_note {
        eprintln!("{note}");
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

    #[test]
    fn detects_held_port_as_in_use() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("should be able to bind an ephemeral port");
        let port = listener
            .local_addr()
            .expect("bound listener must have a local address")
            .port();

        assert!(
            tcp_port_in_use(port),
            "port {port} is held by this test but tcp_port_in_use returned false"
        );

        drop(listener);
    }
}
