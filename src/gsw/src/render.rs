//! Assemble a [`Snapshot`] into a colored, compact text block.

use std::time::Duration;

use colored::{ColoredString, Colorize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::age::{age_dim_level, format_age, format_age_detailed, AgeDim};
use crate::bar::render_bar;
use crate::git::FileStatus;

/// Everything render() needs to draw one frame.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub branch: String,
    pub base: String,
    pub commits_ahead: u32,
    pub last_commit_age: Duration,
    pub files: Vec<RenderEntry>,
}

/// One file row in the frame.
#[derive(Debug, Clone)]
pub struct RenderEntry {
    pub path: String,
    /// Pre-rename path, when applicable.
    pub orig_path: Option<String>,
    pub status: FileStatus,
    pub staged: bool,
    pub adds: u32,
    pub dels: u32,
    pub binary: bool,
    /// File mtime age. `None` for deleted files / untracked dirs we won't stat.
    pub age: Option<Duration>,
}

/// Options driving layout.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub terminal_width: usize,
    pub bar_width: usize,
    pub max_files: Option<usize>,
}

/// Width of the "+adds" / "-dels" / age column fields.
const ADDS_FIELD: usize = 5;
const DELS_FIELD: usize = 4;
const AGE_FIELD: usize = 4;

/// Visible separator characters between columns.
const SEP_PATH_BAR: usize = 0;
const SEP_BAR_ADDS: usize = 2;
const SEP_ADDS_DELS: usize = 1;
const SEP_DELS_AGE: usize = 3;

/// Produce the colored, multi-line frame.
pub fn render(snapshot: &Snapshot, opts: &RenderOptions) -> String {
    let mut lines = Vec::new();

    lines.push(render_header(snapshot));
    lines.push(render_separator(opts.terminal_width));

    let display_count = opts
        .max_files
        .unwrap_or(usize::MAX)
        .min(snapshot.files.len());
    let max_change = snapshot
        .files
        .iter()
        .map(|e| e.adds.saturating_add(e.dels))
        .max()
        .unwrap_or(0)
        .max(1);
    let path_width = compute_path_width(opts);

    for entry in snapshot.files.iter().take(display_count) {
        lines.push(render_row(entry, opts, max_change, path_width));
    }

    let hidden = snapshot.files.len().saturating_sub(display_count);
    if hidden > 0 {
        lines.push(
            format!("  +{hidden} more file{}", if hidden == 1 { "" } else { "s" })
                .dimmed()
                .to_string(),
        );
    }

    lines.join("\n")
}

fn compute_path_width(opts: &RenderOptions) -> usize {
    // Layout overhead per row: icon(1) + letter(1) + " "(1) + bar(N) + seps + fields.
    let overhead = 3
        + opts.bar_width
        + SEP_PATH_BAR
        + SEP_BAR_ADDS
        + ADDS_FIELD
        + SEP_ADDS_DELS
        + DELS_FIELD
        + SEP_DELS_AGE
        + AGE_FIELD;
    opts.terminal_width.saturating_sub(overhead).max(8)
}

fn render_header(snap: &Snapshot) -> String {
    let commit_word = if snap.commits_ahead == 1 { "commit" } else { "commits" };
    let age = format_age_detailed(snap.last_commit_age);
    let header = format!(
        "gsw • {branch} • {n} {word} ahead of {base} • last commit {age} ago",
        branch = snap.branch,
        n = snap.commits_ahead,
        word = commit_word,
        base = snap.base,
        age = age,
    );
    header.bold().to_string()
}

fn render_separator(width: usize) -> String {
    "─".repeat(width).dimmed().to_string()
}

