//! Assemble a [`Snapshot`] into a colored, compact text block.

use std::time::Duration;

use colored::{ColoredString, Colorize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::age::{age_dim_level, age_fade_factor, fade_rgb, format_age_detailed, AgeDim};
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
    /// Upstream tracking branch status (ahead/behind). `None` when the
    /// current branch has no configured upstream.
    pub upstream: Option<UpstreamStatus>,
}

/// State of the local branch relative to its upstream tracking ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamStatus {
    /// Short upstream name, e.g. `origin/gsw-origin`.
    pub name: String,
    /// Commits on HEAD not yet on the upstream.
    pub ahead: u32,
    /// Commits on the upstream not yet on HEAD.
    pub behind: u32,
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
    /// When true, the commit-log rows fade from a bright base color toward
    /// a dark floor as commits age, using 24-bit (truecolor) ANSI. When
    /// false, log rows use the same 8-color/dim styling as everything else.
    pub truecolor: bool,
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
            lines.push(render_log_row(entry, opts.terminal_width, opts.truecolor));
        }
    }

    lines.join("\n")
}

/// Visible gap between the short hash and the subject in a log row.
const LOG_HASH_SUBJECT_SEP: &str = "  ";

fn render_log_row(entry: &LogEntry, width: usize, truecolor: bool) -> String {
    // Layout: `{hash}  {subject…}   {age}` — the rightmost AGE_FIELD cells
    // hold the right-aligned age, matching the file-row age column exactly.
    // The subject is padded to fill the gap so the age column lines up.
    let hash_width = UnicodeWidthStr::width(entry.hash.as_str());
    let hash_sep_width = LOG_HASH_SUBJECT_SEP.chars().count();
    let sep_to_age = " ".repeat(SEP_DELS_AGE);

    let subject_budget = width
        .saturating_sub(hash_width + hash_sep_width + SEP_DELS_AGE + AGE_FIELD)
        .max(1);
    let subject_truncated = truncate_right(&entry.subject, subject_budget);
    let subject_padded = pad_right(&subject_truncated, subject_budget);

    let age_raw = format_age_detailed(entry.age);
    let age_field = format!("{age_raw:>width$}", width = AGE_FIELD);

    let hash_str = colorize_log_hash(&entry.hash, entry.age, truecolor);
    let subject_str = colorize_log_subject(&subject_padded, entry.age, truecolor);
    let age_str = colorize_log_age(&age_field, entry.age, truecolor);
    format!("{hash_str}{LOG_HASH_SUBJECT_SEP}{subject_str}{sep_to_age}{age_str}")
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
    let upstream_field = snap
        .upstream
        .as_ref()
        .map(|u| format!(" • ↑{} ↓{} {}", u.ahead, u.behind, u.name))
        .unwrap_or_default();
    format!(
        "gsw • {branch} • {n} {word} ahead of {base}{upstream_field} • last commit {age} ago",
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

    let icon_str = colorize_icon(icon, entry, 0.0, false);
    let letter_str = colorize_letter(letter, entry, 0.0, false);
    let path_str = colorize_path(&path_padded, entry, 0.0, false);

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
        let age_str = colorize_age(&age_field, entry.age, 0.0, false);
        return format!("{icon_str} {letter_str} {path_str}{gutter}{age_str}");
    }

    let bar_raw = if entry.binary {
        center("bin", opts.bar_width)
    } else {
        render_bar(entry.adds.saturating_add(entry.dels), max_change, opts.bar_width)
    };
    let bar_str = colorize_bar(&bar_raw, entry, 0.0, false);

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
        colorize_adds(&adds_field, 0.0, false).to_string()
    } else {
        adds_field
    };
    let dels_str = if entry.dels > 0 {
        colorize_dels(&dels_field, 0.0, false).to_string()
    } else {
        dels_field
    };

    let age_raw = entry.age.map(format_age_detailed).unwrap_or_default();
    let age_field = format!("{age_raw:>width$}", width = AGE_FIELD);
    let age_str = colorize_age(&age_field, entry.age, 0.0, false);

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

fn colorize_icon(
    icon: char,
    entry: &RenderEntry,
    factor: f32,
    truecolor: bool,
) -> ColoredString {
    let s = icon.to_string();
    if truecolor {
        let base = match entry.status {
            FileStatus::Conflicted => FILE_ICON_CONFLICT_RGB,
            FileStatus::Untracked | FileStatus::UntrackedDir => FILE_ICON_UNTRACKED_RGB,
            _ if entry.staged => FILE_ICON_STAGED_RGB,
            _ => FILE_ICON_UNSTAGED_RGB,
        };
        let (r, g, b) = fade_rgb(base, factor);
        return s.truecolor(r, g, b);
    }
    match entry.status {
        FileStatus::Conflicted => s.red().bold(),
        FileStatus::Untracked | FileStatus::UntrackedDir => s.cyan().dimmed(),
        _ if entry.staged => s.green(),
        _ => s.yellow(),
    }
}

fn colorize_letter(
    letter: char,
    entry: &RenderEntry,
    factor: f32,
    truecolor: bool,
) -> ColoredString {
    let s = letter.to_string();
    if truecolor {
        let base = match entry.status {
            FileStatus::Conflicted => FILE_LETTER_CONFLICT_RGB,
            FileStatus::Untracked | FileStatus::UntrackedDir => FILE_LETTER_UNTRACKED_RGB,
            FileStatus::Added => FILE_LETTER_ADDED_RGB,
            FileStatus::Deleted => FILE_LETTER_DELETED_RGB,
            FileStatus::Renamed | FileStatus::Copied => FILE_LETTER_RENAMED_RGB,
            _ => FILE_LETTER_DEFAULT_RGB,
        };
        let (r, g, b) = fade_rgb(base, factor);
        return s.truecolor(r, g, b);
    }
    match entry.status {
        FileStatus::Conflicted => s.red().bold(),
        FileStatus::Untracked | FileStatus::UntrackedDir => s.cyan().dimmed(),
        FileStatus::Added => s.green().bold(),
        FileStatus::Deleted => s.red().bold(),
        FileStatus::Renamed | FileStatus::Copied => s.magenta().bold(),
        _ => s.bold(),
    }
}

