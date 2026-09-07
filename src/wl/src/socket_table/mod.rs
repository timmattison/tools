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
//! the tool could make the very bind it was watching for fail. It could not
//! answer at all for a port below the privileged threshold, where the kernel
//! refuses the bind before the question is reached, and it could not see a
//! listener bound to an address that was not one of its four candidates.
//!
//! macOS and Linux each publish their own list of listening sockets to any
//! user, and [`port_status`] reads that list. See [`macos`] for the layout of
//! the kernel's records, the walk, and the checks that keep a wrong
//! transcription loud, and [`linux`] for the two `/proc` tables and the checks
//! that keep a wrong reading loud. Both fail closed the same way: a reading
//! that did not complete answers [`PortStatus::Unknown`], never
//! [`PortStatus::Free`].
//!
//! # Every other platform says it cannot tell
//!
//! A platform with no reader here answers [`PortStatus::Unknown`] for every
//! port, because nothing asked. Windows is the one that loses an answer by it.
//! `listeners::get_all` reads the complete Windows socket table already, so a
//! port it named no process for really was free, and `wl` now declines to say
//! so instead. Closing that gap means reading `GetExtendedTcpTable`
//! (`iphlpapi.h`) here — the same list `netstat` reads — and nobody has written
//! that yet.

mod linux;
mod macos;

/// What this module was able to establish about a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortStatus {
    /// Some process holds the port. The kernel's own list of sockets carries a
    /// listening socket on it, which is proof even though this module cannot
    /// identify the holder.
    InUse,
    /// Nothing holds the port. A complete reading of the kernel's socket table
    /// found no listener on it.
    Free,
    /// This module could not find out. Either this platform has no reader here,
    /// or a reading stopped short of the end, so a listener may be hiding
    /// behind the failure. Never the answer of a reading that completed.
    Unknown,
}

/// Say whether anything is listening on `port` on this host.
///
/// On macOS and Linux this reads the kernel's list of TCP sockets, which needs
/// no privileges and holds no port. Every other platform answers
/// [`PortStatus::Unknown`] — see the module documentation.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "linux")),
    allow(
        unused_variables,
        reason = "a platform with no socket-table reader answers Unknown without \
                  ever looking at the port"
    )
)]
pub fn port_status(port: u16) -> PortStatus {
    #[cfg(target_os = "macos")]
    {
        macos::port_status(port)
    }
    #[cfg(target_os = "linux")]
    {
        linux::port_status(port)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        PortStatus::Unknown
    }
}
