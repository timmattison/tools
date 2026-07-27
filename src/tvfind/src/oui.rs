//! MAC address vendor lookup, used to spot televisions that are powered off.
//!
//! A TV in standby refuses every TCP connection but still answers ARP, so it
//! is invisible to a port scan yet plainly present in the neighbour table.
//! Resolving its MAC prefix against nmap's OUI database recovers it.

use std::collections::HashMap;
use std::net::Ipv4Addr;

/// Locations nmap's OUI database may live, newest Homebrew layout first.
pub const OUI_DB_PATHS: &[&str] = &[
    "/opt/homebrew/share/nmap/nmap-mac-prefixes",
    "/usr/local/share/nmap/nmap-mac-prefixes",
    "/usr/share/nmap/nmap-mac-prefixes",
];

/// An ARP neighbour that did not answer any probe.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Neighbour {
    /// Address held in the neighbour table.
    pub ip: Ipv4Addr,
    /// Hardware address as printed by `arp`.
    pub mac: String,
}

/// Normalise a MAC address to its 6-hex-digit uppercase OUI prefix.
///
/// macOS `arp` prints octets without leading zeros (`0:f:e7:83:b8:eb`), so
/// each octet is padded before the prefix is assembled.
///
/// Returns `None` if `mac` does not have at least three hex octets.
#[must_use]
pub fn mac_prefix(mac: &str) -> Option<String> {
    let _ = mac;
    None
}

/// Parse nmap's `nmap-mac-prefixes` into a prefix-to-vendor map.
#[must_use]
pub fn parse_db(text: &str) -> HashMap<String, String> {
    let _ = text;
    HashMap::new()
}

/// Parse the output of `arp -a -n` into neighbours.
#[must_use]
pub fn parse_arp_table(text: &str) -> Vec<Neighbour> {
    let _ = text;
    Vec::new()
}
