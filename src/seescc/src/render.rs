//! Pure rendering of the one-shot human view: a header line (label + injected
//! wall-clock) and a right-aligned table of metric rows. Time is injected as a
//! preformatted string so this module stays pure and deterministic under test.

use std::time::Duration;

use serde::ser::SerializeMap;
use unicode_width::UnicodeWidthStr;

/// Format a history `window` as a compact human string using the largest unit
/// that divides it *exactly*.
///
/// The window is reported with the coarsest whole unit: `900s -> "15m"`,
/// `3600s -> "1h"`, `5400s -> "90m"`. A duration that is not a whole number of
/// the next coarser unit stays at the finer one, so `90s -> "90s"` (never
/// `"1.5m"`) and `Duration::from_millis(500) -> "500ms"`. A zero window renders
/// as `"0s"`.
pub(crate) fn format_window(window: Duration) -> String {
    let total_millis = window.as_millis();
    if total_millis == 0 {
        return "0s".to_string();
    }

    const MILLIS_PER_SECOND: u128 = 1_000;
    const MILLIS_PER_MINUTE: u128 = 60 * MILLIS_PER_SECOND;
    const MILLIS_PER_HOUR: u128 = 60 * MILLIS_PER_MINUTE;

    if total_millis.is_multiple_of(MILLIS_PER_HOUR) {
        format!("{}h", total_millis / MILLIS_PER_HOUR)
    } else if total_millis.is_multiple_of(MILLIS_PER_MINUTE) {
        format!("{}m", total_millis / MILLIS_PER_MINUTE)
    } else if total_millis.is_multiple_of(MILLIS_PER_SECOND) {
        format!("{}s", total_millis / MILLIS_PER_SECOND)
    } else {
        format!("{total_millis}ms")
    }
}

/// Build the watch-mode footer line content (without the leading space the
/// frame adds): `cache <size> / <max> · <window> window`.
///
/// Stub: returns an empty string until the behavior is implemented.
#[allow(
    dead_code,
    reason = "consumed by build_watch, wired into the watch loop in a later slice"
)]
pub(crate) fn build_footer(_cache_size: u64, _max_cache_size: u64, _window: Duration) -> String {
    String::new()
}

/// One display row: a label and its already-formatted value string.
pub(crate) struct Row {
    pub label: String,
    pub value: String,
}

/// Build the one-shot human frame for `rows`, with `languages_label` in the
/// header (e.g. "Rust" or "all") and `clock` (e.g. "12:34:56") right-justified
/// against `width`. Labels are left-aligned in a shared column; values are
/// right-aligned in a shared column. Display widths come from `unicode-width`,
/// so multi-byte labels stay aligned.
pub(crate) fn build_human(
    languages_label: &str,
    clock: &str,
    width: usize,
    rows: &[Row],
) -> String {
    let left = format!("sccache · {languages_label}");
    let header_pad = width
        .saturating_sub(UnicodeWidthStr::width(left.as_str()) + UnicodeWidthStr::width(clock))
        .max(1);
    let header = format!("{left}{}{clock}", " ".repeat(header_pad));

    let max_label = rows
        .iter()
        .map(|r| UnicodeWidthStr::width(r.label.as_str()))
        .max()
        .unwrap_or(0);
    let max_value = rows
        .iter()
        .map(|r| UnicodeWidthStr::width(r.value.as_str()))
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    out.push_str(&header);
    out.push('\n');
    out.push('\n');
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let label_pad = max_label.saturating_sub(UnicodeWidthStr::width(row.label.as_str()));
        let value_pad = max_value.saturating_sub(UnicodeWidthStr::width(row.value.as_str()));
        out.push(' ');
        out.push_str(&row.label);
        out.push_str(&" ".repeat(label_pad));
        out.push_str("  ");
        out.push_str(&" ".repeat(value_pad));
        out.push_str(&row.value);
    }
    out
}

/// A JSON number for one field of the one-shot JSON report.
pub(crate) enum JsonValue {
    /// An integer count or byte size, emitted as a JSON integer.
    Int(u64),
    /// A rate/percentage, emitted as a JSON floating-point number.
    Float(f64),
}

/// One key/value field of the one-shot JSON object, in display order.
pub(crate) struct JsonField {
    pub key: &'static str,
    pub value: JsonValue,
}