fn render_row(
    entry: &RenderEntry,
    opts: &RenderOptions,
    max_change: u32,
    path_width: usize,
) -> String {
    let (icon, letter) = icon_and_letter(entry);

    let path_display_raw = match &entry.orig_path {
        Some(orig) => format!("{orig} → {new}", new = entry.path),
        None => entry.path.clone(),
    };
    let path_truncated = truncate_left(&path_display_raw, path_width);
    let path_padded = pad_right(&path_truncated, path_width);

    let icon_str = colorize_icon(icon, entry);
    let letter_str = colorize_letter(letter, entry);
    let path_str = colorize_path(&path_padded, entry);

    // Untracked files get a stripped-down row — no bar, no counts — but
    // pad the gutter where bar/adds/dels would be so the age column still
    // lines up with normal rows above and below it.
    if matches!(
        entry.status,
        FileStatus::Untracked | FileStatus::UntrackedDir
    ) {
        let gutter_width = opts.bar_width
            + SEP_BAR_ADDS
            + ADDS_FIELD
            + SEP_ADDS_DELS
            + DELS_FIELD
            + SEP_DELS_AGE;
        let gutter = " ".repeat(gutter_width);
        let age = entry.age.map(format_age).unwrap_or_default();
        let age_field = format!("{age:>width$}", width = AGE_FIELD);
        let age_str = colorize_age(&age_field, entry.age);
        return format!("{icon_str}{letter_str} {path_str}{gutter}{age_str}");
    }

    let bar_raw = if entry.binary {
        center("bin", opts.bar_width)
    } else {
        render_bar(entry.adds.saturating_add(entry.dels), max_change, opts.bar_width)
    };
    let bar_str = colorize_bar(&bar_raw, entry);

    let adds_raw = if entry.adds > 0 {
        format!("+{}", entry.adds)
    } else {
        String::new()
    };
    let dels_raw = if entry.dels > 0 {
        format!("-{}", entry.dels)
    } else {
        String::new()
    };
    let adds_field = format!("{adds_raw:>width$}", width = ADDS_FIELD);
    let dels_field = format!("{dels_raw:>width$}", width = DELS_FIELD);

    let adds_str = if entry.adds > 0 {
        adds_field.green().to_string()
    } else {
        adds_field
    };
    let dels_str = if entry.dels > 0 {
        dels_field.red().to_string()
    } else {
        dels_field
    };

    let age_raw = entry.age.map(format_age).unwrap_or_else(|| String::from("—"));
    let age_field = format!("{age_raw:>width$}", width = AGE_FIELD);
    let age_str = colorize_age(&age_field, entry.age);

    let sep_bar_adds = " ".repeat(SEP_BAR_ADDS);
    let sep_adds_dels = " ".repeat(SEP_ADDS_DELS);
    let sep_dels_age = " ".repeat(SEP_DELS_AGE);

    format!(
        "{icon_str}{letter_str} {path_str}{bar_str}{sep_bar_adds}{adds_str}{sep_adds_dels}{dels_str}{sep_dels_age}{age_str}",
    )
}

fn icon_and_letter(entry: &RenderEntry) -> (char, char) {
    let icon = match entry.status {
        FileStatus::Conflicted => '!',
        FileStatus::Untracked | FileStatus::UntrackedDir => '?',
        _ if entry.staged => '●',
        _ => '○',
    };
    let letter = match entry.status {
        FileStatus::Modified => 'M',
        FileStatus::Added => 'A',
        FileStatus::Deleted => 'D',
        FileStatus::Renamed => 'R',
        FileStatus::Copied => 'C',
        FileStatus::TypeChange => 'T',
        FileStatus::Untracked | FileStatus::UntrackedDir => '?',
        FileStatus::Conflicted => 'U',
    };
    (icon, letter)
}

fn colorize_icon(icon: char, entry: &RenderEntry) -> ColoredString {
    let s = icon.to_string();
    match entry.status {
        FileStatus::Conflicted => s.red().bold(),
        FileStatus::Untracked | FileStatus::UntrackedDir => s.cyan().dimmed(),
        _ if entry.staged => s.green(),
        _ => s.yellow(),
    }
}

fn colorize_letter(letter: char, entry: &RenderEntry) -> ColoredString {
    let s = letter.to_string();
    match entry.status {
        FileStatus::Conflicted => s.red().bold(),
        FileStatus::Untracked | FileStatus::UntrackedDir => s.cyan().dimmed(),
        FileStatus::Added => s.green().bold(),
        FileStatus::Deleted => s.red().bold(),
        FileStatus::Renamed | FileStatus::Copied => s.magenta().bold(),
        _ => s.bold(),
    }
}

fn colorize_path(path: &str, entry: &RenderEntry) -> ColoredString {
    match entry.status {
        FileStatus::Conflicted => path.red(),
        FileStatus::Untracked | FileStatus::UntrackedDir => path.cyan().dimmed(),
        _ if entry.staged => path.normal().dimmed(),
        _ => path.yellow(),
    }
}

