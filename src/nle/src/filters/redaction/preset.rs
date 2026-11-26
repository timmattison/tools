//! The patterns that ship with the app.

use serde::{Deserialize, Serialize};

/// A pattern that matches nothing. It is a stand-in for a pattern that is not
/// written yet.
const NOTHING: &str = r"[^\s\S]";

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
        NOTHING
    }
}