fn colorize_path(
    path: &str,
    entry: &RenderEntry,
    factor: f32,
    truecolor: bool,
) -> ColoredString {
    if truecolor {
        let base = match entry.status {
            FileStatus::Conflicted => FILE_PATH_CONFLICT_RGB,
            FileStatus::Untracked | FileStatus::UntrackedDir => FILE_PATH_UNTRACKED_RGB,
            _ if entry.staged => FILE_PATH_STAGED_RGB,
            _ => FILE_PATH_UNSTAGED_RGB,
        };
        let (r, g, b) = fade_rgb(base, factor);
        return path.truecolor(r, g, b);
    }
    match entry.status {
        FileStatus::Conflicted => path.red(),
        FileStatus::Untracked | FileStatus::UntrackedDir => path.cyan().dimmed(),
        _ if entry.staged => path.normal().dimmed(),
        _ => path.yellow(),
    }
}

/// Dim cyan used as the background under a partial-fill cell so the
/// unpainted right portion of glyphs like `▍` blends into the dim `░`
/// empty cells beside them instead of showing through as terminal black.
/// `░` is the LIGHT SHADE character (~25% pixel coverage), so this is
/// roughly a quarter of typical ANSI cyan's brightness.
const BAR_PARTIAL_BG_CYAN: (u8, u8, u8) = (0, 48, 48);
/// Same idea for the conflicted-file bar, which paints in red.
const BAR_PARTIAL_BG_RED: (u8, u8, u8) = (48, 0, 0);

/// Build one `ColoredString` per visible cell of `bar`. The joined string
/// returned by [`colorize_bar`] is just `colorize_bar_styled(...).join("")`
/// with `.to_string()` applied to each cell — sharing the cell builder lets
/// tests inspect the typed fg/bg colors per cell instead of parsing ANSI.
fn colorize_bar_styled(
    bar: &str,
    entry: &RenderEntry,
    factor: f32,
    truecolor: bool,
) -> Vec<ColoredString> {
    if entry.binary {
        return bar.chars().map(|c| c.to_string().dimmed()).collect();
    }
    let is_conflicted = matches!(entry.status, FileStatus::Conflicted);
    let (bg_br, bg_bg, bg_bb) = if is_conflicted {
        BAR_PARTIAL_BG_RED
    } else {
        BAR_PARTIAL_BG_CYAN
    };
    bar.chars()
        .map(|c| {
            let s = c.to_string();
            if truecolor {
                let fg_base = if is_conflicted {
                    FILE_BAR_CONFLICT_RGB
                } else {
                    FILE_BAR_RGB
                };
                let (fr, fg, fb) = fade_rgb(fg_base, factor);
                if is_partial_block(c) {
                    let (pr, pg, pb) = fade_rgb((bg_br, bg_bg, bg_bb), factor);
                    s.truecolor(fr, fg, fb).on_truecolor(pr, pg, pb)
                } else {
                    s.truecolor(fr, fg, fb)
                }
            } else if is_partial_block(c) {
                if is_conflicted {
                    s.red().on_truecolor(bg_br, bg_bg, bg_bb)
                } else {
                    s.cyan().on_truecolor(bg_br, bg_bg, bg_bb)
                }
            } else if is_conflicted {
                s.red()
            } else {
                s.cyan()
            }
        })
        .collect()
}

fn colorize_bar(bar: &str, entry: &RenderEntry, factor: f32, truecolor: bool) -> String {
    let cells = colorize_bar_styled(bar, entry, factor, truecolor);
    let mut out = String::with_capacity(bar.len() * 2);
    for c in cells {
        out.push_str(&c.to_string());
    }
    out
}

/// True for the eighth-block partial-fill glyphs `▏▎▍▌▋▊▉`, which only
/// paint a fraction of their cell width and need a background fill so
/// they don't leave a black gap.
fn is_partial_block(c: char) -> bool {
    matches!(c, '\u{2589}'..='\u{258F}')
}

