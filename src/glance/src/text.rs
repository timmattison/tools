//! The words of the menu and the line of the `--sample` flag.
//!
//! These functions are pure. They turn a sample into text, so a test can read
//! each line without AppKit.

use crate::cpu::CpuShare;
use crate::host::HostName;
use crate::pressure::{Level, Pressure};
use crate::sample::Sample;
use serde_json::{json, Map, Value};
use std::fmt;

/// The start of the first menu line after a failed read.
const READ_ERROR_PREFIX: &str = "Cannot read the kernel: ";
/// The key of the short host name in the JSON line.
const KEY_HOST: &str = "host";
/// The key of the reason in the JSON line of a failed read.
const KEY_ERROR: &str = "error";
/// The key of the user share in the JSON line.
const KEY_USER: &str = "user";
/// The key of the system share in the JSON line.
const KEY_SYSTEM: &str = "system";
/// The key of the idle share in the JSON line.
const KEY_IDLE: &str = "idle";
/// The key of the level name in the JSON line.
const KEY_LEVEL: &str = "pressure_level";
/// The key of the raw level in the JSON line.
const KEY_LEVEL_RAW: &str = "pressure_level_raw";
/// The key of the percentage in use in the JSON line.
const KEY_PERCENT: &str = "pressure_percent";

/// The largest percentage.
const PERCENT_MAX: f64 = 100.0;

/// The title of the menu item that quits the tool.
pub const QUIT_TITLE: &str = "Quit glance";

/// The first menu line before the first sample arrives.
pub const WAITING_LINE: &str = "Waiting for the first sample";

/// The number of info lines in the menu.
pub const MENU_LINES: usize = 4;

/// The four info lines of the menu.
#[must_use]
pub fn menu_lines(sample: &Sample) -> [String; MENU_LINES] {
    let cpu = sample.cpu;
    let pressure = sample.pressure;
    let level = pressure.level();
    let level_text = match level {
        Level::Unknown(raw) => format!("{} (raw {raw})", level.name()),
        _ => level.name().to_owned(),
    };
    [
        format!("User: {}%", percent(cpu.user())),
        format!("System: {}%", percent(cpu.system())),
        format!("Idle: {}%", percent(cpu.idle())),
        format!(
            "Memory pressure: {level_text}, {}% in use",
            pressure.percent_in_use()
        ),
    ]
}

/// The four info lines of the menu, from the last read error and the newest sample.
///
/// An error wins over a sample. After an error, the three lower lines are
/// empty, so the values of an older sample do not look current. With no error
/// and no sample, the first line is [`WAITING_LINE`].
#[must_use]
pub fn menu_titles(error: Option<&str>, newest: Option<&Sample>) -> [String; MENU_LINES] {
    match (error, newest) {
        (Some(reason), _) => lines_under(read_error_line(&LOCAL_PLACEHOLDER, reason)),
        (None, Some(sample)) => menu_lines(sample),
        (None, None) => lines_under(WAITING_LINE.to_owned()),
    }
}

/// The origin that `menu_titles` uses until it takes an origin as a parameter.
const LOCAL_PLACEHOLDER: Origin<'static> = Origin {
    host: "",
    destination: None,
};

/// The first menu line, with three empty lines under it.
fn lines_under(first: String) -> [String; MENU_LINES] {
    [first, String::new(), String::new(), String::new()]
}

/// Rounds a share from 0 to 1 to a whole percentage.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0 to 100 and rounded, so it fits in a u8"
)]
fn percent(share: f64) -> u8 {
    (share * PERCENT_MAX).clamp(0.0, PERCENT_MAX).round() as u8
}

