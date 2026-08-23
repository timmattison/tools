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

/// Render the CIDR that `address` sits in, given its `netmask`.
#[must_use]
pub fn subnet_from(address: Ipv4Addr, netmask: Ipv4Addr) -> String {
    let mask = u32::from(netmask);
    let network = Ipv4Addr::from(u32::from(address) & mask);

    format!("{network}/{}", mask.count_ones())
}

/// The CIDR to scan, chosen from the interfaces the operating system reports.
///
/// `interfaces` arrives in the order the operating system gives, and that order
/// is not a ranking. `tvfind` takes the first IPv4 interface that is not
/// loopback and whose netmask leaves room for a neighbour. It skips an
/// interface that holds one or two addresses, because a VPN presents such an
/// interface and a scan of it reaches nothing.
///
/// Returns `None` if no interface qualifies.
#[must_use]
pub fn subnet_of(interfaces: &[get_if_addrs::Interface]) -> Option<String> {
    let _ = interfaces;
    todo!("the subnet is not yet chosen by the room an interface leaves for a neighbour")
}

/// Why the interfaces of this machine gave no subnet to scan.
///
/// The message names each interface that was skipped for a single-address
/// netmask. That is the case a user cannot see from the outside.
fn no_subnet_reason(interfaces: &[get_if_addrs::Interface]) -> String {
    let _ = interfaces;
    todo!("the error does not yet report an interface that was skipped")
}

/// The CIDR of the first non-loopback IPv4 interface on this machine.
///
/// # Errors
///
/// Returns an error if no usable IPv4 interface is present.
pub fn local_subnet() -> Result<String> {
    let interfaces = get_if_addrs::get_if_addrs().context("could not enumerate interfaces")?;

    interfaces
        .iter()
        .filter(|interface| !interface.is_loopback())
        .find_map(|interface| match interface.addr {
            get_if_addrs::IfAddr::V4(ref v4) => Some(subnet_from(v4.ip, v4.netmask)),
            get_if_addrs::IfAddr::V6(_) => None,
        })
        .context("no non-loopback IPv4 interface found; pass --subnet explicitly")
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

    #[test]
    fn renders_a_non_octet_aligned_netmask_as_its_prefix_length() {
        // 255.255.254.0 is the /23 this machine's own network uses.
        assert_eq!(
            subnet_from(
                Ipv4Addr::new(192, 168, 0, 128),
                Ipv4Addr::new(255, 255, 254, 0)
            ),
            "192.168.0.0/23"
        );
    }

    #[test]
    fn renders_a_plain_class_c_netmask() {
        assert_eq!(
            subnet_from(Ipv4Addr::new(10, 0, 0, 5), Ipv4Addr::new(255, 255, 255, 0)),
            "10.0.0.0/24"
        );
    }

    #[test]
    fn masks_the_address_down_when_rendering() {
        assert_eq!(
            subnet_from(Ipv4Addr::new(172, 16, 5, 9), Ipv4Addr::new(255, 240, 0, 0)),
            "172.16.0.0/12"
        );
    }

    #[test]
    fn reports_a_local_subnet_that_can_actually_be_scanned() {
        let Ok(cidr) = local_subnet() else {
            // No usable IPv4 interface, e.g. an isolated build sandbox.
            return;
        };

        assert!(
            hosts_in(&cidr).is_ok(),
            "local_subnet produced an unscannable CIDR: {cidr}"
        );
    }

    /// One IPv4 interface, in the shape `get_if_addrs` reports.
    fn ipv4_interface(name: &str, ip: [u8; 4], netmask: [u8; 4]) -> get_if_addrs::Interface {
        get_if_addrs::Interface {
            name: name.to_owned(),
            addr: get_if_addrs::IfAddr::V4(get_if_addrs::Ifv4Addr {
                ip: Ipv4Addr::from(ip),
                netmask: Ipv4Addr::from(netmask),
                broadcast: None,
            }),
        }
    }

    /// The wired LAN of this machine, a `/23`.
    fn en0() -> get_if_addrs::Interface {
        ipv4_interface("en0", [192, 168, 0, 128], [255, 255, 254, 0])
    }

    /// A second adapter on the same LAN.
    fn en1() -> get_if_addrs::Interface {
        ipv4_interface("en1", [192, 168, 1, 131], [255, 255, 254, 0])
    }

    /// A Tailscale interface. Its netmask holds one address: itself.
    fn utun2() -> get_if_addrs::Interface {
        ipv4_interface("utun2", [100, 122, 91, 17], [255, 255, 255, 255])
    }

    #[test]
    fn takes_the_lan_when_a_vpn_interface_follows_it() {
        // The order `get_if_addrs` gave on this machine.
        assert_eq!(
            subnet_of(&[en0(), utun2(), en1()]).as_deref(),
            Some("192.168.0.0/23")
        );
    }

    #[test]
    fn takes_the_lan_when_a_vpn_interface_comes_first() {
        // The operating system owns the enumeration order and can give this one
        // instead. It made the scan report `100.122.91.17/32 (1 host)`.
        assert_eq!(
            subnet_of(&[utun2(), en0(), en1()]).as_deref(),
            Some("192.168.0.0/23")
        );
    }

    #[test]
    fn skips_a_point_to_point_slash_31() {
        // A /31 holds this machine and the far end of a link. A television is
        // not the far end of a point-to-point link.
        let ppp0 = ipv4_interface("ppp0", [10, 8, 0, 1], [255, 255, 255, 254]);

        assert_eq!(subnet_of(&[ppp0, en0()]).as_deref(), Some("192.168.0.0/23"));
    }

    #[test]
    fn skips_the_loopback_interface() {
        let lo0 = ipv4_interface("lo0", [127, 0, 0, 1], [255, 0, 0, 0]);

        assert_eq!(subnet_of(&[lo0, en0()]).as_deref(), Some("192.168.0.0/23"));
    }

    #[test]
    fn reports_no_subnet_when_every_interface_holds_a_single_address() {
        let utun4 = ipv4_interface("utun4", [10, 3, 5, 9], [255, 255, 255, 255]);

        assert_eq!(subnet_of(&[utun2(), utun4]), None);
    }

    #[test]
    fn tells_the_user_to_pass_subnet_when_no_interface_qualifies() {
        let reason = no_subnet_reason(&[utun2()]);

        assert!(
            reason.contains("--subnet"),
            "the error must name the flag that gets past it, got: {reason}"
        );
    }

    #[test]
    fn names_the_single_address_interface_it_skipped() {
        let reason = no_subnet_reason(&[utun2()]);

        assert!(
            reason.contains("utun2"),
            "the error must name the interface it skipped, got: {reason}"
        );
    }

    #[test]
    fn separates_a_skipped_interface_from_no_interface_at_all() {
        assert_ne!(
            no_subnet_reason(&[]),
            no_subnet_reason(&[utun2()]),
            "an interface that was skipped must change what the user is told"
        );
    }
}
