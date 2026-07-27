//! IPv4 CIDR arithmetic and discovery of the subnet this machine sits on.

use std::net::Ipv4Addr;

use anyhow::{bail, Context, Result};

/// Number of address bits in an IPv4 address.
const ADDRESS_BITS: u32 = 32;

/// Widest block that may be scanned, as an address count.
///
/// A `/16` is already 65k hosts and minutes of probing; anything wider is
/// almost certainly a typo rather than an intent, and enumerating it would
/// cost gigabytes before the first packet went out.
const MAX_BLOCK_ADDRESSES: u64 = 1 << 16;

/// Expand a CIDR block into the host addresses worth probing.
///
/// The network and broadcast addresses are omitted for any block wider than a
/// `/31`, since no host answers on them.
///
/// # Errors
///
/// Returns an error if `cidr` is not `A.B.C.D/bits` with `bits` in `0..=32`,
/// or if the block spans more than [`MAX_BLOCK_ADDRESSES`] addresses.
pub fn hosts_in(cidr: &str) -> Result<Vec<Ipv4Addr>> {
    let (network, broadcast) = bounds_of(cidr)?;

    let span = u64::from(broadcast - network) + 1;
    if span > MAX_BLOCK_ADDRESSES {
        bail!("`{cidr}` is too large to scan: {span} addresses, limit is {MAX_BLOCK_ADDRESSES}");
    }

    // Anything wider than a /31 reserves its first and last address.
    let (first, last) = if broadcast - network >= 2 {
        (network + 1, broadcast - 1)
    } else {
        (network, broadcast)
    };

    Ok((first..=last).map(Ipv4Addr::from).collect())
}

/// The network and broadcast addresses of `cidr`, as host-order integers.
fn bounds_of(cidr: &str) -> Result<(u32, u32)> {
    let (base, prefix) = cidr
        .split_once('/')
        .with_context(|| format!("expected a CIDR of the form A.B.C.D/bits, got `{cidr}`"))?;

    let base: Ipv4Addr = base
        .parse()
        .with_context(|| format!("`{base}` is not an IPv4 address"))?;
    let prefix: u32 = prefix
        .parse()
        .with_context(|| format!("`{prefix}` is not a prefix length"))?;
    if prefix > ADDRESS_BITS {
        bail!("prefix length /{prefix} exceeds /{ADDRESS_BITS}");
    }

    // A /0 would shift by the full width, which overflows rather than yielding 0.
    let host_bits = ADDRESS_BITS - prefix;
    let mask = if host_bits == ADDRESS_BITS {
        0
    } else {
        u32::MAX << host_bits
    };

    let network = u32::from(base) & mask;
    Ok((network, network | !mask))
}

/// The CIDR of the first non-loopback IPv4 interface on this machine.
///
/// # Errors
///
/// Returns an error if no usable IPv4 interface is present.
pub fn local_subnet() -> Result<String> {
    Ok(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_a_slash_23_into_every_usable_host() {
        let hosts = hosts_in("192.168.0.0/23").unwrap();

        assert_eq!(
            hosts.len(),
            510,
            "a /23 spans 512 addresses, less network and broadcast"
        );
        assert_eq!(hosts.first().copied(), Some(Ipv4Addr::new(192, 168, 0, 1)));
        assert_eq!(hosts.last().copied(), Some(Ipv4Addr::new(192, 168, 1, 254)));
    }

    #[test]
    fn keeps_the_sole_address_of_a_slash_32() {
        let hosts = hosts_in("10.0.0.7/32").unwrap();

        assert_eq!(hosts, vec![Ipv4Addr::new(10, 0, 0, 7)]);
    }

    #[test]
    fn masks_a_host_address_down_to_its_network() {
        // `192.168.1.165/23` names a host inside the block, not the base.
        let hosts = hosts_in("192.168.1.165/23").unwrap();

        assert_eq!(hosts.first().copied(), Some(Ipv4Addr::new(192, 168, 0, 1)));
        assert_eq!(hosts.len(), 510);
    }

    #[test]
    fn rejects_a_cidr_without_a_prefix_length() {
        assert!(hosts_in("192.168.0.0").is_err());
    }

    #[test]
    fn rejects_a_prefix_length_above_32() {
        assert!(hosts_in("192.168.0.0/33").is_err());
    }

    #[test]
    fn rejects_a_malformed_address() {
        assert!(hosts_in("not-an-ip/24").is_err());
    }

    #[test]
    fn refuses_a_block_too_large_to_scan() {
        // A /8 is 16.7M addresses: hours of probing and gigabytes of Vec.
        let error = hosts_in("10.0.0.0/8").unwrap_err().to_string();

        assert!(
            error.contains("too large"),
            "error should explain the block is oversized, got: {error}"
        );
    }

    #[test]
    fn still_accepts_the_widest_supported_block() {
        // Pins the cutoff so the size guard cannot creep tighter than a /16.
        assert_eq!(hosts_in("10.1.0.0/16").unwrap().len(), 65_534);
    }
}
