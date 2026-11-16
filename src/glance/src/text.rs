//! The words of the menu and the line of the `--sample` flag.
//!
//! These functions are pure. They turn a sample into text, so a test can read
//! each line without AppKit.

use crate::cpu::CpuShare;
use crate::host::HostName;
use crate::pressure::Level;
use crate::sample::Sample;
use serde_json::{json, Value};

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
        (Some(reason), _) => lines_under(read_error_line(reason)),
        (None, Some(sample)) => menu_lines(sample),
        (None, None) => lines_under(WAITING_LINE.to_owned()),
    }
}

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

/// The first menu line after a failed read.
#[must_use]
pub fn read_error_line(reason: &str) -> String {
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
            empty_lines(&read_error_line("cannot read x: y"))
        );
    }

    #[test]
    fn an_error_with_no_sample_clears_the_three_lower_lines() {
        assert_eq!(
            menu_titles(Some("cannot read x: y"), None),
            empty_lines(&read_error_line("cannot read x: y"))
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

    #[test]
    fn a_failed_read_names_the_reason() {
        assert_eq!(
            read_error_line("cannot read x: y"),
            "Cannot read the kernel: cannot read x: y"
        );
    }

    #[test]
    fn the_fixed_titles_are_the_words_of_the_design() {
        assert_eq!(QUIT_TITLE, "Quit glance");
        assert_eq!(WAITING_LINE, "Waiting for the first sample");
    }
}
