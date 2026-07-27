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
}
