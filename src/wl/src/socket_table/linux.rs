//! Read Linux's own list of TCP sockets and answer for one port.
//!
//! The kernel publishes every TCP socket as a line of text: `/proc/net/tcp`
//! carries the IPv4 sockets and `/proc/net/tcp6` the IPv6 ones. Both files are
//! world-readable and both list every socket on the host whoever owns it — the
//! same tables `netstat` and `ss` read. That is the reach a bind probe never
//! had: a port held by another user's process answers here, and so does a port
//! below the privileged threshold, where a bind is refused before the question
//! is reached.
//!
//! Each file opens with a header line naming its columns and carries one line
//! per socket:
//!
//! ```text
//!   sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
//!    0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 18716 1 ...
//! ```
//!
//! Two columns answer the question. `local_address` holds the address and the
//! port, both hexadecimal, separated by a colon; the port is the half this
//! module reads, and the address half differs only in width between the two
//! files — eight hexadecimal digits for IPv4, thirty-two for IPv6. `st` holds
//! the socket's TCP state, and `0A` is `TCP_LISTEN`
//! (`include/net/tcp_states.h`). A socket in any other state holds a local port
//! without accepting on it, which is not what `wl` was asked about.
//!
//! # A wrong reading is loud, not silent
//!
//! The header names the two columns this module reads, at the two positions it
//! reads them from, so a column inserted ahead of either one is caught by the
//! header rather than by a misread row. A header that does not name
//! `local_address` and `st` where this module expects them answers
//! [`PortStatus::Unknown`], and so does a file with no header line at all.
//!
//! Every body row is then read in full, and one row that does not parse makes
//! the whole answer [`PortStatus::Unknown`] — even a row after the listening
//! socket this module was looking for. A table whose rows stop making sense may
//! have been misread from its first row, so the listener it appeared to hold is
//! not proof either. Half a table read cleanly is not a reading of the table.
//!
//! The two files are read under the same rule, with one exception. A missing
//! `/proc/net/tcp6` is not a failure: a kernel built or booted without IPv6
//! publishes no table for it, and nothing can be listening on a protocol the
//! host does not carry, so the IPv4 table answers alone. A `/proc/net/tcp6`
//! that exists and cannot be read is a table this run did not read, and answers
//! [`PortStatus::Unknown`] like any other. A missing `/proc/net/tcp` is a
//! failure outright: the file is unconditional on a kernel that carries
//! `/proc`, so its absence says this run is not reading what it thinks it is.
//!
//! [`PortStatus::Free`] is only ever the answer of two complete readings that
//! found no listening socket on the port.
//!
//! The parsing half compiles on every platform so that its captured-table tests
//! run on every platform. Only the two file reads that feed it are Linux-only.

#![cfg_attr(
    not(target_os = "linux"),
    allow(
        dead_code,
        reason = "the text parser compiles everywhere so its captured-table tests \
                  run everywhere; only the /proc reads that feed it are Linux-only"
    )
)]

use super::PortStatus;

/// `TCP_LISTEN` (`include/net/tcp_states.h`), the value the `st` column carries
/// for a socket that has had `listen()` called on it.
const TCP_LISTEN: u8 = 0x0A;

/// The column holding the local address and port, counting from zero.
const LOCAL_ADDRESS_COLUMN: usize = 1;

/// The column holding the socket's TCP state, counting from zero.
const STATE_COLUMN: usize = 3;

/// What the header calls [`LOCAL_ADDRESS_COLUMN`], in both files.
const LOCAL_ADDRESS_HEADING: &str = "local_address";

/// What the header calls [`STATE_COLUMN`], in both files.
const STATE_HEADING: &str = "st";

/// Width of the address half of `local_address` in `/proc/net/tcp`.
const IPV4_ADDRESS_HEX_DIGITS: usize = 8;

/// Width of the address half of `local_address` in `/proc/net/tcp6`.
const IPV6_ADDRESS_HEX_DIGITS: usize = 32;

/// Width of the port half of `local_address`, in both files.
const PORT_HEX_DIGITS: usize = 4;

/// The kernel's table of IPv4 TCP sockets.
#[cfg(target_os = "linux")]
const PROC_NET_TCP: &str = "/proc/net/tcp";

/// The kernel's table of IPv6 TCP sockets.
#[cfg(target_os = "linux")]
const PROC_NET_TCP6: &str = "/proc/net/tcp6";

