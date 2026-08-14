//! MAC address vendor lookup, used to spot televisions that are powered off.
//!
//! A TV in standby refuses every TCP connection but still answers ARP, so it
//! is invisible to a port scan yet plainly present in the neighbour table.
//! Resolving its MAC prefix against nmap's OUI database recovers it.

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;

/// Octets of a MAC address that make up the vendor prefix.
const OUI_OCTETS: usize = 3;
/// Hex digits in a normalised OUI prefix.
const OUI_PREFIX_LEN: usize = OUI_OCTETS * 2;

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

/// A neighbour whose MAC belongs to the vendor being looked for, but which
/// answered no probe — almost always a set that is powered off.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Candidate {
    /// Address held in the neighbour table.
    pub ip: Ipv4Addr,
    /// Hardware address as printed by `arp`.
    pub mac: String,
    /// Vendor the MAC prefix is registered to.
    pub vendor: String,
}

/// Neighbours matching `vendor_filter` by MAC that no probe identified.
///
/// `identified` holds the addresses already confirmed as televisions, so a TV
/// that answered is never also reported as a silent candidate.
#[must_use]
pub fn unresponsive_candidates(
    arp_output: &str,
    db: &HashMap<String, String>,
    identified: &HashSet<Ipv4Addr>,
    vendor_filter: &str,
) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = parse_arp_table(arp_output)
        .into_iter()
        .filter(|neighbour| !identified.contains(&neighbour.ip))
        .filter_map(|neighbour| {
            let vendor = db.get(&mac_prefix(&neighbour.mac)?)?;
            crate::vendor::matches(vendor, vendor_filter).then(|| Candidate {
                ip: neighbour.ip,
                mac: neighbour.mac,
                vendor: vendor.clone(),
            })
        })
        .collect();

    candidates.sort_by_key(|candidate| candidate.ip);
    candidates
}

/// Normalise a MAC address to its 6-hex-digit uppercase OUI prefix.
///
/// macOS `arp` prints octets without leading zeros (`0:f:e7:83:b8:eb`), so
/// each octet is padded before the prefix is assembled.
///
/// Returns `None` if `mac` does not have at least three hex octets.
#[must_use]
pub fn mac_prefix(mac: &str) -> Option<String> {
    let mut prefix = String::with_capacity(OUI_PREFIX_LEN);

    for octet in mac.split(':').take(OUI_OCTETS) {
        if octet.is_empty() || octet.len() > 2 || !octet.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        // `arp` drops leading zeros, so 'f' has to become "0F".
        for _ in octet.len()..2 {
            prefix.push('0');
        }
        prefix.push_str(&octet.to_ascii_uppercase());
    }

    (prefix.len() == OUI_PREFIX_LEN).then_some(prefix)
}

/// Parse nmap's `nmap-mac-prefixes` into a prefix-to-vendor map.
#[must_use]
pub fn parse_db(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| line.trim().split_once(char::is_whitespace))
        .filter(|(prefix, _)| {
            // Longer MA-M/MA-S assignments cannot be resolved from three octets.
            prefix.len() == OUI_PREFIX_LEN && prefix.chars().all(|c| c.is_ascii_hexdigit())
        })
        .map(|(prefix, vendor)| (prefix.to_ascii_uppercase(), vendor.trim().to_owned()))
        .collect()
}

