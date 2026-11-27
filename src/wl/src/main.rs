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

/// What `wl` says about a port for which it named no process.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    /// The line for standard output.
    line: String,
    /// Whether a process that the current user cannot see can explain the
    /// answer. The privilege note applies exactly when this is true.
    hidden_listener_possible: bool,
}

/// The answer for `port` when `wl` named no process on it, from what the
/// kernel's socket table says about the port.
fn verdict(port: u16, status: Option<PortStatus>) -> Verdict {
    let (line, hidden_listener_possible) = match status {
        Some(PortStatus::InUse) => (
            format!("Port {port} is in use, but no owning process is visible to the current user."),
            true,
        ),
        // A complete read of the kernel's socket table lists the sockets of
        // every user, so after Free no hidden process exists, and `sudo` gives
        // the same answer.
        Some(PortStatus::Free) => (format!("No processes listening on port {port}"), false),
        None => (
            format!(
                "Cannot tell whether port {port} is in use: wl reads the kernel's list of listening sockets on macOS and Linux only."
            ),
            true,
        ),
        Some(PortStatus::Unknown) => (
            format!(
                "Cannot tell whether port {port} is in use: wl could not read the kernel's list of listening sockets."
            ),
            true,
        ),
    };
    Verdict {
        line,
        hidden_listener_possible,
    }
}

/// Print the privilege note, when this run has earned one.
///
/// The note applies exactly when a process that the current user cannot see
/// can explain the answer: after a [`Verdict`] whose
/// [`hidden_listener_possible`](Verdict::hidden_listener_possible) is true, and
/// after a failed enumeration of listeners. It does not apply after an answer
/// that named a process, because the user already has the answer and the advice
/// is noise after it.
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
                let answer = verdict(args.port, socket_table::port_status(args.port));
                println!("{}", answer.line);
                if answer.hidden_listener_possible {
                    print_privilege_note(privilege_note);
                }
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

// Each test here reads the privilege note, and only Unix writes one.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn emits_note_for_non_root_euid() {
        assert!(non_root_privilege_note(1000).is_some());
    }

    #[test]
    fn no_note_for_root_euid() {
        assert!(non_root_privilege_note(0).is_none());
    }

    #[test]
    fn note_mentions_sudo() {
        let note = non_root_privilege_note(1000).expect("non-root should produce a note");
        assert!(
            note.contains("sudo"),
            "expected note to tell users to re-run with sudo, got: {note}"
        );
    }

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

#[cfg(test)]
mod verdict_tests {
    use super::*;

    /// The line tells the user that a listener exists but its owner is hidden.
    #[test]
    fn in_use_line_says_the_owner_is_hidden() {
        assert_eq!(
            verdict(8080, Some(PortStatus::InUse)).line,
            "Port 8080 is in use, but no owning process is visible to the current user."
        );
    }

    /// The line tells the user that no process listens on the port.
    #[test]
    fn free_line_says_no_process_listens() {
        assert_eq!(
            verdict(8080, Some(PortStatus::Free)).line,
            "No processes listening on port 8080"
        );
    }

    /// The line tells the user that `wl` could not read the socket table.
    #[test]
    fn unknown_line_says_wl_cannot_tell() {
        assert_eq!(
            verdict(8080, Some(PortStatus::Unknown)).line,
            "Cannot tell whether port 8080 is in use: wl could not read the kernel's list of listening sockets."
        );
    }

    /// On a platform with no reader, `wl` did not try a read, so the line must
    /// not say that a read failed.
    #[test]
    fn no_reader_line_names_the_platforms_that_have_one() {
        assert_eq!(
            verdict(8080, None).line,
            "Cannot tell whether port 8080 is in use: wl reads the kernel's list of listening sockets on macOS and Linux only."
        );
    }

    /// `wl` named no process and cannot show that the port is free, so a hidden
    /// listener can explain the answer.
    #[test]
    fn no_reader_earns_the_privilege_note() {
        assert!(verdict(8080, None).hidden_listener_possible);
    }

    /// A listener exists, and the user cannot see its owner.
    #[test]
    fn in_use_earns_the_privilege_note() {
        assert!(verdict(8080, Some(PortStatus::InUse)).hidden_listener_possible);
    }

    /// The read did not complete, so a hidden listener can explain the answer.
    #[test]
    fn unknown_earns_the_privilege_note() {
        assert!(verdict(8080, Some(PortStatus::Unknown)).hidden_listener_possible);
    }

    /// Free is the answer of a complete read of the socket table.
    #[test]
    fn free_earns_no_privilege_note() {
        assert!(
            !verdict(8080, Some(PortStatus::Free)).hidden_listener_possible,
            "a complete read of the kernel's socket table lists the sockets of \
             every user, so after Free no hidden process exists, and the note is false"
        );
    }
}
