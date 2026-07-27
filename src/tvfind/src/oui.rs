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

    #[test]
    fn skips_longer_ma_m_assignments_that_are_not_oui_prefixes() {
        // 28-bit and 36-bit IEEE blocks appear with 7- or 9-character keys and
        // cannot be resolved from the first three octets of a MAC.
        let db = parse_db(OUI_DB);

        assert!(!db.contains_key("8C1F64233"));
        assert_eq!(db.len(), 3);
    }
}
