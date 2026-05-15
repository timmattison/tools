//! Assemble a [`Snapshot`] into a colored, compact text block.

use std::time::Duration;

use colored::{ColoredString, Colorize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::age::{age_dim_level, format_age_detailed, AgeDim};
use crate::bar::render_bar;
use crate::git::FileStatus;

/// Everything render() needs to draw one frame.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub branch: String,
    pub base: String,
    pub commits_ahead: u32,
    pub last_commit_age: Option<Duration>,
    pub files: Vec<RenderEntry>,
    /// Most recent commits, newest first. Empty when not requested.
    pub log: Vec<LogEntry>,
}

/// One recent-commit row.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub hash: String,
    pub subject: String,
    /// How long ago this commit was authored. Same `Duration` shape as the
    /// per-file age column so the two render in identical units.
    pub age: Duration,
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
    /// Maximum recent-commit rows to render. 0 disables the log section.
    pub log_lines: usize,
}

/// Width of the "+adds" / "-dels" / age column fields.
const ADDS_FIELD: usize = 5;
const DELS_FIELD: usize = 4;
/// Wide enough for `59m59s`, `23h59m`, `99d23h`. Older files overflow slightly.
const AGE_FIELD: usize = 6;

/// Visible separator characters between columns.
const SEP_BAR_ADDS: usize = 2;
const SEP_ADDS_DELS: usize = 1;
const SEP_DELS_AGE: usize = 3;

/// Produce the colored, multi-line frame.
pub fn render(snapshot: &Snapshot, opts: &RenderOptions) -> String {
    let mut lines = Vec::new();

    let header_plain = header_text(snapshot);
    let header_width = UnicodeWidthStr::width(header_plain.as_str());
    lines.push(header_plain.bold().to_string());
    lines.push(render_separator(header_width));

    let display_count = match opts.max_files {
        Some(0) | None => snapshot.files.len(),
        Some(n) => n.min(snapshot.files.len()),
    };
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

    if opts.log_lines > 0 && !snapshot.log.is_empty() {
        // When there are no file rows above us, the post-header separator
        // already sits directly above the log section; adding another would
        // produce a double rule with nothing between them.
        if !snapshot.files.is_empty() {
            lines.push(render_separator(header_width));
        }
        for entry in snapshot.log.iter().take(opts.log_lines) {
            lines.push(render_log_row(entry, opts.terminal_width));
        }
    }

    lines.join("\n")
}

fn render_log_row(entry: &LogEntry, width: usize) -> String {
    // Layout: `{hash}  {subject…}   {age}` — the rightmost AGE_FIELD cells
    // hold the right-aligned age, matching the file-row age column exactly.
    // The subject is padded to fill the gap so the age column lines up.
    let hash_width = UnicodeWidthStr::width(entry.hash.as_str());
    let hash_sep = "  ";
    let hash_sep_width = hash_sep.chars().count();
    let sep_to_age = " ".repeat(SEP_DELS_AGE);

    let subject_budget = width
        .saturating_sub(hash_width + hash_sep_width + SEP_DELS_AGE + AGE_FIELD)
        .max(1);
    let subject_truncated = truncate_right(&entry.subject, subject_budget);
    let subject_padded = pad_right(&subject_truncated, subject_budget);

    let age_raw = format_age_detailed(entry.age);
    let age_field = format!("{age_raw:>width$}", width = AGE_FIELD);
    let age_str = colorize_age(&age_field, Some(entry.age));

    let hash_str = entry.hash.yellow().to_string();
    format!("{hash_str}{hash_sep}{subject_padded}{sep_to_age}{age_str}")
}

/// Total width of everything to the right of the path column: the bar plus
/// the +adds/-dels/age fields and their separators. Used both for sizing the
/// path column and for padding the gutter on untracked rows that skip the
/// bar/adds/dels so the age column still aligns.
fn right_block_width(bar_width: usize) -> usize {
    bar_width
        + SEP_BAR_ADDS
        + ADDS_FIELD
        + SEP_ADDS_DELS
        + DELS_FIELD
        + SEP_DELS_AGE
        + AGE_FIELD
}

fn compute_path_width(opts: &RenderOptions) -> usize {
    // Per-row overhead: icon(1) + " "(1) + letter(1) + " "(1) + right block.
    let overhead = 4 + right_block_width(opts.bar_width);
    opts.terminal_width.saturating_sub(overhead).max(8)
}

