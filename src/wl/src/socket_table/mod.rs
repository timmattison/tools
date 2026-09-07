//! Ask the operating system whether anything is listening on a TCP port.
//!
//! [`port_status`] is the one entrance. It answers the question `wl` falls back
//! on when it could name no owning process: is the port held by somebody this
//! user cannot see, is it genuinely idle, or can this run not tell?
//!
//! # Reading the answer beats provoking it
//!
//! The first version of this asked by *binding* the port, across four local
//! addresses, and reading the refusal. That answered a second question nobody
//! asked: for the length of the call, `wl` itself held the port it was asked
//! about. Users run `wl` in a loop while they wait for a server to come up, so
//! the tool could make the very bind it was watching for fail. It also could
//! not answer at all for a port below the privileged threshold, where the
//! kernel refuses the bind before the question is reached.
//!
//! macOS publishes its own list of listening sockets to any user, so
//! [`port_status`] reads that list instead. See [`macos`] for the layout, the
//! walk, and the checks that keep a wrong transcription loud.
//!
//! # The bind probe is a fallback, and it is going away
//!
//! Every platform without a reader still binds, in the `bind_probe` module
//! below — which is why that name is not a link here: it is compiled out on
//! macOS. It is a stopgap: a Linux reader lands next, and the bind probe goes
//! with it. Do not build anything new on it.

mod macos;

/// What this module was able to establish about a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortStatus {
    /// Some process holds the port. Whoever reported it — a listening socket
    /// in the kernel's table, or a bind that returned `EADDRINUSE` — that is
    /// proof, even though this module cannot identify the holder.
    InUse,
    /// Nothing holds the port. A complete reading of the kernel's socket table
    /// found no listener, or the bind probe bound the port itself, or the
    /// address it asked for does not exist on this host.
    Free,
    /// This module could not find out. Something stopped it before it reached
    /// the question, so a listener may be hiding behind the failure. Never the
    /// answer of a check that completed.
    Unknown,
}

/// Say whether anything is listening on `port` on this host.
///
/// On macOS this reads the kernel's list of TCP sockets, which needs no
/// privileges and holds no port. Everywhere else it still binds the port to
/// find out — see the module documentation.
pub fn port_status(port: u16) -> PortStatus {
    #[cfg(target_os = "macos")]
    {
        macos::port_status(port)
    }
    #[cfg(not(target_os = "macos"))]
    {
        bind_probe::tcp_port_probe(port)
    }
}

