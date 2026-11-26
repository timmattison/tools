//! The patterns that ship with the app.

use serde::{Deserialize, Serialize};

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

/// An AWS key in one of two forms.
///
/// It matches an access key ID: a known four-letter prefix and 16 capital
/// letters or digits. It matches the value of a secret access key or a session
/// token that stands next to its key name. The key name, the quotes, and the
/// separator stay visible, because the group `redact` holds only the value.
/// The key name can have an underscore or not, and any case.
///
/// It does not match a secret value that has no key name next to it, for
/// example a bare 40-character string or a git SHA.
const AWS_KEY: &str = concat!(
    r"\b(?:AKIA|ASIA|ABIA|ACCA|AGPA|AIDA|AIPA|ANPA|ANVA|APKA|AROA|ASCA)[A-Z0-9]{16}\b",
    r#"|(?i:secret_?access_?key|session_?token)["']?\s*[:=]\s*["']?(?P<redact>[A-Za-z0-9/+=]{16,})"#,
);

/// An API token in one of several known formats.
///
/// It matches a GitHub token (`ghp_` and its kin, and `github_pat_`), a GitLab
/// token (`glpat-`), a Slack token (`xox`), an `sk-` key, a Stripe key
/// (`sk_live_`, `rk_test_`, and their kin), a Google API key (`AIza`), an npm
/// token (`npm_`), and a JSON web token with three parts. It also matches the
/// token after the word `Bearer`. Only that token is replaced, because the
/// group `redact` holds it. Only this branch has the group, because a group
/// name can occur once in a pattern.
///
/// It does not match a word that only looks like a prefix, for example
/// `risk-assessment-for-the-new-project-x`, or a token that is too short.
const TOKEN: &str = concat!(
    r"\bgh[pousr]_[A-Za-z0-9]{36,}\b",
    r"|\bgithub_pat_[A-Za-z0-9_]{22,}\b",
    r"|\bglpat-[A-Za-z0-9_-]{20,}",
    r"|\bxox[abposr]-[A-Za-z0-9-]{10,}",
    r"|\bsk-[A-Za-z0-9_-]{20,}",
    r"|\b[sr]k_(?:live|test)_[A-Za-z0-9]{16,}",
    r"|\bAIza[0-9A-Za-z_-]{35}",
    r"|\bnpm_[A-Za-z0-9]{36}\b",
    r"|\beyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
    r"|(?i:bearer)\s+(?P<redact>[A-Za-z0-9._~+/-]+=*)",
);

/// A hostname with at least two labels.
///
/// The last label must be in a fixed list: `com`, `net`, `org`, `io`, `dev`,
/// `cloud`, `edu`, `gov`, `mil`, `info`, `biz`, `us`, `uk`, `ca`, `de`, `fr`,
/// `jp`, `au`, `local`, `internal`, `lan`, `localdomain`, `corp`, and `arpa`.
/// The list has no case.
///
/// It does not match a name with one label, such as `localhost`. It does not
/// match a file name such as `main.rs` or `notes.txt`, because its last label
/// is not in the list.
const HOSTNAME: &str = concat!(
    r"\b(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+",
    r"(?i:com|net|org|io|dev|cloud|edu|gov|mil|info|biz|us|uk|ca|de|fr|jp|au",
    r"|local|internal|lan|localdomain|corp|arpa)\b",
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
            Preset::AwsKey => AWS_KEY,
            Preset::Token => TOKEN,
            Preset::Hostname => HOSTNAME,
        }
    }

    /// The name of the preset in a saved project and in a menu. It is the
    /// name that `serde` gives the preset.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Preset::Email => "email",
            Preset::IpAddress => "ip-address",
            Preset::AwsKey => "aws-key",
            Preset::Token => "token",
            Preset::Hostname => "hostname",
        }
    }

    /// The preset that has the name `key`, or `None` when no preset has it.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|preset| preset.key() == key)
    }
}
