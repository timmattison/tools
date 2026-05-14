//! Assemble a [`Snapshot`] into a colored, compact text block.

use std::time::Duration;

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

/// Produce the colored, multi-line frame.
pub fn render(_snapshot: &Snapshot, _opts: &RenderOptions) -> String {
    // Stub.
    String::from("TODO")
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
                // CSI terminator is any byte in 0x40..=0x7E.
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
        // Staged → filled-circle icon.
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
        // No counts, no bar — the bar block chars should not appear.
        assert!(
            !row.contains('█') && !row.contains('░'),
            "untracked row should not include a bar: {row}",
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
        // Japanese (each char is 3 bytes UTF-8 but display width 2)
        let path = "日本語/とても/長い/ファイル名.rs".to_string();
        let snap = snap_with(vec![entry(&path, FileStatus::Modified, true, 1, 0)]);
        // Narrow width to force truncation through multi-byte chars.
        let out = render(
            &snap,
            &RenderOptions {
                terminal_width: 40,
                bar_width: 6,
                max_files: None,
            },
        );
        let stripped = strip_ansi(&out);
        // Must contain SOME suffix of the file name.
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
        // Three file rows, then "+7 more".
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
}
