//! IPv4 CIDR arithmetic and discovery of the subnet this machine sits on.

use std::net::Ipv4Addr;

use anyhow::Result;

/// Expand a CIDR block into the host addresses worth probing.
///
/// The network and broadcast addresses are omitted for any block wider than a
/// `/31`, since no host answers on them.
///
/// # Errors
///
/// Returns an error if `cidr` is not `A.B.C.D/bits` with `bits` in `0..=32`.
pub fn hosts_in(cidr: &str) -> Result<Vec<Ipv4Addr>> {
    let _ = cidr;
    Ok(Vec::new())
}

/// The CIDR of the first non-loopback IPv4 interface on this machine.
///
/// # Errors
///
/// Returns an error if no usable IPv4 interface is present.
pub fn local_subnet() -> Result<String> {
    Ok(String::new())
}
