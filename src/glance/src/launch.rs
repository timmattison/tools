//! What one run of `glance` does with the status item of its target: start it
//! here, start it in the background, or stop it.
//!
//! The flow is in [`soleinstance::launch`]. This module holds what is specific
//! to `glance`: the settings, the arguments of the background copy, the lines
//! that tell the state of the status item, and the run of the status item
//! itself.
//!
//! Each target has one status item (see [`crate::instance`]). A run first
//! looks for the status item of its target. When one runs, the run stops it,
//! starts nothing, and ends. Thus a second run of the same command closes the
//! status item that the first run opened. A run for a different target
//! leaves that status item alone.

use std::io;
use std::num::NonZeroU32;
use std::process::ExitCode;

use soleinstance::instance::{LockFile, Pid};
pub use soleinstance::launch::Mode;
use soleinstance::launch::Program;

use crate::connection::{HostNaming, Remote};
use crate::flags::{
    flag, flag_with_value, EXIT_AFTER, HOST, INTERVAL, REMOTE_BINARY, RESOLVE, SPAWNED_COPY,
};
use crate::instance::{self, Target};
use crate::interval::Interval;
use crate::shell;

/// The name of the tool at the start of each message.
const NAME: &str = "glance";

/// The settings of the status item.
#[derive(Debug, Clone)]
pub struct Settings {
    /// The time between two samples.
    pub interval: Interval,
    /// The second Mac that the status item shows, or `None` for this Mac.
    pub remote: Option<Remote>,
    /// The life of the status item after it is up, or `None` for no bound.
    pub exit_after: Option<NonZeroU32>,
}

impl Settings {
    /// Gives the target of these settings. The naming of the second Mac does
    /// not change the target.
    #[must_use]
    pub fn target(&self) -> Target {
        self.remote.as_ref().map_or(Target::Local, |remote| {
            Target::Host(remote.destination.clone())
        })
    }
}

/// Gives the arguments that a background start gives to its copy.
///
/// The copy parses them with the same command line as a person's run, so
/// the copy gets the same settings.
#[must_use]
pub fn child_arguments(settings: &Settings) -> Vec<String> {
    let mut arguments = vec![
        flag(SPAWNED_COPY),
        flag_with_value(INTERVAL, &settings.interval.to_string()),
    ];
    if let Some(remote) = &settings.remote {
        arguments.push(flag_with_value(HOST, remote.destination.as_str()));
        arguments.push(flag_with_value(
            REMOTE_BINARY,
            remote.remote_binary.as_str(),
        ));
        if remote.naming == HostNaming::Reported {
            arguments.push(flag(RESOLVE));
        }
    }
    if let Some(seconds) = settings.exit_after {
        arguments.push(flag_with_value(EXIT_AFTER, &seconds.to_string()));
    }
    arguments
}

/// Starts or stops the status item of the target of `settings`, as `mode`
/// tells, and gives the exit code of the run.
///
/// A foreground run returns only when the status item cannot start. A
/// background run returns when the status item of its copy is up.
#[must_use]
pub fn run(mode: Mode, settings: Settings) -> ExitCode {
    soleinstance::launch::run(mode, Glance { settings })
}

/// The status item of `glance`, as [`soleinstance::launch`] runs it.
struct Glance {
    settings: Settings,
}

impl Program for Glance {
    fn name(&self) -> &str {
        NAME
    }

    fn lock(&self) -> io::Result<LockFile> {
        instance::lock_file_for_user(&self.settings.target())
    }

    fn child_arguments(&self) -> Vec<String> {
        child_arguments(&self.settings)
    }

    fn ready_line(&self, pid: Pid) -> String {
        ready_line(&self.settings.target(), pid)
    }

    fn stopped_line(&self, pid: Pid) -> String {
        stopped_line(&self.settings.target(), pid)
    }

    fn exit_after(&self) -> Option<NonZeroU32> {
        self.settings.exit_after
    }

    fn run_here(self, on_ready: Box<dyn FnOnce()>) -> ExitCode {
        shell::run(self.settings.interval, self.settings.remote, on_ready)
    }
}

/// The line that tells that the status item of `target` is up.
fn ready_line(target: &Target, pid: Pid) -> String {
    match target {
        Target::Local => {
            format!("glance: the status item runs as PID {pid}. Run glance again to stop it.")
        }
        Target::Host(destination) => format!(
            "glance: the status item of the host {destination} runs as PID {pid}. \
             Run glance again with the same --host to stop it."
        ),
    }
}

/// The line that tells that a run stopped the status item of `target`.
fn stopped_line(target: &Target, pid: Pid) -> String {
    match target {
        Target::Local => format!("glance: stopped the status item that ran as PID {pid}."),
        Target::Host(destination) => format!(
            "glance: stopped the status item of the host {destination} that ran as PID {pid}."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{HostNaming, RemoteBinary};

    /// The PID of the lines in these tests.
    fn pid() -> Pid {
        Pid::new(4242).expect("a PID")
    }

    /// The settings of the host `destination` with the remote binary `binary`.
    fn host_settings(destination: &str, binary: &str, naming: HostNaming) -> Settings {
        Settings {
            interval: Interval::DEFAULT,
            remote: Some(Remote {
                destination: destination.parse().expect("a destination"),
                remote_binary: binary.parse::<RemoteBinary>().expect("a remote binary"),
                naming,
            }),
            exit_after: NonZeroU32::new(30),
        }
    }

    #[test]
    fn the_local_lines_name_the_pid_and_the_command_that_stops() {
        assert_eq!(
            ready_line(&Target::Local, pid()),
            "glance: the status item runs as PID 4242. Run glance again to stop it."
        );
        assert_eq!(
            stopped_line(&Target::Local, pid()),
            "glance: stopped the status item that ran as PID 4242."
        );
    }

    #[test]
    fn the_host_lines_name_the_destination() {
        let target = host_settings("tim@m1-ts", "glance", HostNaming::Destination).target();
        assert_eq!(
            ready_line(&target, pid()),
            "glance: the status item of the host tim@m1-ts runs as PID 4242. \
             Run glance again with the same --host to stop it."
        );
        assert_eq!(
            stopped_line(&target, pid()),
            "glance: stopped the status item of the host tim@m1-ts that ran as PID 4242."
        );
    }

    #[test]
    fn settings_with_no_remote_target_this_mac() {
        let settings = Settings {
            interval: Interval::DEFAULT,
            remote: None,
            exit_after: None,
        };
        assert_eq!(settings.target(), Target::Local);
        assert_eq!(
            child_arguments(&settings),
            ["--spawned-copy", "--interval=1"]
        );
    }

    #[test]
    fn the_copy_of_a_resolve_start_gets_the_resolve_flag() {
        assert_eq!(
            child_arguments(&host_settings("m1-ts", "-odd/glance", HostNaming::Reported)),
            [
                "--spawned-copy",
                "--interval=1",
                "--host=m1-ts",
                "--remote-binary=-odd/glance",
                "--resolve",
                "--exit-after=30",
            ]
        );
    }

    #[test]
    fn the_copy_of_a_host_gets_the_destination_the_binary_and_the_bound() {
        assert_eq!(
            child_arguments(&host_settings(
                "m1-ts",
                "-odd/glance",
                HostNaming::Destination
            )),
            [
                "--spawned-copy",
                "--interval=1",
                "--host=m1-ts",
                "--remote-binary=-odd/glance",
                "--exit-after=30",
            ]
        );
    }
}
