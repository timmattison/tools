//! The settings of `ha`: what to watch, how far back to look, and how often
//! to refresh.
//!
//! The settings come from two places: the command line and a TOML config
//! file. The command line wins. This module reads the file, merges the two
//! sources, and checks every value. Each error names the key, the value, and
//! the file, so a person can find the mistake at once.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::target::{Target, TargetError};

/// The window `ha` uses when nothing sets one: the finished runs of 8 hours.
pub const DEFAULT_WINDOW: Duration = Duration::from_hours(8);

/// The refresh interval `ha` uses when nothing sets one: 60 seconds.
pub const DEFAULT_INTERVAL: Duration = Duration::from_mins(1);

/// The reasons why `ha` cannot build its settings. Each message names the
/// input that caused the error.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// A duration text is not a whole number above zero with a unit.
    #[error(
        "invalid duration \"{input}\": write a whole number above zero and a unit, \
         s, m, h, or d (for example 30s, 15m, 8h, or 2d)"
    )]
    InvalidDuration {
        /// The text as given, after the trim.
        input: String,
    },
    /// The config file is not valid TOML, or it holds a key that `ha` does not
    /// know, or a value of the wrong type.
    #[error("{path}: {source}")]
    Toml {
        /// The config file.
        path: String,
        /// What the TOML reader found.
        source: toml::de::Error,
    },
    /// A duration in the config file is not valid.
    #[error("{key} in {path}: {source}")]
    FileDuration {
        /// The config key that holds the value.
        key: &'static str,
        /// The config file.
        path: String,
        /// Why the duration is not valid.
        source: Box<SettingsError>,
    },
    /// A repository or owner in the config file is not valid.
    #[error("{key} in {path}: {source}")]
    FileTarget {
        /// The config key that holds the value.
        key: &'static str,
        /// The config file.
        path: String,
        /// Why the value is not valid.
        source: TargetError,
    },
    /// A target on the command line is not valid.
    #[error("target on the command line: {source}")]
    CliTarget {
        /// Why the target is not valid.
        source: TargetError,
    },
    /// Nothing says what to watch.
    #[error("no targets: give owner/name or owner arguments, or add repos or owners to {config}")]
    NoTargets {
        /// The config file, or a description of it when the system gives no
        /// path.
        config: String,
    },
}

/// What `ha` needs to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The repositories and owners to watch. The list is never empty.
    pub targets: Vec<Target>,
    /// How far back the board shows finished runs.
    pub window: Duration,
    /// How long `ha` waits between two refreshes.
    pub interval: Duration,
}

/// The settings that the command line gave. The command line wins over the
/// config file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandLine {
    /// The `owner` and `owner/name` arguments, as typed. When the list is not
    /// empty, it replaces all targets of the config file.
    pub targets: Vec<String>,
    /// The `--config` path.
    pub config: Option<PathBuf>,
    /// The `--window` value.
    pub window: Option<Duration>,
    /// The `--interval` value.
    pub interval: Option<Duration>,
}

/// Builds the settings from the command line and the text of the config file.
///
/// `file` holds the path of the config file and its text, or `None` when there
/// is no file. The path is only for error messages. This function does no I/O.
///
/// # Errors
///
/// Returns a [`SettingsError`] when the file text is not valid, when a value
/// is not valid, or when no target is left.
pub fn resolve(_cli: &CommandLine, file: Option<(&Path, &str)>) -> Result<Settings, SettingsError> {
    let (path, parsed) = match file {
        Some((path, text)) => (
            path.display().to_string(),
            toml::from_str::<FileConfig>(text).map_err(|source| SettingsError::Toml {
                path: path.display().to_string(),
                source,
            })?,
        ),
        None => (String::new(), FileConfig::default()),
    };
    let duration = |key: &'static str, value: Option<&String>, default: Duration| match value {
        Some(text) => parse_duration(text).map_err(|source| SettingsError::FileDuration {
            key,
            path: path.clone(),
            source: Box::new(source),
        }),
        None => Ok(default),
    };
    let window = duration("window", parsed.window.as_ref(), DEFAULT_WINDOW)?;
    let interval = duration("interval", parsed.interval.as_ref(), DEFAULT_INTERVAL)?;
    let targets = parsed
        .repos
        .iter()
        .chain(&parsed.owners)
        .filter_map(|text| Target::parse(text).ok())
        .collect();
    Ok(Settings {
        targets,
        window,
        interval,
    })
}

/// The keys of the config file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    repos: Vec<String>,
    #[serde(default)]
    owners: Vec<String>,
    window: Option<String>,
    interval: Option<String>,
}