/// The sample as one JSON object on one line, with no trailing newline.
///
/// The keys are `host`, `user`, `system`, `idle`, `pressure_level`,
/// `pressure_level_raw`, and `pressure_percent`, in this order.
#[must_use]
pub fn sample_json(host: &HostName, sample: &Sample) -> String {
    let cpu: &CpuShare = &sample.cpu;
    let level = sample.pressure.level();
    // The `json!` macro sorts the keys, so each value goes through serde_json
    // alone and the line keeps the order of the design.
    let fields = [
        (KEY_HOST, json!(host.as_str())),
        (KEY_USER, json!(cpu.user())),
        (KEY_SYSTEM, json!(cpu.system())),
        (KEY_IDLE, json!(cpu.idle())),
        (KEY_LEVEL, json!(level.name())),
        (KEY_LEVEL_RAW, json!(level.raw())),
        (KEY_PERCENT, json!(sample.pressure.percent_in_use())),
    ];
    object_line(&fields)
}

/// A JSON object on one line, with the keys in the order of `fields`.
fn object_line(fields: &[(&str, Value)]) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{}:{value}", json!(key)))
        .collect();
    format!("{{{}}}", body.join(","))
}

/// The line of a failed read as one JSON object on one line, with no trailing newline.
///
/// The keys are `host` and `error`, in this order. The reason is escaped, so a
/// quote or a newline in it keeps the line on one line.
#[must_use]
pub fn error_json(host: &HostName, reason: &str) -> String {
    object_line(&[(KEY_HOST, json!(host.as_str())), (KEY_ERROR, json!(reason))])
}

/// One line of the stream, read back.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamLine {
    /// A sample that a Mac read.
    Sample {
        /// The short name of the Mac that read the kernel.
        host: HostName,
        /// The sample.
        sample: Sample,
    },
    /// A failed read on a Mac.
    Error {
        /// The short name of the Mac that failed to read.
        host: HostName,
        /// The reason of the failure.
        reason: String,
    },
}

/// The reason why a line is not a stream line.
#[derive(Debug, Clone, PartialEq)]
pub enum LineError {
    /// The line is not JSON.
    NotJson {
        /// The text of the JSON parser.
        reason: String,
    },
    /// The line is JSON, but not an object.
    NotAnObject,
    /// The object has no value for a key that it needs.
    MissingKey {
        /// The key that is absent.
        key: &'static str,
    },
    /// The value of a key has the wrong type.
    WrongType {
        /// The key with the wrong value.
        key: &'static str,
    },
    /// A CPU share is outside 0 to 1.
    ShareOutOfRange {
        /// The key of the share.
        key: &'static str,
        /// The value that the line gave.
        value: f64,
    },
    /// The level name is not the name of the raw level.
    LevelMismatch {
        /// The name that the line gave.
        name: String,
        /// The raw level that the line gave.
        raw: i32,
    },
    /// The percentage in use is above 100.
    PercentAboveMaximum {
        /// The value that the line gave.
        value: u64,
    },
}

impl fmt::Display for LineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LineError::NotJson { reason } => write!(f, "the line is not JSON: {reason}"),
            LineError::NotAnObject => write!(f, "the line is not a JSON object"),
            LineError::MissingKey { key } => write!(f, "the line has no key \"{key}\""),
            LineError::WrongType { key } => {
                write!(f, "the key \"{key}\" holds a value of the wrong type")
            }
            LineError::ShareOutOfRange { key, value } => {
                write!(f, "the key \"{key}\" holds {value}, outside 0 to 1")
            }
            LineError::LevelMismatch { name, raw } => {
                write!(
                    f,
                    "the level name \"{name}\" does not fit the raw level {raw}"
                )
            }
            LineError::PercentAboveMaximum { value } => {
                write!(f, "the percentage {value} is above 100")
            }
        }
    }
}

impl std::error::Error for LineError {}

