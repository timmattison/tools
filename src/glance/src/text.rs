//! The words of the menu and the line of the `--sample` flag.
//!
//! These functions are pure. They turn a sample into text, so a test can read
//! each line without AppKit.

use crate::sample::Sample;

/// The title of the menu item that quits the tool.
pub const QUIT_TITLE: &str = "Quit glance";

/// The first menu line before the first sample arrives.
pub const WAITING_LINE: &str = "Waiting for the first sample";

/// The four info lines of the menu.
#[must_use]
pub fn menu_lines(sample: &Sample) -> [String; 4] {
    let _ = sample;
    [String::new(), String::new(), String::new(), String::new()]
}

/// The sample as one JSON object on one line, with no trailing newline.
///
/// The keys are `user`, `system`, `idle`, `pressure_level`,
/// `pressure_level_raw`, and `pressure_percent`, in this order.
#[must_use]
pub fn sample_json(sample: &Sample) -> String {
    let _ = sample;
    String::from("{}")
}

/// The first menu line after a failed read.
#[must_use]
pub fn read_error_line(reason: &str) -> String {
    let _ = reason;
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{CpuShare, Ticks};
    use crate::pressure::Pressure;

    const TOLERANCE: f64 = 1e-12;
    /// The key order of the JSON line, as the design file states it.
    const KEY_ORDER: [&str; 6] = [
        "\"user\"",
        "\"system\"",
        "\"idle\"",
        "\"pressure_level\"",
        "\"pressure_level_raw\"",
        "\"pressure_percent\"",
    ];

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
        let line = sample_json(&warn_sample());
        assert!(!line.contains('\n'), "{line}");
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
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
            serde_json::from_str(&sample_json(&sample(1, 1, 1, 7, 40))).unwrap();
        assert_eq!(value["pressure_level"], "unknown");
        assert_eq!(value["pressure_level_raw"], 7);
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