/// Reads a duration such as `30s`, `15m`, `8h`, or `2d`.
///
/// The text is a whole number above zero and one unit: `s` for seconds, `m`
/// for minutes, `h` for hours, or `d` for days. Spaces around the text do not
/// count.
///
/// # Errors
///
/// Returns [`SettingsError::InvalidDuration`] when the text has another shape,
/// when the number is zero, or when the result does not fit in a duration.
pub fn parse_duration(input: &str) -> Result<Duration, SettingsError> {
    let text = input.trim();
    let invalid = || SettingsError::InvalidDuration {
        input: text.to_owned(),
    };
    let unit = text.chars().next_back().ok_or_else(invalid)?;
    let seconds_per_unit: u64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3_600,
        'd' => 86_400,
        _ => return Err(invalid()),
    };
    let digits = text.strip_suffix(unit).ok_or_else(invalid)?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    let count: u64 = digits.parse().map_err(|_| invalid())?;
    let seconds = count.checked_mul(seconds_per_unit).ok_or_else(invalid)?;
    if seconds == 0 {
        return Err(invalid());
    }
    Ok(Duration::from_secs(seconds))
}

/// Writes a duration the way a person writes it in a setting.
///
/// The text uses the largest unit that divides the duration exactly, for
/// example `8h`, `90m`, `45s`, or `2d`. A zero duration gives `0s`. Parts of
/// a second do not show.
#[must_use]
pub fn format_duration_setting(d: Duration) -> String {
    let seconds = d.as_secs();
    let units = [(86_400, 'd'), (3_600, 'h'), (60, 'm')];
    for (seconds_per_unit, suffix) in units {
        if seconds > 0 && seconds.is_multiple_of(seconds_per_unit) {
            return format!("{}{suffix}", seconds / seconds_per_unit);
        }
    }
    format!("{seconds}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG_PATH: &str = "/home/tester/.config/ha/config.toml";

    fn target(text: &str) -> Target {
        Target::parse(text).expect("the test target is valid")
    }

    fn resolve_file(cli: &CommandLine, text: &str) -> Result<Settings, SettingsError> {
        resolve(cli, Some((Path::new(CONFIG_PATH), text)))
    }

    #[test]
    fn parse_duration_reads_each_unit() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("15m").unwrap(), Duration::from_mins(15));
        assert_eq!(parse_duration("8h").unwrap(), Duration::from_hours(8));
        assert_eq!(parse_duration("2d").unwrap(), Duration::from_hours(48));
    }

    #[test]
    fn parse_duration_trims_whitespace() {
        assert_eq!(parse_duration("  8h\t").unwrap(), Duration::from_hours(8));
    }

    #[test]
    fn parse_duration_rejects_a_bad_text_and_quotes_it() {
        let bad = [
            "0s",
            "0d",
            "8x",
            "h",
            "",
            "   ",
            "-1h",
            "+5s",
            "1.5h",
            "8 h",
            "99999999999999999999d",
            "18446744073709551615d",
            "日本語h",
        ];
        for input in bad {
            let error = parse_duration(input).expect_err(input).to_string();
            assert!(
                error.contains(&format!("\"{}\"", input.trim())),
                "{input:?}: the message does not quote the input: {error}"
            );
            assert!(
                error.contains("s, m, h, or d"),
                "{input:?}: the message does not name the units: {error}"
            );
        }
    }

    #[test]
    fn format_duration_setting_uses_the_largest_exact_unit() {
        let cases = [
            (Duration::from_hours(8), "8h"),
            (Duration::from_mins(90), "90m"),
            (Duration::from_secs(45), "45s"),
            (Duration::from_hours(48), "2d"),
            (Duration::from_secs(86_401), "86401s"),
            (Duration::from_hours(36), "36h"),
            (Duration::ZERO, "0s"),
        ];
        for (duration, expected) in cases {
            assert_eq!(format_duration_setting(duration), expected);
        }
    }

    #[test]
    fn format_duration_setting_round_trips_through_parse_duration() {
        for text in ["30s", "90m", "8h", "2d", "7d"] {
            let duration = parse_duration(text).unwrap();
            assert_eq!(format_duration_setting(duration), text);
        }
    }

    #[test]
    fn resolve_reads_every_key_of_the_file() {
        let text = r#"
            repos = ["timmattison/tools", "Lock-Free-Labs/nvr-tools"]
            owners = ["Lock-Free-Labs"]
            window = "2d"
            interval = "15m"
        "#;
        let settings = resolve_file(&CommandLine::default(), text).unwrap();
        assert_eq!(
            settings.targets,
            vec![
                target("timmattison/tools"),
                target("Lock-Free-Labs/nvr-tools"),
                target("Lock-Free-Labs"),
            ]
        );
        assert_eq!(settings.window, Duration::from_hours(48));
        assert_eq!(settings.interval, Duration::from_mins(15));
    }

    #[test]
    fn resolve_uses_the_defaults_for_a_key_the_file_leaves_out() {
        let settings = resolve_file(&CommandLine::default(), "owners = [\"timmattison\"]").unwrap();
        assert_eq!(settings.targets, vec![target("timmattison")]);
        assert_eq!(settings.window, Duration::from_hours(8));
        assert_eq!(settings.interval, Duration::from_secs(60));
    }
}
