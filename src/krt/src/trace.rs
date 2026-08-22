//! The privilege gate of a run, and the wall in front of the tracer.
//!
//! `krt` probes through the trippy crates. No other module of `krt` names a
//! type of those crates, so an upgrade of them breaks this one file and no
//! other. The guard `repo_guards::trippy_wall` holds that rule in place.
//!
//! This slice builds the gate. The gate asks the platform whether a probe needs
//! raw socket privileges, and whether the process holds them. A platform that
//! needs none runs unprivileged, even when the process holds them, because a
//! run that quietly changes the way it probes records one thing and does
//! another. A platform that needs them and holds none stops, and the message
//! names the remedy of each platform. The tracer arrives in a later slice.

use crate::record;

/// The remedy of a platform that needs raw socket privileges and holds none.
///
/// `main` writes every message as `krt: {reason}`, so the text carries no
/// program name. The two lines under the first one carry two spaces each, and
/// the remedy of each platform starts at the same column.
#[allow(
    dead_code,
    reason = "main wires the privilege gate beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
const MISSING_PRIVILEGES: &str = "\
this platform needs raw socket privileges to send probes.
  Linux:   sudo setcap 'cap_net_raw+p' $(which krt)
  Windows: run krt from an elevated prompt";

/// Acquires the privileges of the platform and decides the mode of a run.
///
/// # Errors
///
/// Returns [`PrivilegeError::Missing`] when the platform needs raw socket
/// privileges and the process does not hold them. Returns
/// [`PrivilegeError::Discovery`] when the platform will not report what it
/// holds.
#[allow(
    dead_code,
    reason = "main wires the privilege gate beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
pub(crate) fn acquire_privilege() -> Result<record::Privilege, PrivilegeError> {
    let privilege = trippy_privilege::Privilege::acquire_privileges().map_err(|error| {
        PrivilegeError::Discovery {
            reason: error.to_string(),
        }
    })?;
    choose_privilege(privilege.has_privileges(), privilege.needs_privileges())
}

/// Decides the mode of a run from what the platform reports.
///
/// `has` is true when the process holds raw socket privileges. `needs` is true
/// when a probe of this platform needs them.
///
/// # Errors
///
/// Returns [`PrivilegeError::Missing`] when the platform needs the privileges
/// and the process holds none.
#[allow(
    dead_code,
    reason = "main wires the privilege gate beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
fn choose_privilege(has: bool, needs: bool) -> Result<record::Privilege, PrivilegeError> {
    match (needs, has) {
        // macOS sends through an `IPPROTO_ICMP` socket with the `IP_HDRINCL`
        // socket option, so it needs no privileges. A process that holds them
        // there still runs unprivileged, because `krt` never changes the way it
        // probes without a word.
        (false, _) => Ok(record::Privilege::Unprivileged),
        (true, true) => Ok(record::Privilege::Privileged),
        (true, false) => Err(PrivilegeError::Missing),
    }
}

/// Why a run does not start.
#[allow(
    dead_code,
    reason = "main wires the privilege gate beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum PrivilegeError {
    /// The platform needs raw socket privileges, and the process holds none.
    ///
    /// Linux supports an `IPPROTO_ICMP` socket and does not support the
    /// `IP_HDRINCL` socket option, so it needs `CAP_NET_RAW`. Windows needs an
    /// elevated token. The message names the remedy of each one.
    #[error("{MISSING_PRIVILEGES}")]
    Missing,
    /// The platform will not report the privileges that it holds.
    #[error("the platform will not report the privileges that it holds: {reason}")]
    Discovery {
        /// The reason that the platform gave.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{choose_privilege, PrivilegeError};
    use crate::record::Privilege;
    use crate::PROGRAM;

    /// The remedy, exactly as the design writes it.
    ///
    /// `main` writes every message as `krt: {reason}`, so the text carries no
    /// program name. The two lines under the first one carry two spaces each,
    /// and the remedy of each platform starts at the same column.
    const REMEDY: &str = "\
this platform needs raw socket privileges to send probes.
  Linux:   sudo setcap 'cap_net_raw+p' $(which krt)
  Windows: run krt from an elevated prompt";

    /// The first line that `main` writes for a platform without the privileges.
    const FIRST_LINE: &str = "krt: this platform needs raw socket privileges to send probes.";

    /// The reason of a platform that will not report what it holds.
    const A_REASON: &str = "the operating system refused the query";

    /// The mode of a run that the platform admits.
    fn mode(has: bool, needs: bool) -> Privilege {
        choose_privilege(has, needs).expect("the platform must admit a mode")
    }

    /// The fault of a platform that the gate stops.
    fn fault(has: bool, needs: bool) -> PrivilegeError {
        choose_privilege(has, needs).expect_err("the gate must stop the run")
    }

    #[test]
    fn a_platform_that_needs_no_privileges_runs_unprivileged() {
        assert_eq!(mode(false, false), Privilege::Unprivileged);
    }

    /// macOS needs no privileges, so a `sudo krt` on macOS still runs
    /// unprivileged. The design decides that case against the reflex: `krt`
    /// never changes the way it probes without a word.
    #[test]
    fn a_platform_that_needs_no_privileges_runs_unprivileged_even_with_them() {
        assert_eq!(mode(true, false), Privilege::Unprivileged);
    }

    #[test]
    fn a_platform_that_needs_privileges_and_holds_them_runs_privileged() {
        assert_eq!(mode(true, true), Privilege::Privileged);
    }

    #[test]
    fn a_platform_that_needs_privileges_and_holds_none_stops_the_run() {
        assert_eq!(fault(false, true), PrivilegeError::Missing);
    }

    #[test]
    fn the_message_of_a_missing_privilege_names_the_remedy_of_each_platform() {
        assert_eq!(PrivilegeError::Missing.to_string(), REMEDY);
    }

    #[test]
    fn the_line_that_main_writes_names_the_program_and_the_reason() {
        let error = PrivilegeError::Missing;
        let line = format!("{PROGRAM}: {error}");
        assert!(
            line.starts_with(FIRST_LINE),
            "the line names the program and the reason: {line}"
        );
    }

    #[test]
    fn a_platform_that_will_not_report_what_it_holds_names_the_reason() {
        let error = PrivilegeError::Discovery {
            reason: A_REASON.to_owned(),
        };
        let message = error.to_string();
        assert!(
            message.contains(A_REASON),
            "the message names the reason: {message}"
        );
    }
}