fn header_text(snap: &Snapshot) -> String {
    let commit_word = if snap.commits_ahead == 1 { "commit" } else { "commits" };
    let age = snap
        .last_commit_age
        .map_or_else(|| "?".to_string(), format_age_detailed);
    format!(
        "gsw • {branch} • {n} {word} ahead of {base} • last commit {age} ago",
        branch = snap.branch,
        n = snap.commits_ahead,
        word = commit_word,
        base = snap.base,
    )
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
        let gutter_width = right_block_width(opts.bar_width) - AGE_FIELD;
        let gutter = " ".repeat(gutter_width);
        let age = entry.age.map(format_age_detailed).unwrap_or_default();
        let age_field = format!("{age:>width$}", width = AGE_FIELD);
        let age_str = colorize_age(&age_field, entry.age);
        return format!("{icon_str} {letter_str} {path_str}{gutter}{age_str}");
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

    let age_raw = entry.age.map(format_age_detailed).unwrap_or_default();
    let age_field = format!("{age_raw:>width$}", width = AGE_FIELD);
    let age_str = colorize_age(&age_field, entry.age);

    let sep_bar_adds = " ".repeat(SEP_BAR_ADDS);
    let sep_adds_dels = " ".repeat(SEP_ADDS_DELS);
    let sep_dels_age = " ".repeat(SEP_DELS_AGE);

    format!(
        "{icon_str} {letter_str} {path_str}{bar_str}{sep_bar_adds}{adds_str}{sep_adds_dels}{dels_str}{sep_dels_age}{age_str}",
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
        AgeDim::Stale => text.dimmed().italic(),
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

/// Truncate `s` from the right to fit within `max_width` display columns,
/// suffixing with `…` when truncation happens. UTF-8 safe.
fn truncate_right(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    let ellipsis_width = 1_usize;
    let target = max_width.saturating_sub(ellipsis_width);
    let mut acc = 0_usize;
    let mut result = String::new();
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if acc + cw > target {
            break;
        }
        acc += cw;
        result.push(c);
    }
    result.push('…');
    result
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
            log_lines: 0,
        }
    }

    fn snap_with(files: Vec<RenderEntry>) -> Snapshot {
        Snapshot {
            branch: "gsv".into(),
            base: "main".into(),
            commits_ahead: 3,
            last_commit_age: Some(Duration::from_secs(5 * 60 + 23)),
            files,
            log: vec![],
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
    fn separator_matches_header_visible_width() {
        // If the separator is wider than the header line, resizing the
        // terminal smaller wraps it onto the next line. Size it to match.
        let snap = snap_with(vec![]);
        let out = strip_ansi(&render(&snap, &opts()));
        let mut lines = out.lines();
        let header = lines.next().expect("header line");
        let sep = lines.next().expect("separator line");
        assert_eq!(
            UnicodeWidthStr::width(header),
            UnicodeWidthStr::width(sep),
            "separator width should match header width\n  header: {header:?}\n  sep:    {sep:?}",
        );
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
    fn row_has_space_between_icon_and_status_letter() {
        let snap = snap_with(vec![entry("a.rs", FileStatus::Modified, true, 1, 0)]);
        let out = strip_ansi(&render(&snap, &opts()));
        let row = out.lines().nth(2).unwrap_or("");
        assert!(
            row.contains("● M "),
            "icon and letter should be separated by a space: {row}",
        );
    }

    #[test]
    fn untracked_row_has_space_between_icon_and_status_letter() {
        let snap = snap_with(vec![entry(
            "scratch.txt",
            FileStatus::Untracked,
            false,
            0,
            0,
        )]);
        let out = strip_ansi(&render(&snap, &opts()));
        let row = out.lines().nth(2).unwrap_or("");
        assert!(
            row.contains("? ? "),
            "untracked icon and letter should also be space-separated: {row}",
        );
    }

    #[test]
    fn per_file_age_uses_detailed_two_unit_format() {
        let mut e = entry("file.rs", FileStatus::Modified, true, 1, 0);
        e.age = Some(Duration::from_secs(5 * 60 + 23));
        let snap = snap_with(vec![e]);
        let out = strip_ansi(&render(&snap, &opts()));
        let row = out.lines().nth(2).unwrap_or("");
        assert!(
            row.contains("5m23s"),
            "per-file age should match the header's two-unit format: {row}",
        );
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
                log_lines: 0,
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
                log_lines: 0,
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
                log_lines: 0,
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
    fn max_files_zero_means_unlimited() {
        let files: Vec<RenderEntry> = (0..5)
            .map(|i| entry(&format!("f{i}.rs"), FileStatus::Modified, true, 1, 0))
            .collect();
        let snap = snap_with(files);
        let out = strip_ansi(&render(
            &snap,
            &RenderOptions {
                terminal_width: 80,
                bar_width: 6,
                max_files: Some(0),
                log_lines: 0,
            },
        ));
        for i in 0..5 {
            assert!(
                out.contains(&format!("f{i}.rs")),
                "every file should appear when max_files=0 means unlimited: {out}",
            );
        }
        assert!(
            !out.contains("more file"),
            "no 'more files' footer when nothing is hidden: {out}",
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
    fn missing_age_renders_as_blank_not_emdash() {
        // The em-dash placeholder visually drifts past the terminal edge in
        // some font/terminal combos (zellij + certain fonts render em-dash
        // wider than unicode-width reports). Leave the age column empty
        // when we don't have an mtime — the row already shows `D` / `?` /
        // etc. to explain why.
        let mut e = entry("deleted.rs", FileStatus::Deleted, true, 0, 5);
        e.age = None;
        let snap = snap_with(vec![e]);
        let out = strip_ansi(&render(&snap, &opts()));
        let row = out.lines().nth(2).unwrap_or("");
        assert!(
            !row.contains('\u{2014}'),
            "no-age row should not include an em-dash placeholder: {row}",
        );
    }

    #[test]
    fn default_max_files_never_returns_zero() {
        assert_eq!(default_max_files(0), 1);
        assert_eq!(default_max_files(1), 1);
        assert_eq!(default_max_files(4), 1);
        assert_eq!(default_max_files(5), 1);
    }

    fn log_entry(hash: &str, subject: &str, age_secs: u64) -> LogEntry {
        LogEntry {
            hash: hash.into(),
            subject: subject.into(),
            age: Duration::from_secs(age_secs),
        }
    }

    #[test]
    fn log_section_renders_hash_and_subject() {
        let mut snap = snap_with(vec![]);
        snap.log = vec![
            log_entry("abc1234", "first commit subject", 30),
            log_entry("def5678", "second commit subject", 60),
        ];
        let mut o = opts();
        o.log_lines = 10;
        let out = strip_ansi(&render(&snap, &o));
        assert!(out.contains("abc1234"), "first hash should appear: {out}");
        assert!(
            out.contains("first commit subject"),
            "first subject should appear: {out}",
        );
        assert!(out.contains("def5678"), "second hash should appear: {out}");
        assert!(
            out.contains("second commit subject"),
            "second subject should appear: {out}",
        );
    }

    #[test]
    fn log_section_caps_at_log_lines() {
        let mut snap = snap_with(vec![]);
        snap.log = (0..30_u64)
            .map(|i| LogEntry {
                hash: format!("h{i:06}"),
                subject: format!("subj {i}"),
                age: Duration::from_secs(i * 60),
            })
            .collect();
        let mut o = opts();
        o.log_lines = 5;
        let out = strip_ansi(&render(&snap, &o));
        assert!(out.contains("h000000"), "first entry shown: {out}");
        assert!(out.contains("h000004"), "fifth entry shown: {out}");
        assert!(
            !out.contains("h000005"),
            "sixth entry hidden by log_lines limit: {out}",
        );
    }

    #[test]
    fn log_lines_zero_disables_section() {
        let mut snap = snap_with(vec![]);
        snap.log = vec![log_entry("abc1234", "first", 0)];
        let mut o = opts();
        o.log_lines = 0;
        let out = strip_ansi(&render(&snap, &o));
        assert!(
            !out.contains("abc1234"),
            "log_lines=0 disables log section: {out}",
        );
    }

    #[test]
    fn log_row_truncates_long_subject_to_fit_terminal_width() {
        let mut snap = snap_with(vec![]);
        snap.log = vec![LogEntry {
            hash: "abc1234".into(),
            subject: "really long subject ".repeat(20),
            age: Duration::from_secs(30),
        }];
        let mut o = opts();
        o.log_lines = 1;
        o.terminal_width = 40;
        let out = strip_ansi(&render(&snap, &o));
        let log_line = out
            .lines()
            .find(|l| l.contains("abc1234"))
            .expect("log line should appear");
        let w = UnicodeWidthStr::width(log_line);
        assert!(
            w <= 40,
            "log row width {w} should not exceed terminal_width=40: {log_line:?}",
        );
        assert!(
            log_line.contains('…'),
            "truncated subject should end with ellipsis: {log_line:?}",
        );
    }

    #[test]
    fn empty_file_list_collapses_to_single_separator_before_log() {
        // When there are no staged/unstaged files, the post-header separator
        // already sits directly above the log section, so the additional
        // pre-log separator just creates a double rule with nothing between
        // them. Collapse to a single ─ line.
        let mut snap = snap_with(vec![]);
        snap.log = vec![
            log_entry("abc1234", "first", 0),
            log_entry("def5678", "second", 60),
        ];
        let mut o = opts();
        o.log_lines = 5;
        let out = strip_ansi(&render(&snap, &o));
        let separator_count = out
            .lines()
            .filter(|l| !l.is_empty() && l.chars().all(|c| c == '─'))
            .count();
        assert_eq!(
            separator_count, 1,
            "expected exactly one separator when file list is empty:\n{out}",
        );
    }

    #[test]
    fn log_row_renders_age_in_detailed_format() {
        let mut snap = snap_with(vec![]);
        snap.log = vec![log_entry("abc1234", "subject", 5 * 60 + 23)];
        let mut o = opts();
        o.log_lines = 1;
        let out = strip_ansi(&render(&snap, &o));
        let log_line = out
            .lines()
            .find(|l| l.contains("abc1234"))
            .expect("log row should appear");
        assert!(
            log_line.contains("5m23s"),
            "log row should include the commit age in the two-unit format: {log_line:?}",
        );
    }

    #[test]
    fn log_row_age_column_aligns_with_file_row_age_column() {
        // The age column on a log row should occupy the same rightmost
        // AGE_FIELD cells as the file rows, so the two stacked sections
        // align cleanly under viddy.
        let file = entry("file.rs", FileStatus::Modified, true, 1, 0);
        let mut snap = snap_with(vec![file]);
        snap.log = vec![log_entry("abc1234", "subject", 5 * 60 + 23)];
        let mut o = opts();
        o.log_lines = 1;
        let out = strip_ansi(&render(&snap, &o));
        let file_row = out
            .lines()
            .find(|l| l.contains("file.rs"))
            .expect("file row should appear");
        let log_line = out
            .lines()
            .find(|l| l.contains("abc1234"))
            .expect("log row should appear");
        assert_eq!(
            UnicodeWidthStr::width(file_row.trim_end()),
            UnicodeWidthStr::width(log_line.trim_end()),
            "file row and log row should occupy the same width so age columns align:\n  file: {file_row:?}\n  log:  {log_line:?}",
        );
    }

    #[test]
    fn header_renders_question_mark_when_last_commit_age_unknown() {
        // When git can't tell us when HEAD was authored (empty repo, malformed
        // %ct, clock skew), the header used to say "last commit 0s ago" — i.e.
        // it lied about a fresh commit. Render an explicit "?" instead so the
        // unknown state is visible.
        let mut snap = snap_with(vec![]);
        snap.last_commit_age = None;
        let out = strip_ansi(&render(&snap, &opts()));
        let header = out.lines().next().unwrap_or("");
        assert!(
            header.contains("last commit ? ago"),
            "header should mark unknown last-commit age explicitly: {header}",
        );
        assert!(
            !header.contains("0s ago"),
            "unknown age must not be misrepresented as 0s: {header}",
        );
    }

    #[test]
    fn stale_age_renders_differently_from_aging() {
        // AgeDim has four buckets; if Stale and Aging both render `.dimmed()`
        // the bucket distinction is invisible to the user. Compare the Style
        // bitsets on the returned ColoredStrings directly — avoids touching
        // `colored::control::set_override`, which is process-global and would
        // race with other tests in parallel.
        use colored::Styles;
        let aging = colorize_age("12h0m", Some(Duration::from_secs(2 * 3600)));
        let stale = colorize_age("12h0m", Some(Duration::from_secs(2 * 86400)));
        assert!(
            stale.style.contains(Styles::Italic),
            "Stale should be italicized",
        );
        assert!(
            !aging.style.contains(Styles::Italic),
            "Aging should not be italicized",
        );
    }

    #[test]
    fn log_section_has_separator_before_entries() {
        let mut snap = snap_with(vec![]);
        snap.log = vec![log_entry("abc1234", "first", 0)];
        let mut o = opts();
        o.log_lines = 5;
        let out = strip_ansi(&render(&snap, &o));
        let lines: Vec<&str> = out.lines().collect();
        let log_idx = lines
            .iter()
            .position(|l| l.contains("abc1234"))
            .expect("log row should appear");
        assert!(log_idx >= 1, "log row should be preceded by other lines");
        let preceding = lines[log_idx - 1];
        assert!(
            preceding.chars().all(|c| c == '─' || c.is_whitespace()),
            "line before log section should be a ─ separator: {preceding:?}",
        );
    }
}
