//! `cctop-bar`: the cctop board in the menu bar of macOS, with no terminal.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::num::NonZeroU32;
use std::process::ExitCode;

use cctop_bar::chord::Chord;
use cctop_bar::settings::{self, Settings};
use clap::Parser;

/// The one-line description that `--help` shows.
const ABOUT: &str = "The cctop board in the menu bar: a mini-map of the Claude Code sessions";

/// The text that `--help` shows after the options.
const AFTER_HELP: &str = "\
Each user has one status item. When it runs already, cctop-bar stops it and
starts no new one. Thus a second run of the same command closes the status item.
A new status item runs in the background, and cctop-bar returns to the shell
when it is up. -f keeps the status item in the foreground.";

/// The help of `--hotkey`: the default chord, the modifiers, and the keys.
const HOTKEY_HELP: &str = "\
The global hotkey that shows cctop. The default is ctrl+alt+shift+cmd+backspace,
which is written \u{2303}\u{2325}\u{21e7}\u{2318}\u{232b} on the menu. Join the parts with +.
Modifiers: ctrl, alt, shift, cmd. Keys: a-z, 0-9, f1-f20, backspace, space,
escape, return, tab, up, down, left, right.";

/// The text that `cctop-bar` gives on a platform other than macOS.
#[cfg(not(target_os = "macos"))]
const UNSUPPORTED_PLATFORM: &str = "cctop-bar supports macOS only.";

/// The exit code on a platform other than macOS.
#[cfg(not(target_os = "macos"))]
const EXIT_UNSUPPORTED: u8 = 1;

/// The command line of `cctop-bar`.
#[derive(Parser)]
#[command(name = settings::NAME, version = buildinfo::version_string!(), about = ABOUT, after_help = AFTER_HELP)]
struct Cli {
    /// Send no notification when a session changes to needs-you.
    #[arg(long = settings::NO_NOTIFY)]
    no_notify: bool,

    #[arg(
        long = settings::HOTKEY,
        value_name = "CHORD",
        conflicts_with = "no_hotkey",
        help = HOTKEY_HELP
    )]
    hotkey: Option<Chord>,

    /// Register no global hotkey.
    #[arg(long = settings::NO_HOTKEY)]
    no_hotkey: bool,

    /// Run the status item in this process, which keeps the terminal until
    /// the status item stops. Give it to a LaunchAgent.
    #[arg(short, long)]
    foreground: bool,

    /// The mark of the copy that a background start makes. A person never
    /// gives it.
    #[arg(long = settings::SPAWNED_COPY, hide = true, conflicts_with = "foreground")]
    spawned_copy: bool,

    /// End the status item this many seconds after it is up.
    #[arg(long = settings::EXIT_AFTER, hide = true, value_name = "SECONDS")]
    exit_after: Option<NonZeroU32>,
}

impl Cli {
    /// The settings of this command line.
    fn settings(&self) -> Settings {
        Settings {
            notify: !self.no_notify,
            hotkey: if self.no_hotkey {
                None
            } else {
                Some(self.hotkey.unwrap_or_default())
            },
            exit_after: self.exit_after,
        }
    }
}

fn main() -> ExitCode {
    run(&Cli::parse())
}

/// Runs `cctop-bar` on macOS.
#[cfg(target_os = "macos")]
fn run(cli: &Cli) -> ExitCode {
    let _ = (cli.settings(), cli.foreground, cli.spawned_copy);
    ExitCode::SUCCESS
}

/// Refuses to run on a platform other than macOS.
#[cfg(not(target_os = "macos"))]
fn run(_cli: &Cli) -> ExitCode {
    eprintln!("{UNSUPPORTED_PLATFORM}");
    ExitCode::from(EXIT_UNSUPPORTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The settings of the command line `arguments`.
    fn parse(arguments: &[&str]) -> Result<Settings, clap::Error> {
        Cli::try_parse_from(std::iter::once("cctop-bar").chain(arguments.iter().copied()))
            .map(|cli| cli.settings())
    }

    fn chord(text: &str) -> Chord {
        text.parse().expect("a chord")
    }

    #[test]
    fn the_hotkey_help_names_the_default_chord_in_both_forms() {
        assert!(HOTKEY_HELP.contains(cctop_bar::chord::DEFAULT_CHORD));
        assert!(HOTKEY_HELP.contains(&Chord::default().display()));
    }

    #[test]
    fn no_flags_give_the_defaults() {
        assert_eq!(
            parse(&[]).expect("parses"),
            Settings {
                notify: true,
                hotkey: Some(Chord::default()),
                exit_after: None
            }
        );
    }

    #[test]
    fn no_notify_turns_the_notifications_off() {
        assert!(!parse(&["--no-notify"]).expect("parses").notify);
    }

    #[test]
    fn no_hotkey_gives_no_hotkey() {
        assert_eq!(parse(&["--no-hotkey"]).expect("parses").hotkey, None);
    }

    #[test]
    fn a_given_chord_is_the_hotkey() {
        assert_eq!(
            parse(&["--hotkey", "ctrl+cmd+k"]).expect("parses").hotkey,
            Some(chord("ctrl+cmd+k"))
        );
    }

    #[test]
    fn the_exit_after_flag_gives_the_bound() {
        assert_eq!(
            parse(&["--exit-after", "3"]).expect("parses").exit_after,
            NonZeroU32::new(3)
        );
    }

    #[test]
    fn a_hotkey_with_no_hotkey_is_a_usage_error() {
        let error = parse(&["--hotkey", "ctrl+cmd+k", "--no-hotkey"]).expect_err("conflict");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn a_chord_with_no_modifier_is_a_usage_error_with_the_chord_message() {
        let error = parse(&["--hotkey", "backspace"]).expect_err("bad chord");
        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(
            error
                .to_string()
                .contains(&cctop_bar::chord::ChordError::NoModifier.to_string()),
            "{error}"
        );
    }

    #[test]
    fn the_foreground_flag_conflicts_with_the_spawned_copy() {
        assert!(parse(&["-f", "--spawned-copy"]).is_err());
    }

    #[test]
    fn the_copy_of_each_setting_parses_back_to_the_same_setting() {
        let chords = [None, Some(Chord::default()), Some(chord("ctrl+cmd+k"))];
        for notify in [true, false] {
            for hotkey in chords {
                for exit_after in [None, NonZeroU32::new(30)] {
                    let original = Settings {
                        notify,
                        hotkey,
                        exit_after,
                    };
                    let arguments = settings::child_arguments(&original);
                    let refs: Vec<&str> = arguments.iter().map(String::as_str).collect();
                    assert_eq!(parse(&refs).expect("parses"), original, "{arguments:?}");
                }
            }
        }
    }
}
