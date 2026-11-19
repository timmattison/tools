//! The settings of `cctop-bar` and the text that the launch step needs: the
//! arguments of the background copy and the two lines that tell the state of
//! the status item. The module has no `AppKit` code, thus it runs everywhere.

use std::fmt::Display;
use std::num::NonZeroU32;

use crate::chord::Chord;

/// The long flag that turns the notifications off (no leading dashes).
pub const NO_NOTIFY: &str = "no-notify";
/// The long flag that gives the global hotkey (no leading dashes).
pub const HOTKEY: &str = "hotkey";
/// The long flag that turns the global hotkey off (no leading dashes).
pub const NO_HOTKEY: &str = "no-hotkey";
/// The long flag that marks the background copy (no leading dashes).
pub const SPAWNED_COPY: &str = "spawned-copy";
/// The long flag that bounds the life of the status item (no leading dashes).
pub const EXIT_AFTER: &str = "exit-after";
/// The name of the tool at the start of each message.
pub const NAME: &str = "cctop-bar";

/// The settings of the status item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Whether a change to needs-you sends a notification.
    pub notify: bool,
    /// The global hotkey, or `None` for no hotkey.
    pub hotkey: Option<Chord>,
    /// The life of the status item after it is up, or `None` for no bound.
    pub exit_after: Option<NonZeroU32>,
}

/// Gives the arguments that a background start gives to its copy.
///
/// The list holds `--spawned-copy`, then `--no-notify` when the notifications
/// are off, then `--hotkey=<chord>` or `--no-hotkey`, then `--exit-after=<n>`
/// when the life is bounded. The copy parses them with the same command line
/// as a person's run, so the copy gets the same settings.
#[must_use]
pub fn child_arguments(settings: &Settings) -> Vec<String> {
    let mut arguments = vec![format!("--{SPAWNED_COPY}")];
    if !settings.notify {
        arguments.push(format!("--{NO_NOTIFY}"));
    }
    arguments.push(match settings.hotkey {
        Some(chord) => format!("--{HOTKEY}={chord}"),
        None => format!("--{NO_HOTKEY}"),
    });
    if let Some(seconds) = settings.exit_after {
        arguments.push(format!("--{EXIT_AFTER}={seconds}"));
    }
    arguments
}

/// The line that tells that the status item is up.
#[must_use]
pub fn ready_line(pid: impl Display) -> String {
    format!("{NAME}: the status item runs as PID {pid}. Run {NAME} again to stop it.")
}

/// The line that tells that a run stopped the status item.
#[must_use]
pub fn stopped_line(pid: impl Display) -> String {
    format!("{NAME}: stopped the status item that ran as PID {pid}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(notify: bool, hotkey: Option<Chord>, exit_after: Option<u32>) -> Settings {
        Settings {
            notify,
            hotkey,
            exit_after: exit_after.and_then(NonZeroU32::new),
        }
    }

    #[test]
    fn the_default_settings_give_the_copy_the_default_chord() {
        assert_eq!(
            child_arguments(&settings(true, Some(Chord::default()), None)),
            ["--spawned-copy", "--hotkey=ctrl+alt+shift+cmd+backspace"]
        );
    }

    #[test]
    fn a_start_with_no_notifications_gives_the_copy_no_notify() {
        assert_eq!(
            child_arguments(&settings(false, Some(Chord::default()), None)),
            [
                "--spawned-copy",
                "--no-notify",
                "--hotkey=ctrl+alt+shift+cmd+backspace"
            ]
        );
    }

    #[test]
    fn a_start_with_no_hotkey_gives_the_copy_no_hotkey() {
        assert_eq!(
            child_arguments(&settings(true, None, None)),
            ["--spawned-copy", "--no-hotkey"]
        );
    }

    #[test]
    fn a_custom_chord_goes_to_the_copy_in_its_canonical_form() {
        let chord: Chord = "Command+Control+K".parse().expect("a chord");
        assert_eq!(
            child_arguments(&settings(true, Some(chord), None)),
            ["--spawned-copy", "--hotkey=ctrl+cmd+k"]
        );
    }

    #[test]
    fn a_bound_life_goes_last() {
        assert_eq!(
            child_arguments(&settings(false, None, Some(30))),
            [
                "--spawned-copy",
                "--no-notify",
                "--no-hotkey",
                "--exit-after=30"
            ]
        );
    }

    #[test]
    fn the_ready_line_names_the_pid_and_the_command_that_stops() {
        assert_eq!(
            ready_line(4242),
            "cctop-bar: the status item runs as PID 4242. Run cctop-bar again to stop it."
        );
    }

    #[test]
    fn the_stopped_line_names_the_pid() {
        assert_eq!(
            stopped_line(4242),
            "cctop-bar: stopped the status item that ran as PID 4242."
        );
    }
}
