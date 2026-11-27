//! The one type this crate keeps a credential in.
//!
//! A credential held in a `String` is printed by every derived `Debug` that
//! reaches it, and `ufa` holds four such values on the way to a request: the
//! parsed arguments, the cloud subcommand, the configuration file, and the
//! credential the setup wizard gathers. None of them is formatted with `{:?}`
//! today, which is exactly why a hand-written `Debug` on each is the wrong
//! answer: the leak arrives later, as one `eprintln!` somebody adds while
//! debugging or as a panic message that carries the value, and four redactions
//! written by hand are four chances to forget the fifth.
//!
//! [`Secret`] moves the decision into the type. Its field is private, its
//! `Debug` says [`REDACTED`] and nothing else, and it has **no `Display`** — so
//! `println!("{key}")` does not print the key, it fails to compile. The one way
//! to the value is [`Secret::expose`], which is a word a reader of the call
//! site can see.

use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

/// What a debug dump says in place of the credential.
pub const REDACTED: &str = "<redacted>";

/// A credential, held so that printing it takes saying so.
///
/// It reads and writes as a plain string, so a configuration file written
/// before this type existed still loads, and one written after it is still the
/// `api_key = "..."` a user can read.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// The credential itself.
    ///
    /// Named for what it does. Every call is a place the value leaves the type
    /// that protects it, and there are only as many of them as there are
    /// requests to send.
    ///
    /// # Returns
    ///
    /// The credential, exactly as its source holds it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Prints [`REDACTED`], whatever the credential is.
///
/// This is the whole point of the type, and it is why the field above is
/// private: a derived `Debug` anywhere in the crate that reaches a credential
/// reaches this instead.
impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REDACTED)
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Reads a credential off the command line.
///
/// clap needs this to accept `--api-key`. Nothing a user can type is rejected:
/// a key that is blank names no key, and the crate says so where it resolves
/// credentials rather than here, so the wizard's advice reaches the user
/// instead of a parse error.
impl FromStr for Secret {
    type Err = Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(value))
    }
}

#[cfg(test)]
mod tests {
    use super::{Secret, REDACTED};
    use crate::test_support::FAKE_CONTROLLER_KEY;

    /// The type exists to answer one question, so it answers it first.
    #[test]
    fn a_debug_dump_says_the_marker_and_nothing_else() {
        let secret = Secret::from(FAKE_CONTROLLER_KEY);

        assert_eq!(format!("{secret:?}"), REDACTED);
    }

    /// A debug dump of a `Secret` nested inside another value is the case the
    /// whole design rests on: the four types that hold credentials derive
    /// their own `Debug`, and what they print for the field is this.
    #[test]
    fn a_nested_debug_dump_redacts_too() {
        let nested = Some(vec![Secret::from(FAKE_CONTROLLER_KEY)]);

        let dump = format!("{nested:?}");

        assert!(
            !dump.contains(FAKE_CONTROLLER_KEY),
            "a derived Debug must reach the redaction, got {dump}"
        );
        assert!(
            dump.contains(REDACTED),
            "a derived Debug must print the marker, got {dump}"
        );
    }

    /// The value still has to reach a request, and `expose` is the word that
    /// says where that happens.
    #[test]
    fn the_credential_survives_the_wrapping() {
        assert_eq!(
            Secret::from(FAKE_CONTROLLER_KEY.to_owned()).expose(),
            FAKE_CONTROLLER_KEY,
            "the credential must reach the caller byte for byte"
        );
    }

    /// The configuration file is TOML that a user reads and edits, and a file
    /// written before this type existed holds a plain string. The type has to
    /// read and write exactly that, or every saved configuration breaks.
    #[test]
    fn a_secret_reads_and_writes_as_a_plain_string() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Holder {
            api_key: Secret,
        }

        let written = toml::to_string_pretty(&Holder {
            api_key: Secret::from(FAKE_CONTROLLER_KEY),
        })
        .expect("a secret must serialize");

        assert_eq!(
            written.trim(),
            format!("api_key = \"{FAKE_CONTROLLER_KEY}\""),
            "the file must stay the plain TOML a user can read"
        );

        let read: Holder = toml::from_str(&written).expect("a secret must deserialize");
        assert_eq!(
            read.api_key.expose(),
            FAKE_CONTROLLER_KEY,
            "what was written must load again"
        );
    }
}
