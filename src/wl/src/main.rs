use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use socket_table::PortStatus;
use std::net::SocketAddr;

mod socket_table;

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
             Re-run with sudo (e.g. `sudo wl <port>`) for complete results.",
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
                match socket_table::port_status(args.port) {
                    PortStatus::InUse => println!(
                        "Port {} is in use, but no owning process is visible to the current user.",
                        args.port
                    ),
                    PortStatus::Free => {
                        println!("No processes listening on port {}", args.port);
                    }
                    PortStatus::Unknown => println!(
                        "Cannot tell whether port {} is in use: wl could not read the kernel's list of listening sockets.",
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
}