/// Answer for `port` from the kernel's tables of TCP sockets.
///
/// A table this call cannot read answers [`PortStatus::Unknown`]: the question
/// was never reached, so nothing is known about the port. The one absence that
/// is not a failure is `/proc/net/tcp6` — see the module documentation.
#[cfg(target_os = "linux")]
pub fn port_status(port: u16) -> PortStatus {
    let Ok(ipv4) = std::fs::read_to_string(PROC_NET_TCP) else {
        return PortStatus::Unknown;
    };

    let ipv6 = match std::fs::read_to_string(PROC_NET_TCP6) {
        Ok(table) => parse_port_status(&table, port),
        // No IPv6 on this host, so no IPv6 socket can be holding the port.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => PortStatus::Free,
        // The table is there and this run did not read it.
        Err(_) => return PortStatus::Unknown,
    };

    combine(parse_port_status(&ipv4, port), ipv6)
}

/// Combine the answers from the IPv4 table and the IPv6 table.
///
/// Precedence is `Unknown` > `InUse` > `Free`. One table this run could not
/// read through keeps the whole answer uncertain, even when the other found a
/// listener: the two tables are printed by one kernel from one pair of routines,
/// so a row neither file's layout explains is evidence against both readings,
/// not just against the one it appeared in.
fn combine(ipv4: PortStatus, ipv6: PortStatus) -> PortStatus {
    match (ipv4, ipv6) {
        (PortStatus::Unknown, _) | (_, PortStatus::Unknown) => PortStatus::Unknown,
        (PortStatus::InUse, _) | (_, PortStatus::InUse) => PortStatus::InUse,
        (PortStatus::Free, PortStatus::Free) => PortStatus::Free,
    }
}

/// The whitespace-separated column at `index`, counting from zero.
fn column(line: &str, index: usize) -> Option<&str> {
    line.split_whitespace().nth(index)
}

/// Say whether the header names the two columns this module reads, where it
/// reads them.
///
/// A column inserted ahead of either one moves both, so the check on the names
/// is a check on the positions.
fn header_names_the_columns(header: &str) -> bool {
    column(header, LOCAL_ADDRESS_COLUMN) == Some(LOCAL_ADDRESS_HEADING)
        && column(header, STATE_COLUMN) == Some(STATE_HEADING)
}

/// Read the port out of a `local_address` column.
///
/// `None` means the column is not the `<address>:<port>` pair either file
/// prints — the one answer that must never be read as "free".
fn local_port_of(local_address: &str) -> Option<u16> {
    let (address, port) = local_address.split_once(':')?;

    let address_is_whole = (address.len() == IPV4_ADDRESS_HEX_DIGITS
        || address.len() == IPV6_ADDRESS_HEX_DIGITS)
        && address.bytes().all(|b| b.is_ascii_hexdigit());
    let port_is_whole =
        port.len() == PORT_HEX_DIGITS && port.bytes().all(|b| b.is_ascii_hexdigit());
    if !address_is_whole || !port_is_whole {
        return None;
    }

    u16::from_str_radix(port, 16).ok()
}

/// Say whether one body row is a socket listening on `port`.
///
/// `None` means the row did not match the layout this module describes.
fn row_listens_on(row: &str, port: u16) -> Option<bool> {
    let local_port = local_port_of(column(row, LOCAL_ADDRESS_COLUMN)?)?;
    let state = u8::from_str_radix(column(row, STATE_COLUMN)?, 16).ok()?;

    Some(state == TCP_LISTEN && local_port == port)
}

/// Answer for `port` from one of the kernel's socket tables.
///
/// Split from the file reads so a captured table can exercise it on any
/// platform, `/proc` or no `/proc`.
fn parse_port_status(table: &str, port: u16) -> PortStatus {
    let mut lines = table.lines();

    let Some(header) = lines.next() else {
        // Not even a header line: this is not the table this module reads.
        return PortStatus::Unknown;
    };
    if !header_names_the_columns(header) {
        return PortStatus::Unknown;
    }

    let mut found_listener = false;

    for row in lines {
        // A line carrying nothing but whitespace names no socket.
        if row.trim().is_empty() {
            continue;
        }
        match row_listens_on(row, port) {
            Some(listening) => found_listener |= listening,
            None => return PortStatus::Unknown,
        }
    }

    if found_listener {
        PortStatus::InUse
    } else {
        PortStatus::Free
    }
}

#[cfg(test)]
mod tests {
    use super::{combine, parse_port_status, PortStatus};

