//! The patterns that ship with the app.

use serde::{Deserialize, Serialize};

/// A pattern that matches nothing. It is a stand-in for a pattern that is not
/// written yet.
const NOTHING: &str = r"[^\s\S]";

/// An email address: a local part, `@`, and a domain with a dot.
///
/// It does not match `user@localhost`, because the domain has no dot. It does
/// not match an address with a space around the `@`.
const EMAIL: &str = r"[A-Za-z0-9._%+-]+@(?:[A-Za-z0-9-]+\.)+[A-Za-z]{2,}";

/// An IP address in one of three forms.
///
/// It matches an IPv4 address with four numbers from 0 to 255. It matches a
/// full IPv6 address with eight groups. It matches a compressed IPv6 address
/// that has `::` with at least one group on each side.
///
/// It does not match `::1`, because `::` has no group on its left. It does not
/// match a clock time, a MAC address, a version number, or a Rust path such as
/// `std::fs::read`.
const IP_ADDRESS: &str = concat!(
    r"\b(?:25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])",
    r"(?:\.(?:25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])){3}\b",
    r"|\b(?:[0-9A-Fa-f]{1,4}:){7}[0-9A-Fa-f]{1,4}\b",
    r"|\b(?:[0-9A-Fa-f]{1,4}:){1,6}(?::[0-9A-Fa-f]{1,4}){1,6}\b",
);

/// A pattern that ships with the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    /// Email addresses.
    Email,
    /// IPv4 and IPv6 addresses.
    IpAddress,
    /// AWS access key IDs, secret keys, and session tokens.
    AwsKey,
    /// API tokens, web tokens, and bearer tokens.
    Token,
    /// Hostnames.
    Hostname,
}

impl Preset {
    /// Each preset, in the order that a menu shows them.
    pub const ALL: [Preset; 5] = [
        Preset::Email,
        Preset::IpAddress,
        Preset::AwsKey,
        Preset::Token,
        Preset::Hostname,
    ];

    /// The name of the preset for a person.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Preset::Email => "Email addresses",
            Preset::IpAddress => "IP addresses",
            Preset::AwsKey => "AWS keys",
            Preset::Token => "Tokens",
            Preset::Hostname => "Hostnames",
        }
    }

    /// The regular expression of the preset.
    #[must_use]
    pub fn pattern(self) -> &'static str {
        match self {
            Preset::Email => EMAIL,
            Preset::IpAddress => IP_ADDRESS,
            Preset::AwsKey | Preset::Token | Preset::Hostname => NOTHING,
        }
    }
}
