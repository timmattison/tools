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
fn non_root_privilege_note(_euid: u32) -> Option<&'static str> {
    None
}

fn main() -> Result<()> {
    let args = Args::parse();

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

    Ok(())
}

#[cfg(test)]
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
}
