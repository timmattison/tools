//! Reads the output of `lsof`: the Unix sockets of Zellij and the tty of a
//! process.

use proctree::Pid;

/// One Unix socket of a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnixSocket {
    /// The process that holds the socket.
    pub pid: Pid,
    /// The kernel address of the socket.
    pub address: String,
    /// The kernel address of the peer socket. `None` when the socket has a
    /// path name.
    pub peer: Option<String>,
}

/// Reads the sockets from the output of `lsof -a -U -c zellij -F pdn`.
///
/// A `p` line starts a process. A `d` line starts a socket. An `n` line gives
/// the path of the socket or `->` and the address of the peer. A socket with
/// no `d` line is skipped. The function ignores other field letters.
#[must_use]
pub fn parse_unix_sockets(text: &str) -> Vec<UnixSocket> {
    let mut sockets: Vec<UnixSocket> = Vec::new();
    let mut pid: Option<Pid> = None;
    // True while the current file has a socket in `sockets`.
    let mut file_has_socket = false;
    for line in text.lines() {
        let Some(letter) = line.chars().next() else {
            continue;
        };
        let value = &line[letter.len_utf8()..];
        match letter {
            'p' => {
                pid = value.parse().ok().map(Pid::new);
                file_has_socket = false;
            }
            'f' => file_has_socket = false,
            'd' => {
                file_has_socket = pid.is_some();
                if let Some(pid) = pid {
                    sockets.push(UnixSocket {
                        pid,
                        address: value.to_string(),
                        peer: None,
                    });
                }
            }
            'n' if file_has_socket => {
                if let (Some(peer), Some(socket)) = (value.strip_prefix("->"), sockets.last_mut()) {
                    socket.peer = Some(peer.to_string());
                }
            }
            _ => {}
        }
    }
    sockets
}

/// Finds the clients of the Zellij server `server`.
///
/// A client is another process with a socket whose peer is an address of a
/// socket of the server. The sockets that a server holds with itself do not
/// count. The result is sorted and has no duplicate.
#[must_use]
pub fn client_pids(_server: Pid, _sockets: &[UnixSocket]) -> Vec<Pid> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOCKETS: &str = include_str!("../fixtures/lsof-unix-sockets.txt");

    fn socket(pid: u32, address: &str, peer: Option<&str>) -> UnixSocket {
        UnixSocket {
            pid: Pid::new(pid),
            address: address.to_string(),
            peer: peer.map(ToString::to_string),
        }
    }

    #[test]
    fn the_sockets_of_each_pid_have_an_address_and_a_peer() {
        let sockets = parse_unix_sockets(SOCKETS);
        let of = |pid: u32| -> Vec<&UnixSocket> {
            sockets.iter().filter(|s| s.pid == Pid::new(pid)).collect()
        };
        assert_eq!(of(42617).len(), 10);
        assert_eq!(of(42617)[0], &socket(42617, "0x6746b603499c3e29", None));
        assert_eq!(of(42617)[2], &socket(42617, "0x692bb8b027c5345a", None));
        assert_eq!(
            of(19800)[0],
            &socket(19800, "0x5e3c6212ef4c4f72", Some("0x692bb8b027c5345a"))
        );
        assert_eq!(of(91105).len(), 4);
        assert_eq!(sockets.len(), 10 + 4 + 10 + 4);
    }

    #[test]
    fn a_file_with_no_address_line_is_skipped_and_unknown_fields_are_ignored() {
        let text = "p10\nf3\nn/some/path\nf4\ndx1\ncname\nn->y1\np11\nf5\nn/p\n";
        assert_eq!(parse_unix_sockets(text), vec![socket(10, "x1", Some("y1"))]);
    }

    fn pids(numbers: &[u32]) -> Vec<Pid> {
        numbers.iter().copied().map(Pid::new).collect()
    }

    #[test]
    fn the_client_of_each_server_is_found_once() {
        let sockets = parse_unix_sockets(SOCKETS);
        assert_eq!(client_pids(Pid::new(42617), &sockets), pids(&[19800]));
        assert_eq!(client_pids(Pid::new(70267), &sockets), pids(&[91105]));
    }

    #[test]
    fn a_pid_with_no_client_gives_an_empty_list() {
        let sockets = parse_unix_sockets(SOCKETS);
        assert_eq!(client_pids(Pid::new(19800), &sockets), Vec::<Pid>::new());
        assert_eq!(client_pids(Pid::new(1), &sockets), Vec::<Pid>::new());
    }

    #[test]
    fn the_clients_are_sorted() {
        let sockets = vec![
            socket(1, "0xs", None),
            socket(9, "0xa", Some("0xs")),
            socket(5, "0xb", Some("0xs")),
            socket(5, "0xc", Some("0xs")),
        ];
        assert_eq!(client_pids(Pid::new(1), &sockets), pids(&[5, 9]));
    }
}