fn colorize_age(
    text: &str,
    age: Option<Duration>,
    factor: f32,
    truecolor: bool,
) -> ColoredString {
    if truecolor {
        let (r, g, b) = fade_rgb(FILE_AGE_RGB, factor);
        return text.truecolor(r, g, b);
    }
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

/// Color the `+adds` field for a file row.
///
/// With `truecolor`, applies the age-driven fade starting from
/// [`FILE_ADDS_RGB`]. Without, falls back to ANSI green.
fn colorize_adds(text: &str, factor: f32, truecolor: bool) -> ColoredString {
    if truecolor {
        let (r, g, b) = fade_rgb(FILE_ADDS_RGB, factor);
        return text.truecolor(r, g, b);
    }
    text.green()
}

/// Color the `-dels` field for a file row.
///
/// With `truecolor`, applies the age-driven fade starting from
/// [`FILE_DELS_RGB`]. Without, falls back to ANSI red.
fn colorize_dels(text: &str, factor: f32, truecolor: bool) -> ColoredString {
    if truecolor {
        let (r, g, b) = fade_rgb(FILE_DELS_RGB, factor);
        return text.truecolor(r, g, b);
    }
    text.red()
}

/// Base RGB for commit-log hashes when truecolor fading is on. Picked to
/// match the perceptual feel of the legacy `yellow()` ANSI hash without
/// depending on a specific terminal palette.
const LOG_HASH_BASE_RGB: (u8, u8, u8) = (255, 215, 0);
/// Base RGB for commit-log subjects — a near-white that fades visibly.
const LOG_SUBJECT_BASE_RGB: (u8, u8, u8) = (220, 220, 220);
/// Base RGB for the commit-log age column.
const LOG_AGE_BASE_RGB: (u8, u8, u8) = (190, 190, 190);

// --- File-row truecolor base palette ---------------------------------------
//
// Per-status base RGB values for the file list under truecolor mode. Each
// base is tuned so factor=0 reads as the same hue family as the legacy
// ANSI color, and factor=1 (× FADE_FLOOR) still keeps the hue visible.

const FILE_PATH_UNSTAGED_RGB: (u8, u8, u8) = (220, 200, 100);
const FILE_PATH_STAGED_RGB: (u8, u8, u8) = (200, 200, 200);
const FILE_PATH_UNTRACKED_RGB: (u8, u8, u8) = (120, 200, 200);
const FILE_PATH_CONFLICT_RGB: (u8, u8, u8) = (255, 90, 90);

const FILE_ICON_STAGED_RGB: (u8, u8, u8) = (90, 220, 110);
const FILE_ICON_UNSTAGED_RGB: (u8, u8, u8) = (220, 200, 100);
const FILE_ICON_UNTRACKED_RGB: (u8, u8, u8) = (120, 200, 200);
const FILE_ICON_CONFLICT_RGB: (u8, u8, u8) = (255, 80, 80);

const FILE_LETTER_ADDED_RGB: (u8, u8, u8) = (90, 220, 110);
const FILE_LETTER_DELETED_RGB: (u8, u8, u8) = (255, 80, 80);
const FILE_LETTER_RENAMED_RGB: (u8, u8, u8) = (220, 120, 220);
const FILE_LETTER_DEFAULT_RGB: (u8, u8, u8) = (230, 230, 230);
const FILE_LETTER_CONFLICT_RGB: (u8, u8, u8) = (255, 80, 80);
const FILE_LETTER_UNTRACKED_RGB: (u8, u8, u8) = (120, 200, 200);
const FILE_AGE_RGB: (u8, u8, u8) = (190, 190, 190);
const FILE_ADDS_RGB: (u8, u8, u8) = (90, 220, 110);
const FILE_DELS_RGB: (u8, u8, u8) = (255, 90, 90);
const FILE_BAR_RGB: (u8, u8, u8) = (60, 200, 200);
const FILE_BAR_CONFLICT_RGB: (u8, u8, u8) = (255, 80, 80);

/// Fade factor for a file row.
///
/// `Some(age)` shares the commit-log ramp via [`age_fade_factor`] so the
/// file list and log section darken in lockstep. `None` returns `1.0`
/// so files we can't stat (deleted entries, skipped untracked dirs)
/// render at the dark floor.
fn file_fade_factor(age: Option<Duration>) -> f32 {
    age.map_or(1.0, age_fade_factor)
}

/// Apply the age-driven truecolor fade to `s`, starting from `base`.
///
/// Shared by every truecolor commit-log colorizer so the fade math lives
/// in exactly one place — keeps the per-column functions to a single
/// readable `if truecolor { fade } else { fallback }` shape.
fn fade_truecolor(s: &str, age: Duration, base: (u8, u8, u8)) -> ColoredString {
    let (r, g, b) = fade_rgb(base, age_fade_factor(age));
    s.truecolor(r, g, b)
}

/// Color the short hash for a commit-log row.
///
/// With `truecolor`, the hash starts at [`LOG_HASH_BASE_RGB`] and fades
/// toward the dark floor as `age` grows. Without, falls back to the
/// legacy ANSI yellow so eight-color terminals still get a coloured hash.
fn colorize_log_hash(hash: &str, age: Duration, truecolor: bool) -> ColoredString {
    if truecolor {
        fade_truecolor(hash, age, LOG_HASH_BASE_RGB)
    } else {
        hash.yellow()
    }
}

/// Color the subject line for a commit-log row.
///
/// With `truecolor`, the subject fades from a near-white base toward the
/// dark floor. Without, falls back to the same Aging/Stale dim styling as
/// the file-row age column, so the row still gets quieter as it ages.
fn colorize_log_subject(subject: &str, age: Duration, truecolor: bool) -> ColoredString {
    if truecolor {
        fade_truecolor(subject, age, LOG_SUBJECT_BASE_RGB)
    } else {
        match age_dim_level(age) {
            AgeDim::Fresh | AgeDim::Recent => subject.normal(),
            AgeDim::Aging | AgeDim::Stale => subject.dimmed(),
        }
    }
}

/// Color the right-aligned age column for a commit-log row.
fn colorize_log_age(text: &str, age: Duration, truecolor: bool) -> ColoredString {
    if truecolor {
        fade_truecolor(text, age, LOG_AGE_BASE_RGB)
    } else {
        colorize_age(text, Some(age), 0.0, false)
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

/// Minimum rows the recent-commit section gets when it has content and
/// the file list is also non-empty. Without this floor, a branch with
/// hundreds of changed files would proportionally squeeze the log down
/// to one or two rows, hiding the very context the log section exists
/// to provide. The floor is capped by `log_demand` so a 3-commit branch
/// doesn't render with blank padding.
const LOG_FLOOR_ROWS: usize = 5;

/// Plan how many file rows and how many log rows the frame should render,
/// given the actual demand from each section and the total terminal rows
/// available for content (i.e. terminal height minus chrome the caller has
/// already deducted: header, post-header separator, inter-section
/// separator, and a reserved row for a possible `+N more files` footer).
///
/// When everything fits, both sections are rendered in full. When the
/// combined demand exceeds the available rows, rows are split
/// proportionally to each section's demand — except the log section is
/// floored at `min(LOG_FLOOR_ROWS, log_demand)` rows so recent commits
/// stay readable even with a very long file list. Each non-empty
/// section is guaranteed at least one row of its own.
///
/// Returns `(file_cap, log_cap)`. Each cap never exceeds the corresponding
/// demand.
pub fn plan_section_caps(
    file_demand: usize,
    log_demand: usize,
    available_rows: usize,
) -> (usize, usize) {
    if available_rows == 0 {
        return (0, 0);
    }
    if file_demand + log_demand <= available_rows {
        return (file_demand, log_demand);
    }
    if file_demand == 0 {
        return (0, available_rows.min(log_demand));
    }
    if log_demand == 0 {
        return (available_rows.min(file_demand), 0);
    }

    // Both sections want rows and the total overflows. Compute the log
    // section's proportional share, then lift it to the floor so a
    // dominant file list can't squeeze the recent-commit context away.
    // The cap on log_share preserves the existing invariant that the
    // file section keeps at least one row when it has content.
    let total_demand = file_demand + log_demand;
    let raw_log = (log_demand * available_rows + total_demand / 2) / total_demand;
    let log_ceiling = available_rows.saturating_sub(1).min(log_demand);
    let log_floor = LOG_FLOOR_ROWS.min(log_demand).min(log_ceiling);
    let log_share = raw_log.max(log_floor).min(log_ceiling);
    let file_share = available_rows.saturating_sub(log_share).min(file_demand);
    (file_share, log_share)
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
            truecolor: false,
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
            upstream: None,
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
                truecolor: false,
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
                truecolor: false,
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
                truecolor: false,
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
                truecolor: false,
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
    fn partial_cell_gets_background_to_close_gap() {
        // A partial-fill glyph like `▍` paints only the left portion of its
        // cell — the right portion shows the terminal background (black),
        // which reads as a dark gap between the bright bar and the dim `░`
        // empty cells to its right. Paint a matching dim background under
        // just the partial cell to bridge that gap.
        //
        // Force `colored` on so the test inspects the actual ANSI we would
        // emit on a real terminal; without this the crate strips all codes
        // in non-TTY test runs.
        colored::control::set_override(true);
        let e = entry("foo.rs", FileStatus::Modified, true, 9, 1);
        let with_partial = colorize_bar("█████▍", &e, 0.0, false);
        let all_full = colorize_bar("██████", &e, 0.0, false);
        let all_empty = colorize_bar("░░░░░░", &e, 0.0, false);
        colored::control::unset_override();

        assert!(
            with_partial.contains("\x1b[48"),
            "partial-fill cell should have a background color applied: {with_partial:?}",
        );
        assert!(
            !all_full.contains("\x1b[48"),
            "all-full bar needs no background color: {all_full:?}",
        );
        assert!(
            !all_empty.contains("\x1b[48"),
            "all-empty bar needs no background color: {all_empty:?}",
        );
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
    fn plan_section_caps_returns_full_demand_when_room_is_plenty() {
        // No contention: 5 files + 20 log rows easily fit in 100 rows.
        assert_eq!(plan_section_caps(5, 20, 100), (5, 20));
    }

    #[test]
    fn plan_section_caps_splits_proportionally_when_overflowing() {
        // 5 files vs 20 log rows competing for 10 rows: file share is
        // round(5*10/25) = 2, log gets the remaining 8. This is the case
        // that motivated the change — today the file list gets squeezed
        // to 1 or 2 rows regardless of how few files the user actually has.
        assert_eq!(plan_section_caps(5, 20, 10), (2, 8));
    }

    #[test]
    fn plan_section_caps_guarantees_each_section_at_least_one_row() {
        // file=1, log=100 would proportionally give file 0 rows (rounded
        // down). Each non-empty section must keep at least one row so we
        // never silently hide a section that has content.
        let (f, l) = plan_section_caps(1, 100, 10);
        assert!(f >= 1, "non-empty file section must get at least 1 row, got {f}");
        assert_eq!(f + l, 10, "all rows should be allocated when overflowing");
    }

    #[test]
    fn plan_section_caps_grants_all_rows_to_lone_section() {
        // When only one section has content, it should claim every
        // available row up to its demand. The other section gets zero.
        assert_eq!(plan_section_caps(0, 20, 10), (0, 10));
        assert_eq!(plan_section_caps(20, 0, 10), (10, 0));
    }

    #[test]
    fn plan_section_caps_returns_zero_when_no_rows_available() {
        // Pathologically short terminal: nothing fits, so nothing is
        // promised. The caller will at least render header chrome.
        assert_eq!(plan_section_caps(5, 5, 0), (0, 0));
    }

    #[test]
    fn plan_section_caps_never_exceeds_demand() {
        // The contract: returned caps never exceed the corresponding
        // demand, even when the proportional formula would round up past
        // it. file=2, log=20, available=14 is the kind of edge case where
        // a naive proportional formula could produce a file cap > 2.
        let (f, l) = plan_section_caps(2, 20, 14);
        assert!(f <= 2, "file cap must not exceed demand: got {f}");
        assert!(l <= 20, "log cap must not exceed demand: got {l}");
        assert!(f + l <= 14, "total must fit in available rows: {f}+{l}");
    }

    #[test]
    fn plan_section_caps_floors_log_at_five_rows_when_files_dominate() {
        // Repro of the "too many files" report: a branch with ~129
        // changed files vs the default 20-line log on a ~26-row
        // terminal. A naive proportional split would squeeze the log
        // section down to ~3 rows; the floor lifts that to 5 so the
        // recent-commit context stays visible.
        let (f, l) = plan_section_caps(129, 20, 26);
        assert_eq!(l, 5, "log section should be floored at 5 rows, got {l}");
        assert_eq!(
            f, 21,
            "file section should claim the remaining rows after the log floor, got {f}"
        );
    }

    #[test]
    fn plan_section_caps_log_floor_does_not_pad_above_demand() {
        // The floor is min(5, log_demand). With only 3 commits ahead,
        // the log section should get exactly those 3 rows rather than 5
        // rows with two empty lines at the bottom.
        let (f, l) = plan_section_caps(100, 3, 20);
        assert_eq!(l, 3, "log cap should equal demand when demand < floor, got {l}");
        assert_eq!(f, 17, "file section should get the remaining rows, got {f}");
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
        let aging = colorize_age("12h0m", Some(Duration::from_secs(2 * 3600)), 0.0, false);
        let stale = colorize_age("12h0m", Some(Duration::from_secs(2 * 86400)), 0.0, false);
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
    fn header_includes_upstream_arrows_and_name_when_set() {
        // When the current branch has an upstream tracking ref, the header
        // should report how far ahead/behind we are of it, alongside the
        // existing base-branch comparison. Using arrows keeps the field
        // compact for the viddy header line.
        let mut snap = snap_with(vec![]);
        snap.upstream = Some(UpstreamStatus {
            name: "origin/gsv".into(),
            ahead: 2,
            behind: 1,
        });
        let out = strip_ansi(&render(&snap, &opts()));
        let header = out.lines().next().unwrap_or("");
        assert!(
            header.contains("origin/gsv"),
            "header should name the upstream tracking branch: {header}",
        );
        assert!(
            header.contains("↑2"),
            "header should show commits-ahead count with ↑: {header}",
        );
        assert!(
            header.contains("↓1"),
            "header should show commits-behind count with ↓: {header}",
        );
    }

    #[test]
    fn header_shows_zero_ahead_zero_behind_when_in_sync_with_upstream() {
        // Predictable shape: even when ahead/behind are both 0, render the
        // arrows so the eye can scan a column of values under viddy without
        // the field appearing/disappearing.
        let mut snap = snap_with(vec![]);
        snap.upstream = Some(UpstreamStatus {
            name: "origin/main".into(),
            ahead: 0,
            behind: 0,
        });
        let out = strip_ansi(&render(&snap, &opts()));
        let header = out.lines().next().unwrap_or("");
        assert!(
            header.contains("↑0") && header.contains("↓0"),
            "header should still show ↑0 ↓0 when in sync: {header}",
        );
        assert!(
            header.contains("origin/main"),
            "header should name the upstream even when in sync: {header}",
        );
    }

    #[test]
    fn header_omits_upstream_field_when_no_upstream() {
        // No upstream configured: the header should not include the arrows
        // at all, and certainly not invent an "origin/…" name.
        let snap = snap_with(vec![]);
        let out = strip_ansi(&render(&snap, &opts()));
        let header = out.lines().next().unwrap_or("");
        assert!(
            !header.contains('↑') && !header.contains('↓'),
            "header should not show upstream arrows without an upstream: {header}",
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

    // --- truecolor commit-log fade ---------------------------------------
    //
    // These tests inspect the `ColoredString::fgcolor` field directly rather
    // than rendering to ANSI. The `colored` crate gates ANSI emission on a
    // process-global override that races with parallel tests; reading the
    // typed color avoids that entirely (same pattern as
    // `stale_age_renders_differently_from_aging` above).

    #[test]
    fn log_hash_uses_truecolor_when_enabled() {
        // With truecolor on, a fresh commit's hash must be coloured with a
        // 24-bit RGB value (not the legacy `Color::Yellow`), so the gradient
        // has somewhere to fade *from*.
        use colored::Color;
        let cs = colorize_log_hash("abc1234", Duration::from_secs(0), true);
        match cs.fgcolor {
            Some(Color::TrueColor { .. }) => {}
            other => panic!("expected TrueColor when truecolor=true, got {other:?}"),
        }
    }

    #[test]
    fn log_hash_falls_back_to_yellow_without_truecolor() {
        // Without truecolor, the legacy 8-colour yellow must still come
        // through — otherwise we silently drop hash colouring on terminals
        // that can't render 24-bit RGB.
        use colored::Color;
        let cs = colorize_log_hash("abc1234", Duration::from_secs(0), false);
        assert_eq!(cs.fgcolor, Some(Color::Yellow));
    }

    #[test]
    fn log_hash_darkens_with_age_under_truecolor() {
        // The core gradient property: an hour-old commit's hash must come
        // out darker (lower channel values) than a fresh commit's hash on
        // every channel.
        use colored::Color;
        let fresh = colorize_log_hash("abc1234", Duration::from_secs(0), true);
        let hour = colorize_log_hash("abc1234", Duration::from_secs(60 * 60), true);
        let (Some(Color::TrueColor { r: fr, g: fg, b: fb }), Some(Color::TrueColor { r: hr, g: hg, b: hb })) =
            (fresh.fgcolor, hour.fgcolor)
        else {
            panic!("both should be TrueColor under truecolor=true");
        };
        assert!(
            hr < fr && hg <= fg && hb <= fb,
            "hour-old hash should be darker than fresh: fresh=({fr},{fg},{fb}) hour=({hr},{hg},{hb})",
        );
    }

    #[test]
    fn log_hash_stays_above_floor_when_very_old() {
        // The fade must never reach pure black — a week-old commit should
        // still be readable, which means every channel stays at or above
        // the FADE_FLOOR fraction of its base value.
        use crate::age::FADE_FLOOR;
        use colored::Color;
        let cs = colorize_log_hash(
            "abc1234",
            Duration::from_secs(60 * 60 * 24 * 7),
            true,
        );
        let Some(Color::TrueColor { r, g, b }) = cs.fgcolor else {
            panic!("expected TrueColor under truecolor=true");
        };
        // Derive the per-channel floor from the live base RGB so this
        // test asserts the invariant ("no channel drops below its
        // FADE_FLOOR fraction") rather than hardcoding numbers tied to
        // today's choice of LOG_HASH_BASE_RGB. If the base later gains
        // a non-zero blue, the test still checks the right bound.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "u8 × FADE_FLOOR ∈ [0, 1] stays in [0, 255]"
        )]
        let floor_of = |c: u8| (f32::from(c) * FADE_FLOOR).round() as u8;
        let (base_r, base_g, base_b) = LOG_HASH_BASE_RGB;
        let min_r = floor_of(base_r);
        let min_g = floor_of(base_g);
        let min_b = floor_of(base_b);
        // .saturating_sub(1) absorbs one RGB-unit of rounding drift at
        // the fade-curve boundary; the test still fails if any channel
        // drops meaningfully below its computed floor.
        assert!(
            r >= min_r.saturating_sub(1)
                && g >= min_g.saturating_sub(1)
                && b >= min_b.saturating_sub(1),
            "channels must not drop below the floor: actual=({r},{g},{b}) min=({min_r},{min_g},{min_b}) base=({base_r},{base_g},{base_b})",
        );
    }

    #[test]
    fn log_subject_uses_truecolor_when_enabled() {
        // Subjects need to fade too, otherwise the hash darkens while the
        // text next to it stays bright — visually inconsistent.
        use colored::Color;
        let cs = colorize_log_subject("a commit subject", Duration::from_secs(0), true);
        match cs.fgcolor {
            Some(Color::TrueColor { .. }) => {}
            other => panic!("expected TrueColor for subject when truecolor=true, got {other:?}"),
        }
    }

    #[test]
    fn log_subject_darkens_with_age_under_truecolor() {
        use colored::Color;
        let fresh = colorize_log_subject("subj", Duration::from_secs(0), true);
        let hour = colorize_log_subject("subj", Duration::from_secs(60 * 60), true);
        let (Some(Color::TrueColor { r: fr, .. }), Some(Color::TrueColor { r: hr, .. })) =
            (fresh.fgcolor, hour.fgcolor)
        else {
            panic!("both should be TrueColor under truecolor=true");
        };
        assert!(hr < fr, "hour-old subject should be darker than fresh: {fr} -> {hr}");
    }

    #[test]
    fn log_age_uses_truecolor_when_enabled() {
        use colored::Color;
        let cs = colorize_log_age("5m23s", Duration::from_secs(5 * 60 + 23), true);
        match cs.fgcolor {
            Some(Color::TrueColor { .. }) => {}
            other => panic!("expected TrueColor for age when truecolor=true, got {other:?}"),
        }
    }

    #[test]
    fn log_age_darkens_with_age_under_truecolor() {
        use colored::Color;
        let fresh = colorize_log_age("0s", Duration::from_secs(0), true);
        let hour = colorize_log_age("1h0m", Duration::from_secs(60 * 60), true);
        let (Some(Color::TrueColor { r: fr, .. }), Some(Color::TrueColor { r: hr, .. })) =
            (fresh.fgcolor, hour.fgcolor)
        else {
            panic!("both should be TrueColor under truecolor=true");
        };
        assert!(hr < fr, "hour-old age column should be darker than fresh: {fr} -> {hr}");
    }

    #[test]
    fn file_age_uses_truecolor_when_enabled() {
        use colored::Color;
        let cs = colorize_age("5m23s", Some(Duration::from_secs(5 * 60 + 23)), 0.0, true);
        match cs.fgcolor {
            Some(Color::TrueColor { .. }) => {}
            other => panic!("expected TrueColor for file age, got {other:?}"),
        }
    }

    #[test]
    fn file_age_falls_back_to_dim_buckets_without_truecolor() {
        // 8-color fallback must still bold a fresh row's age, matching today.
        use colored::Styles;
        let fresh = colorize_age("30s", Some(Duration::from_secs(30)), 0.0, false);
        assert!(
            fresh.style.contains(Styles::Bold),
            "fresh age should still be bolded in the 8-color path",
        );
    }

    #[test]
    fn file_age_darkens_with_factor_under_truecolor() {
        use colored::Color;
        let fresh = colorize_age("30s", Some(Duration::from_secs(30)), 0.0, true);
        let aged = colorize_age("3d0h", Some(Duration::from_secs(3 * 86400)), 1.0, true);
        let (Some(Color::TrueColor { r: fr, .. }), Some(Color::TrueColor { r: ar, .. })) =
            (fresh.fgcolor, aged.fgcolor)
        else { panic!("both should be TrueColor") };
        assert!(ar < fr, "aged file-age column should be darker: fresh={fr} aged={ar}");
    }

    #[test]
    fn fallback_stale_subject_is_not_italic() {
        // User feedback: italics on old subjects looks weird and out of
        // place. The fallback path should still dim stale subjects (so
        // age is conveyed at all) but without leaning the text.
        use colored::Styles;
        let stale = colorize_log_subject(
            "an old subject",
            Duration::from_secs(60 * 60 * 24 * 7),
            false,
        );
        assert!(
            !stale.style.contains(Styles::Italic),
            "stale subjects should be dimmed but not italicized in fallback mode",
        );
    }

    #[test]
    fn file_fade_factor_is_zero_for_fresh_age() {
        // A file modified moments ago must render at full base brightness,
        // which means factor=0 — the no-fade end of the ramp.
        assert!(
            (file_fade_factor(Some(Duration::from_secs(0))) - 0.0).abs() < 1e-6,
            "fresh file should produce factor=0",
        );
    }

    #[test]
    fn file_fade_factor_floors_when_age_is_none() {
        // Deleted files and unstat'd untracked dirs have no mtime. They must
        // render at the dark floor (factor=1.0) so the row visually announces
        // "this is an unusual state, not actively changing".
        assert!(
            (file_fade_factor(None) - 1.0).abs() < 1e-6,
            "None age should clamp to factor=1.0 (the floor)",
        );
    }

    #[test]
    fn file_fade_factor_matches_commit_ramp_for_some_age() {
        // The file fade must share the *same* ramp as commit rows so the two
        // sections darken in lockstep under viddy. Spot-check the 1h midpoint.
        let one_hour = Duration::from_secs(60 * 60);
        let file = file_fade_factor(Some(one_hour));
        let commit = age_fade_factor(one_hour);
        assert!(
            (file - commit).abs() < 1e-6,
            "file fade must equal commit fade for matching Some(age): file={file}, commit={commit}",
        );
    }

    #[test]
    fn file_path_uses_truecolor_when_enabled() {
        // With truecolor on, a fresh modified file's path must come back as a
        // 24-bit color so the gradient has somewhere to fade from.
        use colored::Color;
        let mut e = entry("src/foo.rs", FileStatus::Modified, false, 1, 0);
        e.age = Some(Duration::from_secs(0));
        let cs = colorize_path("src/foo.rs", &e, 0.0, true);
        match cs.fgcolor {
            Some(Color::TrueColor { .. }) => {}
            other => panic!("expected TrueColor under truecolor=true, got {other:?}"),
        }
    }

    #[test]
    fn file_path_falls_back_to_legacy_color_without_truecolor() {
        // Without truecolor, the legacy ANSI yellow for unstaged-modified
        // paths must still come through unchanged. Regression guard for the
        // 8-color path.
        use colored::Color;
        let e = entry("src/foo.rs", FileStatus::Modified, false, 1, 0);
        let cs = colorize_path("src/foo.rs", &e, 0.0, false);
        assert_eq!(cs.fgcolor, Some(Color::Yellow));
    }

    #[test]
    fn file_path_darkens_with_age_under_truecolor() {
        // Core gradient property: an older path is dimmer than a fresh one on
        // every channel.
        use colored::Color;
        let e = entry("src/foo.rs", FileStatus::Modified, false, 1, 0);
        let fresh = colorize_path("src/foo.rs", &e, 0.0, true);
        let aged = colorize_path("src/foo.rs", &e, 1.0, true);
        let (Some(Color::TrueColor { r: fr, g: fg, b: fb }),
             Some(Color::TrueColor { r: ar, g: ag, b: ab })) =
            (fresh.fgcolor, aged.fgcolor)
        else {
            panic!("both should be TrueColor under truecolor=true");
        };
        assert!(
            ar <= fr && ag <= fg && ab <= fb,
            "aged path should not be brighter on any channel: fresh=({fr},{fg},{fb}) aged=({ar},{ag},{ab})",
        );
        assert!(
            ar < fr || ag < fg || ab < fb,
            "aged path should be strictly darker on at least one channel: fresh=({fr},{fg},{fb}) aged=({ar},{ag},{ab})",
        );
    }

    #[test]
    fn file_path_stays_above_floor_at_factor_one() {
        // The fade must never reach pure black — at the floor, channels stay
        // at FADE_FLOOR * base. Mirrors log_hash_stays_above_floor_when_very_old.
        use crate::age::FADE_FLOOR;
        use colored::Color;
        let e = entry("src/foo.rs", FileStatus::Modified, false, 1, 0);
        let cs = colorize_path("src/foo.rs", &e, 1.0, true);
        let Some(Color::TrueColor { r, g, b }) = cs.fgcolor else {
            panic!("expected TrueColor under truecolor=true");
        };
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "u8 × FADE_FLOOR ∈ [0, 1] stays in [0, 255]"
        )]
        let floor_of = |c: u8| (f32::from(c) * FADE_FLOOR).round() as u8;
        let (br, bg, bb) = FILE_PATH_UNSTAGED_RGB;
        assert!(
            r >= floor_of(br).saturating_sub(1)
                && g >= floor_of(bg).saturating_sub(1)
                && b >= floor_of(bb).saturating_sub(1),
            "channels must not drop below the floor: actual=({r},{g},{b}) base=({br},{bg},{bb})",
        );
    }

    #[test]
    fn file_icon_uses_truecolor_when_enabled() {
        use colored::Color;
        let e = entry("src/foo.rs", FileStatus::Modified, true, 1, 0);
        let cs = colorize_icon('●', &e, 0.0, true);
        match cs.fgcolor {
            Some(Color::TrueColor { .. }) => {}
            other => panic!("expected TrueColor for icon under truecolor=true, got {other:?}"),
        }
    }

    #[test]
    fn file_icon_falls_back_to_ansi_without_truecolor() {
        // Staged-modified icon today is plain green. Regression guard.
        use colored::Color;
        let e = entry("src/foo.rs", FileStatus::Modified, true, 1, 0);
        let cs = colorize_icon('●', &e, 0.0, false);
        assert_eq!(cs.fgcolor, Some(Color::Green));
    }

    #[test]
    fn file_icon_darkens_with_age_under_truecolor() {
        use colored::Color;
        let e = entry("src/foo.rs", FileStatus::Modified, true, 1, 0);
        let fresh = colorize_icon('●', &e, 0.0, true);
        let aged = colorize_icon('●', &e, 1.0, true);
        let (Some(Color::TrueColor { r: fr, .. }), Some(Color::TrueColor { r: ar, .. })) =
            (fresh.fgcolor, aged.fgcolor)
        else {
            panic!("both should be TrueColor");
        };
        assert!(ar < fr, "aged icon should be darker: fresh={fr} aged={ar}");
    }

    #[test]
    fn file_letter_uses_truecolor_when_enabled() {
        use colored::Color;
        let e = entry("src/foo.rs", FileStatus::Added, true, 1, 0);
        let cs = colorize_letter('A', &e, 0.0, true);
        match cs.fgcolor {
            Some(Color::TrueColor { .. }) => {}
            other => panic!("expected TrueColor under truecolor=true, got {other:?}"),
        }
    }

    #[test]
    fn file_letter_falls_back_to_ansi_without_truecolor() {
        use colored::Color;
        let e = entry("src/foo.rs", FileStatus::Added, true, 1, 0);
        let cs = colorize_letter('A', &e, 0.0, false);
        assert_eq!(cs.fgcolor, Some(Color::Green));
    }

    #[test]
    fn file_letter_darkens_with_age_under_truecolor() {
        use colored::Color;
        let e = entry("src/foo.rs", FileStatus::Deleted, true, 0, 1);
        let fresh = colorize_letter('D', &e, 0.0, true);
        let aged = colorize_letter('D', &e, 1.0, true);
        let (Some(Color::TrueColor { r: fr, .. }), Some(Color::TrueColor { r: ar, .. })) =
            (fresh.fgcolor, aged.fgcolor)
        else { panic!("both should be TrueColor") };
        assert!(ar < fr, "aged letter should be darker: fresh={fr} aged={ar}");
    }

    #[test]
    fn file_adds_uses_truecolor_when_enabled() {
        use colored::Color;
        let cs = colorize_adds("  +12", 0.0, true);
        match cs.fgcolor {
            Some(Color::TrueColor { .. }) => {}
            other => panic!("expected TrueColor, got {other:?}"),
        }
    }

    #[test]
    fn file_dels_uses_truecolor_when_enabled() {
        use colored::Color;
        let cs = colorize_dels(" -3", 0.0, true);
        match cs.fgcolor {
            Some(Color::TrueColor { .. }) => {}
            other => panic!("expected TrueColor, got {other:?}"),
        }
    }

    #[test]
    fn file_adds_falls_back_to_green_without_truecolor() {
        use colored::Color;
        let cs = colorize_adds("  +12", 0.0, false);
        assert_eq!(cs.fgcolor, Some(Color::Green));
    }

    #[test]
    fn file_dels_falls_back_to_red_without_truecolor() {
        use colored::Color;
        let cs = colorize_dels(" -3", 0.0, false);
        assert_eq!(cs.fgcolor, Some(Color::Red));
    }

    #[test]
    fn file_adds_darkens_with_factor_under_truecolor() {
        use colored::Color;
        let fresh = colorize_adds("  +12", 0.0, true);
        let aged = colorize_adds("  +12", 1.0, true);
        let (Some(Color::TrueColor { r: fr, g: fg, .. }),
             Some(Color::TrueColor { r: ar, g: ag, .. })) =
            (fresh.fgcolor, aged.fgcolor)
        else { panic!("both should be TrueColor") };
        assert!(ar < fr || ag < fg, "aged +adds should be darker");
    }

    #[test]
    fn file_bar_fill_fades_with_factor_under_truecolor() {
        use colored::Color;
        let e = entry("foo.rs", FileStatus::Modified, true, 6, 0);
        let fresh = colorize_bar_styled("██████", &e, 0.0, true);
        let aged = colorize_bar_styled("██████", &e, 1.0, true);
        // We expect the first cell's fg to be TrueColor in both cases and
        // the aged channel to be strictly lower.
        let (Some(Color::TrueColor { r: fr, g: fg, b: fb }),
             Some(Color::TrueColor { r: ar, g: ag, b: ab })) =
            (fresh[0].fgcolor, aged[0].fgcolor)
        else { panic!("first cell should be TrueColor under truecolor=true") };
        assert!(
            ar < fr || ag < fg || ab < fb,
            "aged bar fill should be darker on at least one channel",
        );
    }

    #[test]
    fn file_bar_partial_bg_fades_with_factor_under_truecolor() {
        use colored::Color;
        // Use a partial-fill glyph (▍ = U+258D) so a background color is set.
        // BAR_PARTIAL_BG_CYAN = (0, 48, 48): r=0 so check g channel instead.
        let e = entry("foo.rs", FileStatus::Modified, true, 6, 0);
        let fresh = colorize_bar_styled("▍", &e, 0.0, true);
        let aged = colorize_bar_styled("▍", &e, 1.0, true);
        let (Some(Color::TrueColor { g: fg, .. }), Some(Color::TrueColor { g: ag, .. })) =
            (fresh[0].bgcolor, aged[0].bgcolor)
        else { panic!("partial cell should have a TrueColor background") };
        assert!(ag < fg, "aged partial-cell bg should be darker: fresh={fg} aged={ag}");
    }

    #[test]
    fn file_bar_fallback_unchanged_without_truecolor() {
        // 8-color path returns the cyan-fill bytes today. Regression guard.
        let e = entry("foo.rs", FileStatus::Modified, true, 6, 0);
        let cells = colorize_bar_styled("█", &e, 0.0, false);
        use colored::Color;
        assert_eq!(cells[0].fgcolor, Some(Color::Cyan));
    }

    // --- Phase 8: end-to-end render() truecolor wiring ----------------------

    #[test]
    fn file_row_renders_with_truecolor_when_enabled() {
        use colored::Color;
        // Force the colored crate to actually emit ANSI in the test process so
        // we can detect the truecolor codes from the rendered output.
        colored::control::set_override(true);
        let snap = snap_with(vec![entry("src/foo.rs", FileStatus::Modified, false, 5, 2)]);
        let mut o = opts();
        o.truecolor = true;
        let out = render(&snap, &o);
        colored::control::unset_override();
        // Truecolor foreground sequences start with `\x1b[38;2;`.
        assert!(
            out.contains("\x1b[38;2;"),
            "rendered file row should contain a truecolor ANSI sequence when truecolor=true",
        );
        // Silence the unused-import warning when the macro doesn't fire below.
        let _ = Color::Red;
    }

    #[test]
    fn file_row_no_truecolor_in_8_color_mode() {
        colored::control::set_override(true);
        let snap = snap_with(vec![entry("src/foo.rs", FileStatus::Modified, false, 5, 2)]);
        let out = render(&snap, &opts());
        colored::control::unset_override();
        assert!(
            !out.contains("\x1b[38;2;"),
            "8-color mode must not emit any truecolor sequences for file rows",
        );
    }

    #[test]
    fn file_row_darkens_with_mtime_under_truecolor() {
        // End-to-end: an older file's row should contain a darker (lower-channel)
        // truecolor sequence than a fresher row of the same status.
        colored::control::set_override(true);
        let mut fresh_entry = entry("src/foo.rs", FileStatus::Modified, false, 5, 2);
        fresh_entry.age = Some(Duration::from_secs(0));
        let mut aged_entry = entry("src/bar.rs", FileStatus::Modified, false, 5, 2);
        aged_entry.age = Some(Duration::from_secs(60 * 60));

        let fresh_snap = snap_with(vec![fresh_entry]);
        let aged_snap = snap_with(vec![aged_entry]);
        let mut o = opts();
        o.truecolor = true;
        let fresh_out = render(&fresh_snap, &o);
        let aged_out = render(&aged_snap, &o);
        colored::control::unset_override();

        let max_r = |s: &str| {
            // Extract the largest r-channel from any 38;2;r;g;b foreground sequence.
            let mut best: Option<u8> = None;
            let bytes = s.as_bytes();
            let needle = b"\x1b[38;2;";
            let mut i = 0;
            while let Some(pos) = bytes[i..].windows(needle.len()).position(|w| w == needle) {
                let start = i + pos + needle.len();
                // Read r digits.
                let mut j = start;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > start {
                    if let Ok(r) = std::str::from_utf8(&bytes[start..j]).unwrap().parse::<u8>() {
                        best = Some(best.map_or(r, |b| b.max(r)));
                    }
                }
                i = j;
            }
            best.expect("at least one truecolor sequence")
        };

        let fresh_max = max_r(&fresh_out);
        let aged_max = max_r(&aged_out);
        assert!(
            aged_max < fresh_max,
            "aged row's brightest channel should be lower than fresh row's: fresh={fresh_max} aged={aged_max}",
        );
    }

    #[test]
    fn file_row_no_age_renders_at_floor_under_truecolor() {
        // Deleted file (age=None) should produce only sequences with channels
        // at or below the FADE_FLOOR fraction of their base.
        use crate::age::FADE_FLOOR;
        colored::control::set_override(true);
        let mut e = entry("deleted.rs", FileStatus::Deleted, true, 0, 5);
        e.age = None;
        let snap = snap_with(vec![e]);
        let mut o = opts();
        o.truecolor = true;
        let out = render(&snap, &o);
        colored::control::unset_override();

        // The brightest channel allowed at the floor is `255 × FADE_FLOOR`
        // (a base channel of 255 hits the highest floor). Use that as the
        // conservative upper bound for any column, plus a small slack for
        // rounding. Any row column emitting a channel above this means a
        // colorize_* fn forgot to apply the fade.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "255.0 × FADE_FLOOR ∈ [0, 255]"
        )]
        let upper = ((255.0_f32 * FADE_FLOOR).round() as u8).saturating_add(2);

        // Parse every r-channel and assert all are <= upper.
        let bytes = out.as_bytes();
        let needle = b"\x1b[38;2;";
        let mut i = 0;
        let mut saw_any = false;
        while let Some(pos) = bytes[i..].windows(needle.len()).position(|w| w == needle) {
            let start = i + pos + needle.len();
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > start {
                let r: u8 = std::str::from_utf8(&bytes[start..j]).unwrap().parse().unwrap();
                assert!(
                    r <= upper,
                    "every channel on a no-age row should sit at or below the floor (got {r}, upper {upper})",
                );
                saw_any = true;
            }
            i = j;
        }
        assert!(saw_any, "should have emitted at least one truecolor sequence");
    }
}