/// Reads one line of the stream back into a sample or an error.
///
/// The parse of a line that [`sample_json`] or [`error_json`] printed gives
/// the value that was printed.
///
/// # Errors
///
/// Gives a [`LineError`] if the line is not a JSON object, if a key is absent
/// or has the wrong type, if a share is outside 0 to 1, if the level name does
/// not fit the raw level, or if the percentage is above 100.
pub fn parse_line(line: &str) -> Result<StreamLine, LineError> {
    let value: Value =
        serde_json::from_str(line.trim_end()).map_err(|error| LineError::NotJson {
            reason: error.to_string(),
        })?;
    let Value::Object(object) = value else {
        return Err(LineError::NotAnObject);
    };
    let host = HostName::from_raw(text_of(&object, KEY_HOST)?.as_bytes());
    if object.contains_key(KEY_ERROR) {
        let reason = text_of(&object, KEY_ERROR)?.to_owned();
        return Ok(StreamLine::Error { host, reason });
    }
    let sample = sample_of(&object)?;
    Ok(StreamLine::Sample { host, sample })
}

/// The value of a key that the object must hold.
fn required<'a>(object: &'a Map<String, Value>, key: &'static str) -> Result<&'a Value, LineError> {
    object.get(key).ok_or(LineError::MissingKey { key })
}

/// The text of a key that must hold a string.
fn text_of<'a>(object: &'a Map<String, Value>, key: &'static str) -> Result<&'a str, LineError> {
    required(object, key)?
        .as_str()
        .ok_or(LineError::WrongType { key })
}

/// A CPU share that the key holds, checked against 0 to 1.
fn share_of(object: &Map<String, Value>, key: &'static str) -> Result<f64, LineError> {
    let value = required(object, key)?
        .as_f64()
        .ok_or(LineError::WrongType { key })?;
    if (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(LineError::ShareOutOfRange { key, value })
    }
}

/// The sample that the six sample keys of the object hold.
fn sample_of(object: &Map<String, Value>) -> Result<Sample, LineError> {
    let user = share_of(object, KEY_USER)?;
    let system = share_of(object, KEY_SYSTEM)?;
    let idle = share_of(object, KEY_IDLE)?;
    let name = text_of(object, KEY_LEVEL)?;
    let raw = required(object, KEY_LEVEL_RAW)?
        .as_i64()
        .and_then(|number| i32::try_from(number).ok())
        .ok_or(LineError::WrongType { key: KEY_LEVEL_RAW })?;
    if name != Level::from_raw(raw).name() {
        return Err(LineError::LevelMismatch {
            name: name.to_owned(),
            raw,
        });
    }
    let value = required(object, KEY_PERCENT)?
        .as_u64()
        .ok_or(LineError::WrongType { key: KEY_PERCENT })?;
    let pressure = u8::try_from(value)
        .ok()
        .and_then(|percent| Pressure::from_level_and_percent(raw, percent))
        .ok_or(LineError::PercentAboveMaximum { value })?;
    let cpu = CpuShare::from_shares(user, system, idle).ok_or(LineError::ShareOutOfRange {
        key: KEY_USER,
        value: user,
    })?;
    Ok(Sample { cpu, pressure })
}

/// Where the values of the menu come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Origin<'a> {
    /// The name of the Mac whose kernel gave the values.
    pub host: &'a str,
    /// The SSH destination in remote mode, as the user typed it. `None` in local mode.
    pub destination: Option<&'a str>,
}

/// The first info line of the menu. It names the Mac and, in remote mode, the SSH destination.
///
/// The line is never truncated.
#[must_use]
pub fn host_line(origin: &Origin<'_>) -> String {
    String::new()
}