    /// A `/proc/net/tcp` as the kernel prints it, header line included.
    ///
    /// Row 0 is `sshd` listening on every IPv4 address, port `0016` (22).
    /// Row 1 is `cupsd` listening on loopback, port `0277` (631). Row 2 is an
    /// established connection out of the ephemeral port `8AE6` (35558) — a
    /// local port held by a socket that accepts nothing.
    const PROC_NET_TCP_SAMPLE: &str = concat!(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
        "   0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 18716 1 0000000000000000 100 0 0 10 0\n",
        "   1: 0100007F:0277 00000000:0000 0A 00000000:00000000 00:00000000 00000000   101        0 19024 1 0000000000000000 100 0 0 10 0\n",
        "   2: 0100007F:8AE6 0100007F:0277 01 00000000:00000000 00:00000000 00000000  1000        0 41225 1 0000000000000000 20 4 30 10 -1\n",
    );

    /// A `/proc/net/tcp6` as the kernel prints it, header line included.
    ///
    /// The columns carry the same two facts under wider addresses, and the
    /// header calls the remote one `remote_address` rather than `rem_address`.
    /// Row 0 is `sshd` on port `0016` (22); row 1 is `cupsd` on port `0277`
    /// (631), bound to `::1`.
    const PROC_NET_TCP6_SAMPLE: &str = concat!(
        "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
        "   0: 00000000000000000000000000000000:0016 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 18718 1 0000000000000000 100 0 0 10 0\n",
        "   1: 00000000000000000000000001000000:0277 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000   101        0 19026 1 0000000000000000 100 0 0 10 0\n",
    );

    /// The port row 0 of both captured tables is listening on.
    const SSH_PORT: u16 = 22;

    /// The port row 1 of both captured tables is listening on.
    const CUPS_PORT: u16 = 631;

    /// The local port of the established connection in the IPv4 table, held by
    /// a socket that is not listening.
    const ESTABLISHED_PORT: u16 = 35558;

    /// A table built from a captured header and rows of this test's choosing.
    fn table_of(captured: &str, rows: &[&str]) -> String {
        let header = captured
            .lines()
            .next()
            .expect("the captured table opens with a header line");
        let mut table = String::from(header);
        for row in rows {
            table.push('\n');
            table.push_str(row);
        }
        table.push('\n');
        table
    }

    #[test]
    fn a_listening_socket_on_the_port_is_in_use() {
        assert_eq!(
            parse_port_status(PROC_NET_TCP_SAMPLE, SSH_PORT),
            PortStatus::InUse
        );
    }

    #[test]
    fn a_match_is_found_after_rows_that_do_not_match() {
        assert_eq!(
            parse_port_status(PROC_NET_TCP_SAMPLE, CUPS_PORT),
            PortStatus::InUse
        );
    }

    #[test]
    fn a_listening_socket_on_another_port_leaves_the_port_free() {
        assert_eq!(
            parse_port_status(PROC_NET_TCP_SAMPLE, 9090),
            PortStatus::Free
        );
    }

    #[test]
    fn a_socket_that_never_listened_leaves_the_port_free() {
        // An established connection holds a local port without accepting on it,
        // which is not what `wl` was asked about.
        assert_eq!(
            parse_port_status(PROC_NET_TCP_SAMPLE, ESTABLISHED_PORT),
            PortStatus::Free
        );
    }

    #[test]
    fn an_ipv6_listening_socket_on_the_port_is_in_use() {
        assert_eq!(
            parse_port_status(PROC_NET_TCP6_SAMPLE, SSH_PORT),
            PortStatus::InUse
        );
        assert_eq!(
            parse_port_status(PROC_NET_TCP6_SAMPLE, CUPS_PORT),
            PortStatus::InUse
        );
    }

    #[test]
    fn an_ipv6_listening_socket_on_another_port_leaves_the_port_free() {
        assert_eq!(
            parse_port_status(PROC_NET_TCP6_SAMPLE, 9090),
            PortStatus::Free
        );
    }

    #[test]
    fn a_table_of_nothing_but_a_header_leaves_the_port_free() {
        assert_eq!(
            parse_port_status(&table_of(PROC_NET_TCP_SAMPLE, &[]), SSH_PORT),
            PortStatus::Free
        );
        assert_eq!(
            parse_port_status(&table_of(PROC_NET_TCP6_SAMPLE, &[]), SSH_PORT),
            PortStatus::Free
        );
    }