fn colorize_bar(bar: &str, entry: &RenderEntry) -> ColoredString {
    if entry.binary {
        bar.dimmed()
    } else if matches!(entry.status, FileStatus::Conflicted) {
        bar.red()
    } else {
        bar.cyan()
    }
}

fn colorize_age(text: &str, age: Option<Duration>) -> ColoredString {
    let Some(age) = age else {
        return text.dimmed();
    };
    match age_dim_level(age) {
        AgeDim::Fresh => text.bold(),
        AgeDim::Recent => text.normal(),
        AgeDim::Aging => text.dimmed(),
        AgeDim::Stale => text.dimmed(),
    }
}

/// Pad `s` on the right with spaces until its display width reaches `width`.
fn pad_right(s: &str, width: usize) -> String {
    let current = UnicodeWidthStr::width(s);
    if current >= width {
        s.to_string()
    } else {
        let mut result = String::with_capacity(s.len() + (width - current));
        result.push_str(s);
        for _ in 0..(width - current) {
            result.push(' ');
        }
        result
    }
}

/// Truncate `s` from the left to fit within `max_width` display columns,
/// prefixing with `…` when truncation happens. UTF-8 safe.
fn truncate_left(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    let ellipsis_width = 1_usize;
    let target = max_width.saturating_sub(ellipsis_width);
    let chars: Vec<char> = s.chars().collect();
    let mut acc = 0_usize;
    let mut start = chars.len();
    for (i, c) in chars.iter().enumerate().rev() {
        let cw = UnicodeWidthChar::width(*c).unwrap_or(0);
        if acc + cw > target {
            break;
        }
        acc += cw;
        start = i;
    }
    let mut result = String::from("…");
    for c in &chars[start..] {
        result.push(*c);
    }
    result
}

/// Center `text` within `width` display columns, padding with spaces.
fn center(text: &str, width: usize) -> String {
    let text_w = UnicodeWidthStr::width(text);
    if text_w >= width {
        return text.to_string();
    }
    let total_pad = width - text_w;
    let left = total_pad / 2;
    let right = total_pad - left;
    let mut result = String::with_capacity(width);
    for _ in 0..left {
        result.push(' ');
    }
    result.push_str(text);
    for _ in 0..right {
        result.push(' ');
    }
    result
}

