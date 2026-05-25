//! Parse `git status --porcelain=v2 -z` and `git diff --numstat -z` output.

use std::collections::HashMap;

/// What kind of change a [`FileEntry`] represents.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChange,
    Untracked,
    UntrackedDir,
    Conflicted,
}

/// One row in the rendered output.
///
/// A file with both staged and unstaged changes produces two entries —
/// that mirrors how `git status` displays it and what the user wants to see.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct FileEntry {
    pub path: String,
    /// Set for renames/copies; this is the pre-rename path.
    pub orig_path: Option<String>,
    pub status: FileStatus,
    /// True when the change is in the index (staged), false when only in the worktree.
    pub staged: bool,
}

/// Adds/deletes for a single file, from `git diff --numstat`.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub struct NumStat {
    pub adds: u32,
    pub dels: u32,
    /// Binary files report `-`/`-` in numstat; we surface that here so render
    /// can show `bin` instead of an empty bar.
    pub binary: bool,
}

/// Parse the output of `git status --porcelain=v2 -z`.
///
/// Input is a single string with `\0`-separated records (the `-z` form).
/// For renames/copies the new path and original path are split by `\0`
/// within the entry, which is why we consume two records for a type-2 entry.
#[allow(dead_code, reason = "retained until Task 9 removes all git-CLI parsers")]
pub fn parse_status(input: &str) -> Vec<FileEntry> {
    let pieces: Vec<&str> = input.split('\0').filter(|s| !s.is_empty()).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < pieces.len() {
        let piece = pieces[i];
        let (type_ch, rest) = split_first(piece);
        match type_ch {
            Some('1') => {
                if let Some(rest) = rest.strip_prefix(' ') {
                    parse_ordinary_entry(rest, None, &mut out);
                }
                i += 1;
            }
            Some('2') => {
                let orig = pieces.get(i + 1).map(|s| (*s).to_string());
                if let Some(rest) = rest.strip_prefix(' ') {
                    parse_rename_entry(rest, orig, &mut out);
                }
                i += 2;
            }
            Some('u') => {
                if let Some(rest) = rest.strip_prefix(' ') {
                    parse_unmerged_entry(rest, &mut out);
                }
                i += 1;
            }
            Some('?') => {
                if let Some(path) = rest.strip_prefix(' ') {
                    let status = if path.ends_with('/') {
                        FileStatus::UntrackedDir
                    } else {
                        FileStatus::Untracked
                    };
                    out.push(FileEntry {
                        path: path.to_string(),
                        orig_path: None,
                        status,
                        staged: false,
                    });
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

#[allow(dead_code, reason = "retained until Task 9 removes all git-CLI parsers")]
fn split_first(s: &str) -> (Option<char>, &str) {
    let mut chars = s.chars();
    let first = chars.next();
    (first, chars.as_str())
}

#[allow(dead_code, reason = "retained until Task 9 removes all git-CLI parsers")]
fn parse_ordinary_entry(rest: &str, orig: Option<String>, out: &mut Vec<FileEntry>) {
    // <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
    let parts: Vec<&str> = rest.splitn(8, ' ').collect();
    if parts.len() < 8 {
        return;
    }
    emit_xy(parts[0], parts[7], orig, out);
}

#[allow(dead_code, reason = "retained until Task 9 removes all git-CLI parsers")]
fn parse_rename_entry(rest: &str, orig: Option<String>, out: &mut Vec<FileEntry>) {
    // <XY> <sub> <mH> <mI> <mW> <hH> <hI> <Xscore> <path>
    let parts: Vec<&str> = rest.splitn(9, ' ').collect();
    if parts.len() < 9 {
        return;
    }
    emit_xy(parts[0], parts[8], orig, out);
}

#[allow(dead_code, reason = "retained until Task 9 removes all git-CLI parsers")]
fn parse_unmerged_entry(rest: &str, out: &mut Vec<FileEntry>) {
    // <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
    let parts: Vec<&str> = rest.splitn(10, ' ').collect();
    if parts.len() < 10 {
        return;
    }
    out.push(FileEntry {
        path: parts[9].to_string(),
        orig_path: None,
        status: FileStatus::Conflicted,
        staged: false,
    });
}

#[allow(dead_code, reason = "retained until Task 9 removes all git-CLI parsers")]
fn emit_xy(xy: &str, path: &str, orig: Option<String>, out: &mut Vec<FileEntry>) {
    let mut xy_chars = xy.chars();
    let x_status = char_to_status(xy_chars.next().unwrap_or('.'));
    let y_status = char_to_status(xy_chars.next().unwrap_or('.'));
    // The worktree side of a rename is just an edit on the new path, not a
    // rename itself — orig_path only flows to Y when Y is itself a rename/copy.
    let y_takes_orig = matches!(y_status, Some(FileStatus::Renamed | FileStatus::Copied));
    let (orig_for_x, orig_for_y) = match (x_status.is_some(), y_takes_orig) {
        (true, true) => (orig.clone(), orig),
        (true, false) => (orig, None),
        (false, true) => (None, orig),
        (false, false) => (None, None),
    };
    if let Some(status) = x_status {
        out.push(FileEntry {
            path: path.to_string(),
            orig_path: orig_for_x,
            status,
            staged: true,
        });
    }
    if let Some(status) = y_status {
        out.push(FileEntry {
            path: path.to_string(),
            orig_path: orig_for_y,
            status,
            staged: false,
        });
    }
}

#[allow(dead_code, reason = "retained until Task 9 removes all git-CLI parsers")]
fn char_to_status(c: char) -> Option<FileStatus> {
    match c {
        'M' => Some(FileStatus::Modified),
        'A' => Some(FileStatus::Added),
        'D' => Some(FileStatus::Deleted),
        'R' => Some(FileStatus::Renamed),
        'C' => Some(FileStatus::Copied),
        'T' => Some(FileStatus::TypeChange),
        _ => None,
    }
}

/// Parse the output of `git diff [--cached] --numstat -z`.
///
/// Returns a map from path to its [`NumStat`]. Renames in `-z` numstat
/// emit three NUL-separated tokens: `adds\tdels\t\0origPath\0newPath`,
/// and we key the result on the new path.
#[allow(dead_code, reason = "retained until Task 9 removes all git-CLI parsers")]
pub fn parse_numstat(input: &str) -> HashMap<String, NumStat> {
    let pieces: Vec<&str> = input.split('\0').filter(|s| !s.is_empty()).collect();
    let mut out = HashMap::new();
    let mut i = 0;
    while i < pieces.len() {
        let header = pieces[i];
        let mut tabs = header.split('\t');
        let adds_str = tabs.next().unwrap_or("");
        let dels_str = tabs.next().unwrap_or("");
        let path_part = tabs.next().unwrap_or("");
        let (binary, adds, dels) = if adds_str == "-" && dels_str == "-" {
            (true, 0, 0)
        } else {
            (
                false,
                adds_str.parse::<u32>().unwrap_or(0),
                dels_str.parse::<u32>().unwrap_or(0),
            )
        };
        let (path, step) = if path_part.is_empty() {
            // Rename: next is origPath, then newPath. Key on newPath.
            let new_path = pieces.get(i + 2).map_or(String::new(), |s| (*s).to_string());
            (new_path, 3)
        } else {
            (path_part.to_string(), 1)
        };
        if !path.is_empty() {
            out.insert(path, NumStat { adds, dels, binary });
        }
        i += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        path: &str,
        status: FileStatus,
        staged: bool,
        orig: Option<&str>,
    ) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            orig_path: orig.map(String::from),
            status,
            staged,
        }
    }

    // ---- parse_status ----

    #[test]
    fn status_empty_input_yields_nothing() {
        assert_eq!(parse_status(""), Vec::<FileEntry>::new());
    }

    #[test]
    fn status_single_unstaged_modification() {
        // XY = ".M" → only worktree changed
        let input = "1 .M N... 100644 100644 100644 a b src/foo.rs\0";
        assert_eq!(
            parse_status(input),
            vec![entry("src/foo.rs", FileStatus::Modified, false, None)],
        );
    }

    #[test]
    fn status_single_staged_addition() {
        // XY = "A." → only index changed
        let input = "1 A. N... 000000 100644 100644 a b src/new.rs\0";
        assert_eq!(
            parse_status(input),
            vec![entry("src/new.rs", FileStatus::Added, true, None)],
        );
    }

    #[test]
    fn status_both_staged_and_unstaged_splits_into_two_entries() {
        // XY = "MM" → modified in index AND in worktree
        let input = "1 MM N... 100644 100644 100644 a b src/baz.rs\0";
        assert_eq!(
            parse_status(input),
            vec![
                entry("src/baz.rs", FileStatus::Modified, true, None),
                entry("src/baz.rs", FileStatus::Modified, false, None),
            ],
        );
    }

    #[test]
    fn status_staged_deletion() {
        let input = "1 D. N... 100644 000000 000000 a b gone.txt\0";
        assert_eq!(
            parse_status(input),
            vec![entry("gone.txt", FileStatus::Deleted, true, None)],
        );
    }

    #[test]
    fn status_rename_emits_single_entry_with_orig_path() {
        // Type 2: "R." with score, then path NUL origPath NUL
        let input = "2 R. N... 100644 100644 100644 a b R100 src/new.rs\0src/old.rs\0";
        assert_eq!(
            parse_status(input),
            vec![entry("src/new.rs", FileStatus::Renamed, true, Some("src/old.rs"))],
        );
    }

    #[test]
    fn status_conflict_renders_as_conflicted() {
        let input = "u UU N... 100644 100644 100644 100644 a b c src/clash.rs\0";
        assert_eq!(
            parse_status(input),
            vec![entry("src/clash.rs", FileStatus::Conflicted, false, None)],
        );
    }

    #[test]
    fn status_untracked_file() {
        let input = "? scratch.txt\0";
        assert_eq!(
            parse_status(input),
            vec![entry("scratch.txt", FileStatus::Untracked, false, None)],
        );
    }

    #[test]
    fn status_untracked_directory_keeps_trailing_slash() {
        let input = "? new-folder/\0";
        assert_eq!(
            parse_status(input),
            vec![entry("new-folder/", FileStatus::UntrackedDir, false, None)],
        );
    }

    #[test]
    fn status_path_with_spaces_handled() {
        let input = "1 .M N... 100644 100644 100644 a b a path with spaces.txt\0";
        assert_eq!(
            parse_status(input),
            vec![entry(
                "a path with spaces.txt",
                FileStatus::Modified,
                false,
                None,
            )],
        );
    }

    #[test]
    fn status_multiple_entries_in_order() {
        let input = "1 .M N... 100644 100644 100644 a b a.rs\0\
                     1 A. N... 000000 100644 100644 a b b.rs\0\
                     ? c.txt\0";
        assert_eq!(
            parse_status(input),
            vec![
                entry("a.rs", FileStatus::Modified, false, None),
                entry("b.rs", FileStatus::Added, true, None),
                entry("c.txt", FileStatus::Untracked, false, None),
            ],
        );
    }

    #[test]
    fn status_ignored_entries_are_dropped() {
        // '!' entries should not surface.
        let input = "! target/build.log\0? real.txt\0";
        assert_eq!(
            parse_status(input),
            vec![entry("real.txt", FileStatus::Untracked, false, None)],
        );
    }

    // ---- parse_numstat ----

    #[test]
    fn numstat_empty_input() {
        assert_eq!(parse_numstat(""), HashMap::new());
    }

    #[test]
    fn numstat_simple_adds_and_dels() {
        // -z numstat format: "adds\tdels\tpath\0"
        let input = "12\t3\tsrc/foo.rs\0";
        let result = parse_numstat(input);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result.get("src/foo.rs"),
            Some(&NumStat {
                adds: 12,
                dels: 3,
                binary: false,
            }),
        );
    }

    #[test]
    fn numstat_binary_files_marked() {
        let input = "-\t-\tassets/logo.png\0";
        let result = parse_numstat(input);
        assert_eq!(
            result.get("assets/logo.png"),
            Some(&NumStat {
                adds: 0,
                dels: 0,
                binary: true,
            }),
        );
    }

    #[test]
    fn numstat_rename_keyed_on_new_path() {
        // Rename in -z numstat: "adds\tdels\t\0origPath\0newPath\0"
        let input = "5\t2\t\0src/old.rs\0src/new.rs\0";
        let result = parse_numstat(input);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result.get("src/new.rs"),
            Some(&NumStat {
                adds: 5,
                dels: 2,
                binary: false,
            }),
        );
    }

    #[test]
    fn numstat_multiple_entries() {
        let input = "1\t1\ta.rs\0\
                     20\t5\tb.rs\0\
                     -\t-\timg.png\0";
        let result = parse_numstat(input);
        assert_eq!(result.len(), 3);
        assert_eq!(result.get("a.rs").map(|n| n.adds), Some(1));
        assert_eq!(result.get("b.rs").map(|n| n.adds), Some(20));
        assert_eq!(result.get("img.png").map(|n| n.binary), Some(true));
    }
}
