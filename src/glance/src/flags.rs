//! The long names of the flags that a background start gives to its copy.
//!
//! The command line of `glance` and the start of the copy both take these
//! names from here, so the two cannot drift apart. A test of the binary
//! parses the arguments of the copy back through the command line.

/// The time between two samples.
pub const INTERVAL: &str = "interval";

/// The SSH destination of a second Mac.
pub const HOST: &str = "host";

/// The path of `glance` on the second Mac.
pub const REMOTE_BINARY: &str = "remote-binary";

/// Show the name that the second Mac reports, not the destination.
pub const RESOLVE: &str = "resolve";

/// The life of the status item after it is up, in seconds. Hidden.
pub const EXIT_AFTER: &str = "exit-after";

/// The mark of the copy that a background start makes. Hidden.
pub const SPAWNED_COPY: &str = "spawned-copy";

/// Gives the flag `--name`.
#[must_use]
pub fn flag(name: &str) -> String {
    format!("--{name}")
}

/// Gives the flag `--name=value`.
///
/// The value is in the same argument as the flag. Thus a value that starts
/// with `-`, which a remote binary can do, is never read as a flag.
#[must_use]
pub fn flag_with_value(name: &str, value: &str) -> String {
    format!("--{name}={value}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_has_two_dashes() {
        assert_eq!(flag(SPAWNED_COPY), "--spawned-copy");
    }

    #[test]
    fn a_value_is_in_the_same_argument_as_its_flag() {
        assert_eq!(
            flag_with_value(REMOTE_BINARY, "-bin/glance"),
            "--remote-binary=-bin/glance"
        );
    }
}