/// Pick a sensible default file-row limit for the given terminal height.
///
/// Reserves space for our own header, separator, and a potential `+N more`
/// footer (plus one row of breathing room for whatever shell/viddy chrome
/// sits above us). Always returns at least 1.
pub fn default_max_files(terminal_height: u16) -> usize {
    usize::from(terminal_height.saturating_sub(4)).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drop ANSI CSI sequences so tests can match on the visible glyphs.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut in_escape = false;
        for c in s.chars() {
            if in_escape {
                if (0x40..=0x7E).contains(&(c as u32)) {
                    in_escape = false;
                }
                continue;
            }
            if c == '\x1b' {
                in_escape = true;
                continue;
            }
            out.push(c);
        }
        out
    }

    fn opts() -> RenderOptions {
        RenderOptions {
            terminal_width: 80,
            bar_width: 6,
            max_files: None,
        }
    }

    fn snap_with(files: Vec<RenderEntry>) -> Snapshot {
        Snapshot {
            branch: "gsv".into(),
            base: "main".into(),
            commits_ahead: 3,
            last_commit_age: Duration::from_secs(5 * 60 + 23),
            files,
        }
    }

    fn entry(
        path: &str,
        status: FileStatus,
        staged: bool,
        adds: u32,
        dels: u32,
    ) -> RenderEntry {
        RenderEntry {
            path: path.into(),
            orig_path: None,
            status,
            staged,
            adds,
            dels,
            binary: false,
            age: Some(Duration::from_secs(30)),
        }
    }

    #[test]
    fn header_mentions_branch_commits_and_age() {
        let out = strip_ansi(&render(&snap_with(vec![]), &opts()));
        let header_line = out.lines().next().unwrap_or("");
        assert!(
            header_line.contains("gsv"),
            "header should mention branch: {header_line}",
        );
        assert!(
            header_line.contains("3 commits ahead of main"),
            "header should mention commit count and base: {header_line}",
        );
        assert!(
            header_line.contains("5m23s"),
            "header should mention last-commit age: {header_line}",
        );
    }

    #[test]
    fn single_commit_uses_singular_word() {
        let mut s = snap_with(vec![]);
        s.commits_ahead = 1;
        let out = strip_ansi(&render(&s, &opts()));
        let header_line = out.lines().next().unwrap_or("");
        assert!(
            header_line.contains("1 commit ahead of main"),
            "expected singular 'commit': {header_line}",
        );
    }

    #[test]
    fn staged_modified_file_renders_full_row() {
        let snap = snap_with(vec![entry("src/foo.rs", FileStatus::Modified, true, 12, 3)]);
        let out = strip_ansi(&render(&snap, &opts()));
        let row = out.lines().nth(2).unwrap_or("");
        assert!(row.contains("src/foo.rs"), "row should include path: {row}");
        assert!(row.contains("+12"), "row should include +adds: {row}");
        assert!(row.contains("-3"), "row should include -dels: {row}");
        assert!(row.contains("30s"), "row should include age: {row}");
        assert!(row.contains('●'), "row should use ● for staged: {row}");
        assert!(row.contains('M'), "row should include status letter: {row}");
    }

    #[test]
    fn unstaged_file_uses_open_circle_icon() {
        let snap = snap_with(vec![entry("a.rs", FileStatus::Modified, false, 1, 0)]);
        let out = strip_ansi(&render(&snap, &opts()));
        let row = out.lines().nth(2).unwrap_or("");
        assert!(row.contains('○'), "row should use ○ for unstaged: {row}");
    }

    #[test]
    fn untracked_file_uses_question_icon_and_no_bar() {
        let mut e = entry("scratch.txt", FileStatus::Untracked, false, 0, 0);
        e.age = Some(Duration::from_secs(10));
        let snap = snap_with(vec![e]);
        let out = strip_ansi(&render(&snap, &opts()));
        let row = out.lines().nth(2).unwrap_or("");
        assert!(row.contains('?'), "untracked should use ? icon: {row}");
        assert!(row.contains("scratch.txt"));
        assert!(
            !row.contains('█') && !row.contains('░'),
            "untracked row should not include a bar: {row}",
        );
    }

    #[test]
    fn untracked_row_aligns_age_column_with_normal_rows() {
        let modified = entry("file.rs", FileStatus::Modified, false, 5, 2);
        let mut untracked = entry("scratch.txt", FileStatus::Untracked, false, 0, 0);
        untracked.age = Some(Duration::from_secs(30));
        let snap = snap_with(vec![modified, untracked]);
        let out = strip_ansi(&render(&snap, &opts()));
        let mod_row = out
            .lines()
            .find(|l| l.contains("file.rs"))
            .expect("modified row");
        let untr_row = out
            .lines()
            .find(|l| l.contains("scratch.txt"))
            .expect("untracked row");
        assert_eq!(
            UnicodeWidthStr::width(mod_row),
            UnicodeWidthStr::width(untr_row),
            "untracked row should pad to same total width so age columns align:\n  mod:   {mod_row:?}\n  untr:  {untr_row:?}",
        );
    }

    #[test]
    fn untracked_directory_keeps_trailing_slash() {
        let snap = snap_with(vec![entry(
            "new-folder/",
            FileStatus::UntrackedDir,
            false,
            0,
            0,
        )]);
        let out = strip_ansi(&render(&snap, &opts()));
        let row = out.lines().nth(2).unwrap_or("");
        assert!(
            row.contains("new-folder/"),
            "untracked dir should show trailing slash: {row}",
        );
    }

    #[test]
    fn binary_file_replaces_bar_with_bin_marker() {
        let mut e = entry("assets/logo.png", FileStatus::Modified, true, 0, 0);
        e.binary = true;
        let snap = snap_with(vec![e]);
        let out = strip_ansi(&render(&snap, &opts()));
        let row = out.lines().nth(2).unwrap_or("");
        assert!(row.contains("bin"), "binary file should show 'bin': {row}");
        assert!(
            !row.contains('█'),
            "binary file should not show bar fill: {row}",
        );
    }

    #[test]
    fn renamed_file_shows_old_arrow_new_on_one_line() {
        let mut e = entry("src/new.rs", FileStatus::Renamed, true, 5, 2);
        e.orig_path = Some("src/old.rs".into());
        let snap = snap_with(vec![e]);
        let out = strip_ansi(&render(&snap, &opts()));
        let row = out.lines().nth(2).unwrap_or("");
        assert!(row.contains("src/old.rs"), "rename should show old path: {row}");
        assert!(row.contains("src/new.rs"), "rename should show new path: {row}");
        assert!(row.contains('→'), "rename should use arrow: {row}");
    }

    #[test]
    fn conflict_uses_bang_icon() {
        let snap = snap_with(vec![entry(
            "src/clash.rs",
            FileStatus::Conflicted,
            false,
            5,
            5,
        )]);
        let out = strip_ansi(&render(&snap, &opts()));
        let row = out.lines().nth(2).unwrap_or("");
        assert!(row.contains('!'), "conflict should use ! icon: {row}");
    }

    #[test]
    fn long_path_truncated_from_left_with_ellipsis() {
        let long_path = format!("{}/end.rs", "a/very/long/path".repeat(10));
        let snap = snap_with(vec![entry(&long_path, FileStatus::Modified, true, 1, 0)]);
        let out = strip_ansi(&render(
            &snap,
            &RenderOptions {
                terminal_width: 60,
                bar_width: 6,
                max_files: None,
            },
        ));
        let row = out.lines().nth(2).unwrap_or("");
        assert!(row.contains('…'), "long path should be truncated: {row}");
        assert!(
            row.contains("end.rs"),
            "filename end should be preserved: {row}",
        );
    }

    #[test]
    fn utf8_path_truncation_does_not_panic() {
        let path = "日本語/とても/長い/ファイル名.rs".to_string();
        let snap = snap_with(vec![entry(&path, FileStatus::Modified, true, 1, 0)]);
        let out = render(
            &snap,
            &RenderOptions {
                terminal_width: 40,
                bar_width: 6,
                max_files: None,
            },
        );
        let stripped = strip_ansi(&out);
        assert!(stripped.contains(".rs"));
    }

    #[test]
    fn max_files_truncates_and_shows_more_footer() {
        let files: Vec<RenderEntry> = (0..10)
            .map(|i| entry(&format!("f{i}.rs"), FileStatus::Modified, true, 1, 0))
            .collect();
        let snap = snap_with(files);
        let out = strip_ansi(&render(
            &snap,
            &RenderOptions {
                terminal_width: 80,
                bar_width: 6,
                max_files: Some(3),
            },
        ));
        assert!(out.contains("f0.rs"));
        assert!(out.contains("f2.rs"));
        assert!(!out.contains("f3.rs"), "files past the limit should be hidden");
        assert!(
            out.contains("7 more"),
            "footer should report hidden count: {out}",
        );
    }

    #[test]
    fn bar_scales_to_max_change_in_snapshot() {
        let big = entry("big.rs", FileStatus::Modified, true, 100, 0);
        let small = entry("small.rs", FileStatus::Modified, true, 1, 0);
        let snap = snap_with(vec![big, small]);
        let out = strip_ansi(&render(&snap, &opts()));
        let big_row = out
            .lines()
            .find(|l| l.contains("big.rs"))
            .unwrap_or_default();
        let small_row = out
            .lines()
            .find(|l| l.contains("small.rs"))
            .unwrap_or_default();
        let big_filled = big_row.matches('█').count();
        let small_filled = small_row.matches('█').count();
        assert!(
            big_filled > small_filled,
            "big-change row should have more full cells than small: big={big_filled}, small={small_filled}",
        );
        assert!(big_filled >= 6, "biggest change should fill the bar: {big_row}");
    }

    #[test]
    fn default_max_files_reserves_room_for_chrome() {
        // Reserve 4 rows for header, separator, footer, and breathing room.
        assert_eq!(default_max_files(24), 20);
        assert_eq!(default_max_files(50), 46);
        assert_eq!(default_max_files(10), 6);
    }

    #[test]
    fn default_max_files_never_returns_zero() {
        assert_eq!(default_max_files(0), 1);
        assert_eq!(default_max_files(1), 1);
        assert_eq!(default_max_files(4), 1);
        assert_eq!(default_max_files(5), 1);
    }

}