/// Parse the output of `arp -a -n` into neighbours, one for each address.
///
/// macOS prints a separate line for every interface that reaches a host, so a
/// machine on three networks lists each neighbour three times. Keying on the
/// address collapses those repeats while keeping two addresses that share one
/// hardware address, which is how a router with several addresses appears.
#[must_use]
pub fn parse_arp_table(text: &str) -> Vec<Neighbour> {
    let mut seen = HashSet::new();

    text.lines()
        .filter_map(|line| {
            // `? (192.168.0.1) at 70:a7:41:66:7c:39 on en0 ifscope [ethernet]`
            let (_, rest) = line.split_once('(')?;
            let (address, rest) = rest.split_once(')')?;
            let ip: Ipv4Addr = address.parse().ok()?;

            let (_, rest) = rest.split_once(" at ")?;
            let mac = rest.split_whitespace().next()?;
            // Rejects placeholders such as `(incomplete)`.
            mac_prefix(mac)?;

            // Group addresses set the low bit of the first octet. Nothing sits
            // behind broadcast or multicast, so probing them is wasted work.
            let first_octet = u8::from_str_radix(mac.split(':').next()?, 16).ok()?;
            if first_octet & 1 == 1 {
                return None;
            }

            if !seen.insert(ip) {
                return None;
            }

            Some(Neighbour {
                ip,
                mac: mac.to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_first_three_octets_as_the_prefix() {
        assert_eq!(mac_prefix("34:51:80:d0:83:d2").as_deref(), Some("345180"));
    }

    #[test]
    fn pads_octets_that_arp_printed_without_leading_zeros() {
        // macOS `arp` abbreviates: 0:f:e7 is really 00:0f:e7.
        assert_eq!(mac_prefix("0:f:e7:83:b8:eb").as_deref(), Some("000FE7"));
    }

    #[test]
    fn rejects_an_address_with_too_few_octets() {
        assert!(mac_prefix("34:51").is_none());
    }

    #[test]
    fn rejects_an_address_that_is_not_hexadecimal() {
        assert!(mac_prefix("(incomplete)").is_none());
        assert!(mac_prefix("zz:yy:xx:00:00:00").is_none());
    }

    /// Representative lines from nmap's `nmap-mac-prefixes`.
    const OUI_DB: &str = "# Auto-generated from the IEEE registry
345180 TCL King Electrical Appliances (Huizhou)
2CD974 Hui Zhou Gaoshengda Technology
5CAAFD Sonos

8C1F64233 TCL Operations Polska SP. Z O.O.
";

    #[test]
    fn maps_each_prefix_to_its_registered_vendor() {
        let db = parse_db(OUI_DB);

        assert_eq!(
            db.get("345180").map(String::as_str),
            Some("TCL King Electrical Appliances (Huizhou)")
        );
        assert_eq!(
            db.get("2CD974").map(String::as_str),
            Some("Hui Zhou Gaoshengda Technology")
        );
    }

    #[test]
    fn skips_comments_and_blank_lines() {
        let db = parse_db(OUI_DB);

        assert!(!db.keys().any(|prefix| prefix.starts_with('#')));
        assert!(!db.contains_key(""));
    }

    /// Representative `arp -a -n` output from macOS.
    const ARP_TABLE: &str = "? (192.168.0.1) at 70:a7:41:66:7c:39 on en0 ifscope [ethernet]
? (192.168.1.50) at 0:f:e7:83:b8:eb on en0 ifscope [ethernet]
? (192.168.0.43) at (incomplete) on en0 [ethernet]
? (192.168.0.0) at ff:ff:ff:ff:ff:ff on en0 ifscope [ethernet]
? (224.0.0.251) at 1:0:5e:0:0:fb on en0 ifscope permanent [ethernet]
? (192.168.1.217) at d0:65:b3:a8:60:33 on en0 ifscope [ethernet]
";

    #[test]
    fn reads_the_address_and_hardware_address_of_each_neighbour() {
        let neighbours = parse_arp_table(ARP_TABLE);

        assert_eq!(
            neighbours.first(),
            Some(&Neighbour {
                ip: Ipv4Addr::new(192, 168, 0, 1),
                mac: "70:a7:41:66:7c:39".to_owned(),
            })
        );
    }

    #[test]
    fn skips_entries_whose_resolution_never_completed() {
        let neighbours = parse_arp_table(ARP_TABLE);

        assert!(!neighbours
            .iter()
            .any(|n| n.ip == Ipv4Addr::new(192, 168, 0, 43)));
    }

    #[test]
    fn skips_broadcast_and_multicast_entries() {
        let neighbours = parse_arp_table(ARP_TABLE);

        // No host lives behind a group address, so probing one is wasted work.
        assert!(!neighbours.iter().any(|n| n.mac.starts_with("ff:ff")));
        assert!(!neighbours.iter().any(|n| n.mac.starts_with("1:0:5e")));
    }

    #[test]
    fn keeps_every_genuine_unicast_neighbour() {
        let neighbours = parse_arp_table(ARP_TABLE);

        assert_eq!(neighbours.len(), 3);
    }

    /// One host reachable over three interfaces, as macOS `arp` prints it.
    const MULTI_INTERFACE_ARP: &str =
        "? (192.168.0.1) at 70:a7:41:66:7c:39 on en0 ifscope [ethernet]
? (192.168.0.1) at 70:a7:41:66:7c:39 on en8 ifscope [ethernet]
? (192.168.0.1) at 70:a7:41:66:7c:39 on en1 ifscope [ethernet]
? (192.168.0.2) at 70:a7:41:66:7c:39 on en8 ifscope [ethernet]
";

    #[test]
    fn reports_a_host_once_however_many_interfaces_reach_it() {
        let neighbours = parse_arp_table(MULTI_INTERFACE_ARP);

        let first = neighbours
            .iter()
            .filter(|n| n.ip == Ipv4Addr::new(192, 168, 0, 1))
            .count();
        assert_eq!(first, 1, "one address must yield one neighbour");
    }

    #[test]
    fn keeps_two_addresses_that_share_one_hardware_address() {
        // A router answers for several addresses off one interface. Each is a
        // separate neighbour, so deduplication must key on the address.
        let neighbours = parse_arp_table(MULTI_INTERFACE_ARP);

        assert_eq!(neighbours.len(), 2);
    }

    /// The OUI database entries the candidate fixture depends on.
    fn candidate_db() -> HashMap<String, String> {
        parse_db(
            "D065B3 TCL King Electrical Appliances(Huizhou)Co.
2CD974 Hui Zhou Gaoshengda Technology
70A741 Ubiquiti Inc
5CAAFD Sonos
",
        )
    }

    const CANDIDATE_ARP: &str = "? (192.168.0.1) at 70:a7:41:66:7c:39 on en0 ifscope [ethernet]
? (192.168.0.46) at 5c:aa:fd:59:b5:f6 on en0 ifscope [ethernet]
? (192.168.0.248) at 2c:d9:74:11:bf:36 on en0 ifscope [ethernet]
? (192.168.1.217) at d0:65:b3:a8:60:33 on en0 ifscope [ethernet]
";

    #[test]
    fn reports_a_neighbour_whose_mac_belongs_to_the_wanted_vendor() {
        let found = unresponsive_candidates(CANDIDATE_ARP, &candidate_db(), &HashSet::new(), "tcl");

        assert!(found
            .iter()
            .any(|c| c.ip == Ipv4Addr::new(192, 168, 1, 217)));
    }

    #[test]
    fn reports_a_neighbour_registered_to_the_contract_manufacturer() {
        let found = unresponsive_candidates(CANDIDATE_ARP, &candidate_db(), &HashSet::new(), "tcl");

        assert!(found
            .iter()
            .any(|c| c.ip == Ipv4Addr::new(192, 168, 0, 248)));
    }

    #[test]
    fn omits_a_neighbour_already_identified_as_a_television() {
        let identified = HashSet::from([Ipv4Addr::new(192, 168, 0, 248)]);

        let found = unresponsive_candidates(CANDIDATE_ARP, &candidate_db(), &identified, "tcl");

        assert!(!found
            .iter()
            .any(|c| c.ip == Ipv4Addr::new(192, 168, 0, 248)));
    }

    #[test]
    fn omits_neighbours_belonging_to_other_vendors() {
        let found = unresponsive_candidates(CANDIDATE_ARP, &candidate_db(), &HashSet::new(), "tcl");

        assert_eq!(found.len(), 2, "only the two TCL-family MACs should remain");
    }

    #[test]
    fn reports_only_television_brands_when_no_vendor_filter_was_given() {
        let found = unresponsive_candidates(CANDIDATE_ARP, &candidate_db(), &HashSet::new(), "");

        assert!(
            !found.iter().any(|c| c.vendor.contains("Ubiquiti")),
            "a router is not a television"
        );
        assert!(
            !found.iter().any(|c| c.vendor.contains("Sonos")),
            "a speaker is not a television"
        );
    }

    #[test]
    fn still_reports_a_television_brand_when_no_vendor_filter_was_given() {
        let found = unresponsive_candidates(CANDIDATE_ARP, &candidate_db(), &HashSet::new(), "");

        assert_eq!(found.len(), 2, "both TCL-family MACs must survive");
    }

    #[test]
    fn names_the_vendor_the_prefix_is_registered_to() {
        let found = unresponsive_candidates(CANDIDATE_ARP, &candidate_db(), &HashSet::new(), "tcl");
        let tcl_king = found
            .iter()
            .find(|c| c.ip == Ipv4Addr::new(192, 168, 1, 217))
            .expect("the TCL King neighbour should be reported");

        assert_eq!(
            tcl_king.vendor,
            "TCL King Electrical Appliances(Huizhou)Co."
        );
    }

    #[test]
    fn skips_longer_ma_m_assignments_that_are_not_oui_prefixes() {
        // 28-bit and 36-bit IEEE blocks appear with 7- or 9-character keys and
        // cannot be resolved from the first three octets of a MAC.
        let db = parse_db(OUI_DB);

        assert!(!db.contains_key("8C1F64233"));
        assert_eq!(db.len(), 3);
    }
}
