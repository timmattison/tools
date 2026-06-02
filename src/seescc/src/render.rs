//! Pure rendering of the one-shot human view: a header line (label + injected
//! wall-clock) and a right-aligned table of metric rows. Time is injected as a
//! preformatted string so this module stays pure and deterministic under test.

use unicode_width::UnicodeWidthStr;

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
            assert!(body.starts_with(lbl), "line {line:?} should start with label {lbl}");
            assert!(line.ends_with(val), "line {line:?} should end with value {val}");
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
}