/// Ask about a port by trying to take it.
///
/// The fallback for every platform that has no socket-table reader yet. A
/// later change deletes this module; nothing new should call into it.
#[cfg(not(target_os = "macos"))]
mod bind_probe {
    use super::PortStatus;
    use std::io;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};

    /// Classify one bind attempt.
    ///
    /// `EADDRINUSE` is the only proof of a holder, and `EADDRNOTAVAIL` is proof
    /// of the opposite: the host does not have that address, so nothing can be
    /// listening on it. Every other refusal — `EACCES` above all, which is what
    /// an unprivileged process gets for a port below the platform's privileged
    /// threshold — tells us nothing about the port, only about us.
    fn classify_bind_error(kind: io::ErrorKind) -> PortStatus {
        match kind {
            io::ErrorKind::AddrInUse => PortStatus::InUse,
            io::ErrorKind::AddrNotAvailable => PortStatus::Free,
            _ => PortStatus::Unknown,
        }
    }

    /// Combine the answers from several candidate addresses.
    ///
    /// Precedence is `InUse` > `Unknown` > `Free`: one address that reports a
    /// holder settles the question, and short of that, one address the kernel
    /// refused keeps the whole answer uncertain — a holder could be sitting
    /// behind exactly that refusal.
    fn combine_probes(probes: impl IntoIterator<Item = PortStatus>) -> PortStatus {
        let mut result = PortStatus::Free;

        for probe in probes {
            match probe {
                PortStatus::InUse => return PortStatus::InUse,
                PortStatus::Unknown => result = PortStatus::Unknown,
                PortStatus::Free => {}
            }
        }

        result
    }

    /// Probe whether any process is listening on `port` locally by attempting
    /// to bind it ourselves, across the four common local addresses.
    ///
    /// The three answers mean:
    ///
    /// - [`PortStatus::InUse`] — a bind returned `EADDRINUSE`, so some process
    ///   holds the port, even though this probe can't say which.
    /// - [`PortStatus::Free`] — every candidate either bound successfully or
    ///   does not exist on this host, so nothing is listening on any of them.
    /// - [`PortStatus::Unknown`] — the kernel refused at least one bind and no
    ///   other candidate proved the port is held. A refusal answers a question
    ///   about the caller's privileges, not about the port, so a listener may
    ///   hide behind it.
    ///
    /// [`PortStatus::Unknown`] is the common case for an unprivileged process
    /// on a privileged port: Linux
    /// (`net.ipv4.ip_unprivileged_port_start`, 1024 by default) refuses the
    /// wildcard binds and the loopback binds alike for a port under the
    /// threshold. There is one further gap that no answer covers: a listener
    /// bound only to a specific non-loopback address, such as a LAN interface,
    /// is not among the candidates and may go undetected. Reading the kernel's
    /// socket table has none of these gaps, which is why this probe is on its
    /// way out.
    pub fn tcp_port_probe(port: u16) -> PortStatus {
        let candidates: [SocketAddr; 4] = [
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
            SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
            SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
        ];

        combine_probes(candidates.iter().map(|addr| match TcpListener::bind(addr) {
            Ok(_listener) => PortStatus::Free,
            Err(e) => classify_bind_error(e.kind()),
        }))
    }

    #[cfg(test)]
    mod tests {
        use super::{classify_bind_error, combine_probes, tcp_port_probe, PortStatus};
        use std::io;

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
                PortStatus::InUse,
                "port {port} is held by this test but the probe did not report it in use"
            );

            drop(listener);
        }

        #[test]
        fn reports_a_released_port_as_free() {
            // The kernel picks the port, so two copies of this test that run at
            // the same time never ask about the same one.
            let listener = std::net::TcpListener::bind("127.0.0.1:0")
                .expect("should be able to bind an ephemeral port");
            let port = listener
                .local_addr()
                .expect("bound listener must have a local address")
                .port();
            drop(listener);

            assert_eq!(
                tcp_port_probe(port),
                PortStatus::Free,
                "port {port} was released by this test but the probe did not report it free"
            );
        }

        #[test]
        fn classifies_every_bind_error_kind() {
            let cases = [
                (io::ErrorKind::AddrInUse, PortStatus::InUse),
                (io::ErrorKind::AddrNotAvailable, PortStatus::Free),
                (io::ErrorKind::PermissionDenied, PortStatus::Unknown),
                (io::ErrorKind::Other, PortStatus::Unknown),
                (io::ErrorKind::InvalidInput, PortStatus::Unknown),
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
                combine_probes([PortStatus::Free, PortStatus::Unknown, PortStatus::InUse]),
                PortStatus::InUse
            );
            assert_eq!(
                combine_probes([PortStatus::InUse, PortStatus::Unknown]),
                PortStatus::InUse
            );
        }

        #[test]
        fn one_refusal_makes_the_whole_answer_unknown() {
            assert_eq!(
                combine_probes([PortStatus::Free, PortStatus::Unknown, PortStatus::Free]),
                PortStatus::Unknown
            );
        }

        #[test]
        fn all_free_candidates_answer_free() {
            assert_eq!(
                combine_probes([PortStatus::Free, PortStatus::Free]),
                PortStatus::Free
            );
        }

        #[test]
        fn no_candidates_answer_free() {
            assert_eq!(combine_probes([]), PortStatus::Free);
        }
    }
}
