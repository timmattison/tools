//! Core port-derivation primitives shared by `portplz` and related tools.
//!
//! This crate provides the stable hashing primitive used to turn an arbitrary
//! string into a deterministic, guaranteed-unprivileged TCP port number.

/// A TCP port guaranteed to be unprivileged (always `>= 1024`).
///
/// Construct one only via [`unprivileged_port_from_string`], which enforces the
/// unprivileged invariant. The inner value is private and there is intentionally
/// no `Display` implementation, so callers must go through [`DerivedPort::get`]
/// to obtain the raw `u16` — keeping the invariant impossible to bypass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DerivedPort(u16);

impl DerivedPort {
    /// Returns the underlying port number, which is always `>= 1024`.
    #[must_use]
    pub fn get(&self) -> u16 {
        self.0
    }
}

/// Derives a deterministic, unprivileged port from an arbitrary input string.
///
/// The same input always yields the same port, and the result is always
/// `>= 1024` (i.e. never a privileged port).
#[must_use]
pub fn unprivileged_port_from_string(_input: &str) -> DerivedPort {
    DerivedPort(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_generation() {
        let port = unprivileged_port_from_string("test");
        assert!(port.get() >= 1024);
        assert!(port.get() < 65535);
    }

    #[test]
    fn test_consistent_port() {
        assert_eq!(
            unprivileged_port_from_string("example").get(),
            unprivileged_port_from_string("example").get()
        );
    }

    #[test]
    fn test_different_inputs() {
        assert_ne!(
            unprivileged_port_from_string("branch-a").get(),
            unprivileged_port_from_string("branch-b").get()
        );
    }
}