/// The first menu line after a failed read.
#[must_use]
pub fn read_error_line(origin: &Origin<'_>, reason: &str) -> String {
    let _ = origin;
    format!("{READ_ERROR_PREFIX}{reason}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{CpuShare, Ticks};
    use crate::pressure::Pressure;

    const TOLERANCE: f64 = 1e-12;
    /// The key order of the JSON line, as the design file states it.
    const KEY_ORDER: [&str; 7] = [
        "\"host\"",
        "\"user\"",
        "\"system\"",
        "\"idle\"",
        "\"pressure_level\"",
        "\"pressure_level_raw\"",
        "\"pressure_percent\"",
    ];

    /// The host name that the tests print.
    const HOST: &str = "m1-ts";

    fn host() -> HostName {
        HostName::from_raw(HOST.as_bytes())
    }

    fn value_of(line: &str) -> serde_json::Value {
        serde_json::from_str(line).unwrap()
    }

    fn sample(user: u32, system: u32, idle: u32, level: i32, free: i32) -> Sample {
        Sample {
            cpu: CpuShare::between(
                Ticks::default(),
                Ticks {
                    user,
                    system,
                    idle,
                    nice: 0,
                },
            ),
            pressure: Pressure::from_raw(level, free),
        }
    }

    fn warn_sample() -> Sample {
        sample(50, 25, 25, 2, 47)
    }

    fn empty_lines(first: &str) -> [String; MENU_LINES] {
        [
            first.to_owned(),
            String::new(),
            String::new(),
            String::new(),
        ]
    }

    #[test]
    fn an_error_with_a_sample_clears_the_three_lower_lines() {
        assert_eq!(
            menu_titles(Some("cannot read x: y"), Some(&warn_sample())),
            empty_lines(&read_error_line(&LOCAL_PLACEHOLDER, "cannot read x: y"))
        );
    }

    #[test]
    fn an_error_with_no_sample_clears_the_three_lower_lines() {
        assert_eq!(
            menu_titles(Some("cannot read x: y"), None),
            empty_lines(&read_error_line(&LOCAL_PLACEHOLDER, "cannot read x: y"))
        );
    }

    #[test]
    fn a_sample_with_no_error_gives_the_menu_lines() {
        assert_eq!(
            menu_titles(None, Some(&warn_sample())),
            menu_lines(&warn_sample())
        );
    }

    #[test]
    fn no_error_and_no_sample_gives_the_waiting_line() {
        assert_eq!(menu_titles(None, None), empty_lines(WAITING_LINE));
    }

    #[test]
    fn the_menu_shows_the_shares_and_the_pressure() {
        assert_eq!(
            menu_lines(&warn_sample()),
            [
                "User: 50%",
                "System: 25%",
                "Idle: 25%",
                "Memory pressure: warn, 53% in use"
            ]
        );
    }

    #[test]
    fn the_percentages_round_to_the_nearest_integer() {
        let third = menu_lines(&sample(1, 1, 1, 1, 50));
        assert_eq!(third[0], "User: 33%");
        let two_thirds = menu_lines(&sample(2, 0, 1, 1, 50));
        assert_eq!(two_thirds[0], "User: 67%");
    }

    #[test]
    fn an_unknown_level_shows_its_raw_value() {
        let lines = menu_lines(&sample(1, 1, 1, 7, 40));
        assert_eq!(lines[3], "Memory pressure: unknown (raw 7), 60% in use");
    }

    #[test]
    fn the_json_holds_the_values_in_the_design_order_on_one_line() {
        let line = sample_json(&host(), &warn_sample());
        assert_eq!(value_of(&line)["host"], HOST);
        assert!(!line.contains('\n'), "{line}");
        let value = value_of(&line);
        assert!((value["user"].as_f64().unwrap() - 0.5).abs() < TOLERANCE);
        assert!((value["system"].as_f64().unwrap() - 0.25).abs() < TOLERANCE);
        assert!((value["idle"].as_f64().unwrap() - 0.25).abs() < TOLERANCE);
        assert_eq!(value["pressure_level"], "warn");
        assert_eq!(value["pressure_level_raw"], 2);
        assert_eq!(value["pressure_percent"], 53);
        assert_eq!(value.as_object().unwrap().len(), KEY_ORDER.len());
        let positions: Vec<usize> = KEY_ORDER
            .iter()
            .map(|key| line.find(key).unwrap_or(usize::MAX))
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{line}");
    }

    #[test]
    fn the_json_of_an_unknown_level_keeps_the_raw_value() {
        let value: serde_json::Value =
            serde_json::from_str(&sample_json(&host(), &sample(1, 1, 1, 7, 40))).unwrap();
        assert_eq!(value["pressure_level"], "unknown");
        assert_eq!(value["pressure_level_raw"], 7);
    }

    #[test]
    fn the_error_line_holds_the_host_and_the_reason_in_this_order() {
        let line = error_json(&host(), "cannot read x: y");
        assert_eq!(line, r#"{"host":"m1-ts","error":"cannot read x: y"}"#);
    }

    #[test]
    fn a_reason_with_a_quote_and_a_newline_stays_one_line_of_json() {
        let reason = "bad \"key\"\nnext line";
        let line = error_json(&host(), reason);
        assert!(!line.contains('\n'), "{line}");
        let value = value_of(&line);
        assert_eq!(value["error"], reason);
        assert_eq!(value["host"], HOST);
        assert_eq!(value.as_object().unwrap().len(), 2);
    }

    /// The sample lines that the round trip uses: shares of thirds, each level.
    fn round_trip_samples() -> Vec<Sample> {
        vec![
            sample(1, 1, 1, 1, 50),
            sample(2, 0, 1, 2, 47),
            sample(1, 1, 1, 4, 0),
            sample(2, 0, 1, 7, 100),
        ]
    }

    #[test]
    fn the_parse_of_a_printed_sample_gives_the_same_sample() {
        for printed in round_trip_samples() {
            let line = sample_json(&host(), &printed);
            assert_eq!(
                parse_line(&line),
                Ok(StreamLine::Sample {
                    host: host(),
                    sample: printed
                }),
                "{line}"
            );
        }
    }

    #[test]
    fn the_parse_of_a_printed_sample_ignores_a_trailing_newline() {
        let printed = warn_sample();
        let line = format!("{}\n", sample_json(&host(), &printed));
        assert_eq!(
            parse_line(&line),
            Ok(StreamLine::Sample {
                host: host(),
                sample: printed
            })
        );
    }

    #[test]
    fn the_parse_of_a_printed_error_gives_the_same_error() {
        let reason = "bad \"key\"\nnext 日本語 🎉";
        assert_eq!(
            parse_line(&error_json(&host(), reason)),
            Ok(StreamLine::Error {
                host: host(),
                reason: reason.to_owned()
            })
        );
    }

    /// The printed sample line with the value of `key` replaced.
    fn line_with(key: &str, replacement: Value) -> String {
        let mut value = value_of(&sample_json(&host(), &warn_sample()));
        value[key] = replacement;
        value.to_string()
    }

    #[test]
    fn the_parse_refuses_text_that_is_not_json() {
        assert!(matches!(
            parse_line("not json"),
            Err(LineError::NotJson { .. })
        ));
        assert!(matches!(parse_line(""), Err(LineError::NotJson { .. })));
    }

    #[test]
    fn the_parse_refuses_json_that_is_not_an_object() {
        assert_eq!(parse_line("[1,2]"), Err(LineError::NotAnObject));
        assert_eq!(parse_line("\"text\""), Err(LineError::NotAnObject));
    }

    #[test]
    fn the_parse_refuses_a_line_with_an_absent_key() {
        let printed = value_of(&sample_json(&host(), &warn_sample()));
        let keys: Vec<String> = printed.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), KEY_ORDER.len());
        for key in keys {
            let mut cut = printed.clone();
            cut.as_object_mut().unwrap().remove(&key);
            let error = parse_line(&cut.to_string()).unwrap_err();
            assert!(
                matches!(&error, LineError::MissingKey { key: absent } if *absent == key),
                "{key}: {error:?}"
            );
        }
    }

    #[test]
    fn the_parse_refuses_a_share_outside_0_to_1() {
        for (key, bad) in [("user", 1.5), ("system", -0.5), ("idle", 2.0)] {
            let error = parse_line(&line_with(key, json!(bad))).unwrap_err();
            assert!(
                matches!(&error, LineError::ShareOutOfRange { key: named, value }
                    if *named == key && (*value - bad).abs() < TOLERANCE),
                "{key}: {error:?}"
            );
        }
    }

    #[test]
    fn the_parse_refuses_a_percentage_above_100() {
        assert_eq!(
            parse_line(&line_with("pressure_percent", json!(101))),
            Err(LineError::PercentAboveMaximum { value: 101 })
        );
    }

    #[test]
    fn the_parse_refuses_a_raw_level_that_does_not_fit_its_name() {
        assert_eq!(
            parse_line(&line_with("pressure_level_raw", json!(4))),
            Err(LineError::LevelMismatch {
                name: "warn".to_owned(),
                raw: 4
            })
        );
    }

    #[test]
    fn the_parse_refuses_a_value_of_the_wrong_type() {
        assert_eq!(
            parse_line(&line_with("host", json!(7))),
            Err(LineError::WrongType { key: "host" })
        );
        assert_eq!(
            parse_line(&line_with("user", json!("half"))),
            Err(LineError::WrongType { key: "user" })
        );
        assert_eq!(
            parse_line(&line_with("pressure_level_raw", json!(2_i64.pow(40)))),
            Err(LineError::WrongType {
                key: "pressure_level_raw"
            })
        );
        assert_eq!(
            parse_line(&line_with("pressure_percent", json!(-1))),
            Err(LineError::WrongType {
                key: "pressure_percent"
            })
        );
    }

    #[test]
    fn an_error_line_needs_a_host_and_a_text_reason() {
        assert_eq!(
            parse_line(r#"{"error":"x"}"#),
            Err(LineError::MissingKey { key: "host" })
        );
        assert_eq!(
            parse_line(r#"{"host":"a","error":3}"#),
            Err(LineError::WrongType { key: "error" })
        );
    }

    #[test]
    fn the_messages_of_the_refusals_are_short_words() {
        assert_eq!(
            LineError::MissingKey { key: "user" }.to_string(),
            "the line has no key \"user\""
        );
        assert_eq!(
            LineError::ShareOutOfRange {
                key: "user",
                value: 1.5
            }
            .to_string(),
            "the key \"user\" holds 1.5, outside 0 to 1"
        );
    }

    #[test]
    fn a_failed_read_names_the_reason() {
        assert_eq!(
            read_error_line(&LOCAL_PLACEHOLDER, "cannot read x: y"),
            "Cannot read the kernel: cannot read x: y"
        );
    }

    /// The name of the Mac in the host line tests.
    const REMOTE_HOST: &str = "m1-ts";
    /// The SSH destination in the remote tests.
    const DESTINATION: &str = "tim@m1.local";

    fn local(host: &str) -> Origin<'_> {
        Origin {
            host,
            destination: None,
        }
    }

    fn remote(destination: &str) -> Origin<'_> {
        Origin {
            host: REMOTE_HOST,
            destination: Some(destination),
        }
    }

    #[test]
    fn the_host_line_in_local_mode_names_the_mac() {
        assert_eq!(
            host_line(&local("M4-Pro-Mac-Mini")),
            "Host: M4-Pro-Mac-Mini"
        );
    }

    #[test]
    fn the_host_line_in_remote_mode_names_the_destination() {
        assert_eq!(
            host_line(&remote(DESTINATION)),
            "Host: m1-ts (ssh tim@m1.local)"
        );
    }

    #[test]
    fn the_host_line_keeps_a_long_host_name_whole() {
        let name = "h".repeat(80);
        assert_eq!(host_line(&local(&name)), format!("Host: {name}"));
    }

    #[test]
    fn the_error_line_in_local_mode_names_the_kernel() {
        assert_eq!(
            read_error_line(&local("m"), "cannot read x: y"),
            "Cannot read the kernel: cannot read x: y"
        );
    }

    #[test]
    fn the_error_line_in_remote_mode_names_the_destination() {
        assert_eq!(
            read_error_line(&remote(DESTINATION), "Connection refused"),
            "Cannot read tim@m1.local: Connection refused"
        );
    }

    #[test]
    fn the_fixed_titles_are_the_words_of_the_design() {
        assert_eq!(QUIT_TITLE, "Quit glance");
        assert_eq!(WAITING_LINE, "Waiting for the first sample");
    }
}
