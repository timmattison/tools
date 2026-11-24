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
//! A platform with no reader here gets `None` from [`port_status`]. `wl` then
//! says that it reads the list on macOS and Linux only. It does not say that a
//! read failed, because it did not try one.
//!
//! `wl` does not take [`PortStatus::Free`] from `listeners` on Windows.
//! `listeners` 0.2.1 turns a failed `GetExtendedTcpTable` read into an empty
//! list. It also drops a row whose process the Toolhelp snapshot cannot name.
//! So its silence does not prove a port free.
//!
//! A reader of `GetExtendedTcpTable` (`iphlpapi.h`) here closes the gap.
//! Nobody has written it.

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
    /// This module could not find out. A reading stopped short of the end, so
    /// a listener can hide behind the failure. Never the answer of a reading
    /// that completed.
    Unknown,
}

/// The reader of the kernel's socket table on this platform, if there is one.
#[cfg(target_os = "macos")]
const READER: Option<fn(u16) -> PortStatus> = Some(macos::port_status);
#[cfg(target_os = "linux")]
const READER: Option<fn(u16) -> PortStatus> = Some(linux::port_status);
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const READER: Option<fn(u16) -> PortStatus> = None;

/// Say whether anything is listening on `port` on this host.
///
/// On macOS and Linux this reads the kernel's list of TCP sockets, which needs
/// no privileges and holds no port. Every other platform has no reader and
/// gets `None`.
pub fn port_status(port: u16) -> Option<PortStatus> {
    READER.map(|read| read(port))
}