/// Serialize `fields` as a compact single-line JSON object, preserving the
/// given order (NOT sorted). Suitable for piping into `jq`.
pub(crate) fn build_json(fields: &[JsonField]) -> String {
    /// Serializes a slice of fields as a JSON map in slice order. `serialize_map`
    /// preserves feed order, so keys are emitted exactly as given (not sorted).
    struct OrderedMap<'a>(&'a [JsonField]);

    impl serde::Serialize for OrderedMap<'_> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            let mut map = serializer.serialize_map(Some(self.0.len()))?;
            for field in self.0 {
                match field.value {
                    JsonValue::Int(n) => map.serialize_entry(field.key, &n)?,
                    JsonValue::Float(x) => map.serialize_entry(field.key, &x)?,
                }
            }
            map.end()
        }
    }

    serde_json::to_string(&OrderedMap(fields)).expect("serializing finite JSON numbers cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Row> {
        vec![
            Row {
                label: "Compile requests".into(),
                value: "4,786".into(),
            },
            Row {
                label: "Requests executed".into(),
                value: "3,880".into(),
            },
            Row {
                label: "Cache hits".into(),
                value: "1,718".into(),
            },
            Row {
                label: "Cache misses".into(),
                value: "963".into(),
            },
            Row {
                label: "Hit rate".into(),
                value: "64.1%".into(),
            },
        ]
    }

    #[test]
    fn header_has_label_left_and_clock_right_at_width() {
        let out = build_human("Rust", "12:34:56", 40, &rows());
        let header = out.lines().next().unwrap();
        assert!(header.starts_with("sccache · Rust"));
        assert!(header.ends_with("12:34:56"));
        assert_eq!(UnicodeWidthStr::width(header), 40);
    }

    #[test]
    fn blank_line_after_header() {
        let out = build_human("Rust", "12:34:56", 40, &rows());
        assert_eq!(out.lines().nth(1), Some(""));
    }

    #[test]
    fn rows_in_order_with_right_aligned_values() {
        let out = build_human("Rust", "12:34:56", 40, &rows());
        let data: Vec<&str> = out.lines().skip(2).collect();
        assert_eq!(data.len(), 5);
        let expected_labels = [
            "Compile requests",
            "Requests executed",
            "Cache hits",
            "Cache misses",
            "Hit rate",
        ];
        let expected_values = ["4,786", "3,880", "1,718", "963", "64.1%"];
        for (line, (lbl, val)) in data
            .iter()
            .zip(expected_labels.iter().zip(expected_values.iter()))
        {
            let body = line
                .strip_prefix(' ')
                .expect("each data line starts with one leading space");
            assert!(
                body.starts_with(lbl),
                "line {line:?} should start with label {lbl}"
            );
            assert!(
                line.ends_with(val),
                "line {line:?} should end with value {val}"
            );
        }
        // Right alignment ⇒ all data lines share the same display width.
        let widths: Vec<usize> = data.iter().map(|l| UnicodeWidthStr::width(*l)).collect();
        assert!(
            widths.iter().all(|w| *w == widths[0]),
            "data rows not equal width: {widths:?}"
        );
    }

    #[test]
    fn multibyte_labels_keep_alignment() {
        // "日本語" has display width 6 (3 wide CJK chars), "café" width 4.
        // Byte-length padding would misalign these; unicode-width keeps them even.
        let rows = vec![
            Row {
                label: "日本語".into(),
                value: "1".into(),
            },
            Row {
                label: "café".into(),
                value: "22".into(),
            },
            Row {
                label: "x".into(),
                value: "333".into(),
            },
        ];
        let out = build_human("all", "00:00:00", 30, &rows);
        let data: Vec<&str> = out.lines().skip(2).collect();
        assert_eq!(data.len(), 3);
        let widths: Vec<usize> = data.iter().map(|l| UnicodeWidthStr::width(*l)).collect();
        assert!(
            widths.iter().all(|w| *w == widths[0]),
            "multibyte rows misaligned: {widths:?}"
        );
        assert!(data[0].ends_with('1'));
        assert!(data[1].ends_with("22"));
        assert!(data[2].ends_with("333"));
    }

    #[test]
    fn no_panic_on_empty_or_single_row() {
        let _ = build_human("Rust", "12:00:00", 20, &[]);
        let single = vec![Row {
            label: "Only".into(),
            value: "1".into(),
        }];
        let out = build_human("Rust", "12:00:00", 20, &single);
        assert!(out.lines().nth(2).unwrap().ends_with('1'));
    }

    #[test]
    fn format_window_uses_largest_exact_unit() {
        // 900 s is an exact 15 minutes -> "15m", not "900s".
        assert_eq!(format_window(Duration::from_secs(900)), "15m");
        // 3600 s is an exact hour -> "1h".
        assert_eq!(format_window(Duration::from_secs(3600)), "1h");
        // 5400 s is 90 exact minutes but not a whole hour -> "90m".
        assert_eq!(format_window(Duration::from_secs(5400)), "90m");
    }

    #[test]
    fn format_window_falls_back_to_seconds_when_not_whole_minutes() {
        // 90 s is not a whole number of minutes -> "90s", never "1.5m".
        assert_eq!(format_window(Duration::from_secs(90)), "90s");
    }

    #[test]
    fn format_window_sub_second_uses_milliseconds() {
        assert_eq!(format_window(Duration::from_millis(500)), "500ms");
    }

    #[test]
    fn format_window_zero_is_zero_seconds() {
        assert_eq!(format_window(Duration::from_secs(0)), "0s");
    }

    #[test]
    fn build_footer_uses_shared_size_formatter_and_window() {
        // Fixture values: cache_size 809_212_237, max_cache_size 10_737_418_240,
        // window 15m. The sizes must match the shared size formatter byte-for-byte
        // (computed here, never hardcoded), and the window uses format_window.
        let size = crate::aggregate::format_size(809_212_237);
        let max = crate::aggregate::format_size(10_737_418_240);
        let expected = format!("cache {size} / {max} · 15m window");
        assert_eq!(
            build_footer(809_212_237, 10_737_418_240, Duration::from_secs(900)),
            expected
        );
    }

    #[test]
    fn build_footer_window_unit_follows_format_window() {
        // The window segment is whatever format_window produces, e.g. "1h".
        let size = crate::aggregate::format_size(0);
        let max = crate::aggregate::format_size(0);
        let expected = format!("cache {size} / {max} · 1h window");
        assert_eq!(build_footer(0, 0, Duration::from_secs(3600)), expected);
    }

    #[test]
    fn build_json_empty_is_braces() {
        assert_eq!(build_json(&[]), "{}");
    }

    #[test]
    fn build_json_preserves_order_and_types() {
        let fields = [
            JsonField {
                key: "compile_requests",
                value: JsonValue::Int(4786),
            },
            JsonField {
                key: "requests_executed",
                value: JsonValue::Int(3880),
            },
            JsonField {
                key: "cache_hits",
                value: JsonValue::Int(1718),
            },
            JsonField {
                key: "hit_rate",
                value: JsonValue::Float(64.08),
            },
        ];
        assert_eq!(
            build_json(&fields),
            r#"{"compile_requests":4786,"requests_executed":3880,"cache_hits":1718,"hit_rate":64.08}"#
        );
    }

    #[test]
    fn build_json_is_valid_parseable_json() {
        let fields = [
            JsonField {
                key: "cache_hits",
                value: JsonValue::Int(1718),
            },
            JsonField {
                key: "hit_rate",
                value: JsonValue::Float(64.08),
            },
        ];
        let v: serde_json::Value =
            serde_json::from_str(&build_json(&fields)).expect("build_json must emit valid JSON");
        assert_eq!(v["cache_hits"], 1718);
        assert!((v["hit_rate"].as_f64().unwrap() - 64.08).abs() < 1e-9);
    }

    #[test]
    fn build_json_float_keeps_decimal() {
        let zero = [JsonField {
            key: "x",
            value: JsonValue::Float(0.0),
        }];
        assert_eq!(build_json(&zero), r#"{"x":0.0}"#);

        let hundred = [JsonField {
            key: "x",
            value: JsonValue::Float(100.0),
        }];
        assert_eq!(build_json(&hundred), r#"{"x":100.0}"#);
    }

    #[test]
    fn build_json_single_int_field() {
        let fields = [JsonField {
            key: "k",
            value: JsonValue::Int(42),
        }];
        assert_eq!(build_json(&fields), r#"{"k":42}"#);
    }
}