    #[test]
    fn a_malformed_row_is_unknown() {
        // Each row below is the shape the kernel prints, broken one way. None
        // of them may be read as a table that says the port is free.
        let rows = [
            // A state that is not hexadecimal.
            "   0: 00000000:0016 00000000:0000 ZZ 00000000:00000000 00:00000000 00000000     0        0 18716 1",
            // A local address carrying no port.
            "   0: 00000000 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 18716 1",
            // An address half of neither file's width.
            "   0: 0000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 18716 1",
            // An address half that is not hexadecimal.
            "   0: 000000ZZ:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 18716 1",
            // A port half of the wrong width.
            "   0: 00000000:16 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 18716 1",
            // Too few columns to hold a state at all.
            "   0: 00000000:0016 00000000:0000",
        ];

        for row in rows {
            let table = table_of(PROC_NET_TCP_SAMPLE, &[row]);
            assert_eq!(
                parse_port_status(&table, SSH_PORT),
                PortStatus::Unknown,
                "a table this module cannot read through is not a table that \
                 says the port is free. The row was: {row}"
            );
        }
    }

    #[test]
    fn a_malformed_row_after_the_match_is_still_unknown() {
        // The listening socket is read before the row that stops making sense.
        // A table that stops making sense may have been misread from its first
        // row, so the match is not proof either.
        let broken = "   9: nonsense";
        let table = table_of(PROC_NET_TCP_SAMPLE, &[
            "   0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 18716 1",
            broken,
        ]);

        assert_eq!(parse_port_status(&table, SSH_PORT), PortStatus::Unknown);
    }

    #[test]
    fn a_header_that_names_other_columns_is_unknown() {
        // A column inserted ahead of `local_address` moves every column this
        // module reads. The header says so before a single row is read.
        let table = concat!(
            "  sl  family local_address rem_address   st tx_queue rx_queue\n",
            "   0: 2 00000000:0016 00000000:0000 0A 00000000:00000000\n",
        );

        assert_eq!(parse_port_status(table, SSH_PORT), PortStatus::Unknown);
    }

    #[test]
    fn a_table_with_no_header_is_unknown() {
        assert_eq!(parse_port_status("", SSH_PORT), PortStatus::Unknown);
    }

    #[test]
    fn one_unreadable_table_makes_the_whole_answer_unknown() {
        assert_eq!(
            combine(PortStatus::Unknown, PortStatus::Free),
            PortStatus::Unknown
        );
        assert_eq!(
            combine(PortStatus::Free, PortStatus::Unknown),
            PortStatus::Unknown
        );
        assert_eq!(
            combine(PortStatus::Unknown, PortStatus::InUse),
            PortStatus::Unknown
        );
        assert_eq!(
            combine(PortStatus::InUse, PortStatus::Unknown),
            PortStatus::Unknown
        );
    }

    #[test]
    fn a_listener_in_either_table_holds_the_port() {
        assert_eq!(
            combine(PortStatus::InUse, PortStatus::Free),
            PortStatus::InUse
        );
        assert_eq!(
            combine(PortStatus::Free, PortStatus::InUse),
            PortStatus::InUse
        );
        assert_eq!(
            combine(PortStatus::InUse, PortStatus::InUse),
            PortStatus::InUse
        );
    }

    #[test]
    fn two_complete_readings_that_found_nothing_answer_free() {
        assert_eq!(
            combine(PortStatus::Free, PortStatus::Free),
            PortStatus::Free
        );
    }

    /// Pin this module's reading against the running kernel.
    ///
    /// The captured tables above are text this file carries, so they agree with
    /// a wrong column index as readily as with a right one. This one does not:
    /// a column read from the wrong place, or a port read as decimal, fails it.
    #[cfg(target_os = "linux")]
    #[test]
    fn reads_a_live_listening_socket_from_the_kernel() {
        // The kernel picks the port, so two copies of this test that run at
        // the same time never ask about the same one.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("should be able to bind an ephemeral port");
        let port = listener
            .local_addr()
            .expect("bound listener must have a local address")
            .port();

        assert_eq!(
            super::port_status(port),
            PortStatus::InUse,
            "this test is listening on port {port}, so the kernel's socket table \
             holds a listening socket for it"
        );

        drop(listener);

        assert_eq!(
            super::port_status(port),
            PortStatus::Free,
            "this test released port {port}, so the kernel's socket table holds \
             no listening socket for it"
        );
    }
}
