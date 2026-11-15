//! The worktree that a compiler builds for.
//!
//! The working directory of a `clang` that `cargo` started is the source of
//! the crate in `~/.cargo/registry`, not the worktree. Only the output path in
//! the arguments (`-o <worktree>/target/...`) names the worktree. Thus this
//! module reads the arguments, and it never reads the working directory.
//!
//! The rules are pure. The caller gives a function that tells whether a
//! directory holds a `.git` entry, so a test needs no file system.

use std::path::{Component, Path, PathBuf};

/// The text for a compiler that builds for no worktree.
pub const NO_WORKTREE: &str = "no worktree";

/// The word that gives the output file to a compiler.
const OUTPUT_FLAG: &str = "-o";

/// The word that gives the output directory to `rustc`.
const OUT_DIR_FLAG: &str = "--out-dir";

/// The joined form of the output directory: `--out-dir=<path>`.
const OUT_DIR_JOINED: &str = "--out-dir=";

/// The character that starts an absolute path.
const ROOT: char = '/';

/// The character that joins a key to its value, as in `dependency=/x`.
const KEY_VALUE_JOINT: char = '=';

/// The number of parts of a worktree path that [`label`] keeps.
const LABEL_PARTS: usize = 2;

/// Gives the worktree that a compiler builds for, from its argument text.
///
/// `arguments` is one line of `ps -o args=`: the arguments joined by spaces.
/// `holds_git(dir)` is true when `dir/.git` exists, as a file or as a
/// directory.
///
/// The candidate paths are, in this order:
///
/// 1. the value of `-o`, as a separate word or joined (`-o/abs/x.o`);
/// 2. the value of `--out-dir`, as a separate word or joined with `=`;
/// 3. each other absolute path from left to right, the name of the program
///    included: a word that starts with `/`, or the part after the first `=`
///    of a word when that part starts with `/`.
///
/// Only an absolute path counts, because this function never reads the
/// working directory. For each candidate, the function walks the path and its
/// parents, and it takes the first directory for which `holds_git` is true.
/// The first candidate that gives a directory wins.
///
/// This is a refinement of the rule "take the first path". When the first
/// path lies in no worktree, as the program path under
/// `/Applications/Xcode.app` does, the next candidate gets its turn. A source
/// file in `~/.cargo/registry` never decides when an output path names a
/// worktree, because the output path comes first.
///
/// Gives `None` when no candidate lies in a worktree. The caller shows
/// [`NO_WORKTREE`].
#[must_use]
pub fn worktree_of(arguments: &str, holds_git: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    candidates(arguments)
        .iter()
        .find_map(|candidate| candidate.ancestors().find(|directory| holds_git(directory)))
        .map(Path::to_path_buf)
}

/// Gives the absolute paths of `arguments` in the order of the rules of
/// [`worktree_of`].
fn candidates(arguments: &str) -> Vec<PathBuf> {
    let words: Vec<&str> = arguments.split_ascii_whitespace().collect();
    let mut found: Vec<&str> = Vec::new();

    for (index, word) in words.iter().enumerate() {
        if *word == OUTPUT_FLAG {
            found.extend(words.get(index + 1).copied());
        } else if let Some(joined) = word.strip_prefix(OUTPUT_FLAG) {
            found.push(joined);
        }
    }
    for (index, word) in words.iter().enumerate() {
        if *word == OUT_DIR_FLAG {
            found.extend(words.get(index + 1).copied());
        } else if let Some(joined) = word.strip_prefix(OUT_DIR_JOINED) {
            found.push(joined);
        }
    }
    for word in &words {
        found.push(word);
        if let Some((_, value)) = word.split_once(KEY_VALUE_JOINT) {
            found.push(value);
        }
    }

    found
        .into_iter()
        .filter(|path| path.starts_with(ROOT))
        .map(PathBuf::from)
        .collect()
}

/// Gives the last two parts of a worktree path, for a person to read:
/// `prediction-markets-worktrees/issue-17`.
///
/// A path with one part gives that part. The path `/` gives `/`. The function
/// works on whole parts, so a part with multi-byte characters stays intact.
#[must_use]
pub fn label(worktree: &Path) -> String {
    let parts: Vec<String> = worktree
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let first = parts.len().saturating_sub(LABEL_PARTS);
    if parts.is_empty() {
        return worktree.to_string_lossy().into_owned();
    }
    parts[first..].join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The worktree of the 2026-11-15 case.
    const WORKTREE: &str = "/Volumes/x/prediction-markets-worktrees/issue-17";

    /// Gives a `holds_git` function over a fixed set of directories.
    fn git_in(directories: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = directories.iter().map(PathBuf::from).collect();
        move |directory| set.contains(directory)
    }

    /// Finds the worktree of `arguments` when only [`WORKTREE`] and
    /// `/Users/u/repo` hold a `.git` entry.
    fn find(arguments: &str) -> Option<PathBuf> {
        worktree_of(arguments, git_in(&[WORKTREE, "/Users/u/repo"]))
    }

    fn worktree() -> Option<PathBuf> {
        Some(PathBuf::from(WORKTREE))
    }

    /// An `-o` path in `target/` finds the worktree by walking up to the
    /// first directory that holds `.git`.
    #[test]
    fn an_output_path_finds_the_worktree() {
        let arguments = "clang -c a.c -o /Volumes/x/prediction-markets-worktrees/issue-17/target/debug/build/libduckdb-sys-1/out/a.o";
        assert_eq!(find(arguments), worktree());
    }

    /// The joined form `-o/abs` works as the separate form does.
    #[test]
    fn the_joined_output_form_finds_the_worktree() {
        let arguments = format!("clang -c a.c -o{WORKTREE}/target/a.o");
        assert_eq!(find(&arguments), worktree());
    }

    /// `--out-dir` gives the output directory of `rustc`, in both forms.
    #[test]
    fn an_out_dir_finds_the_worktree() {
        let separate = format!("rustc --out-dir {WORKTREE}/target/debug/deps lib.rs");
        let joined = format!("rustc --out-dir={WORKTREE}/target/debug/deps lib.rs");
        assert_eq!(find(&separate), worktree());
        assert_eq!(find(&joined), worktree());
    }

    /// `-o` wins over `--out-dir`, and over an absolute path that comes
    /// earlier in the text.
    #[test]
    fn the_output_file_wins_over_the_other_paths() {
        let arguments =
            format!("/Users/u/repo/tool /Users/u/repo/x.c --out-dir /Users/u/repo/o -o {WORKTREE}/target/a.o");
        assert_eq!(find(&arguments), worktree());
    }

    /// Another absolute path finds the worktree when no `-o` and no
    /// `--out-dir` is present, also in the `key=/abs` form.
    #[test]
    fn another_absolute_path_finds_the_worktree() {
        let plain = format!("rustc {WORKTREE}/src/lib.rs");
        let keyed = format!("rustc -Cincremental={WORKTREE}/target/inc lib.rs");
        let dependency = format!("rustc -L dependency={WORKTREE}/target/debug/deps lib.rs");
        assert_eq!(find(&plain), worktree());
        assert_eq!(find(&keyed), worktree());
        assert_eq!(find(&dependency), worktree());
    }

    /// A program under Xcode lies in no worktree, so the next path gets its
    /// turn.
    #[test]
    fn a_program_outside_any_worktree_does_not_stop_the_search() {
        let arguments = format!(
            "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang -c a.c {WORKTREE}/src/a.c"
        );
        assert_eq!(find(&arguments), worktree());
    }

    /// The path of the program is not an argument, so it never gives the
    /// worktree. Homebrew keeps its own git checkout at `/opt/homebrew`, and
    /// the `sccache` server runs as `/opt/homebrew/bin/sccache` with no
    /// argument. On 2026-11-15 that server showed as the worktree
    /// `opt/homebrew`.
    #[test]
    fn the_path_of_the_program_never_gives_the_worktree() {
        let homebrew = git_in(&["/opt/homebrew", WORKTREE]);
        assert_eq!(worktree_of("/opt/homebrew/bin/sccache", &homebrew), None);
        assert_eq!(
            worktree_of(
                &format!("/opt/homebrew/bin/clang -c a.c {WORKTREE}/src/a.c"),
                &homebrew
            ),
            worktree()
        );
    }

    /// A source file in the cargo registry never decides when an output path
    /// names a worktree.
    #[test]
    fn a_registry_source_does_not_decide() {
        let arguments = format!(
            "clang -c /Users/u/.cargo/registry/src/index/libduckdb-sys-1/a.cpp -o {WORKTREE}/target/a.o"
        );
        assert_eq!(find(&arguments), worktree());
    }

    /// A relative `-o` is skipped, because the function never reads the
    /// working directory.
    #[test]
    fn a_relative_output_path_is_skipped() {
        assert_eq!(find("clang -c a.c -o foo.o"), None);
    }

    /// Arguments with no path give no worktree.
    #[test]
    fn arguments_with_no_path_give_none() {
        assert_eq!(find("clang -c a.c -O2"), None);
        assert_eq!(find(""), None);
    }

    /// The label is the last two parts of the path.
    #[test]
    fn the_label_is_the_last_two_parts() {
        assert_eq!(
            label(Path::new(WORKTREE)),
            "prediction-markets-worktrees/issue-17"
        );
    }

    /// A path with one part gives that part, and the root gives the root.
    #[test]
    fn the_label_of_a_short_path_is_the_whole_path() {
        assert_eq!(label(Path::new("/repo")), "repo");
        assert_eq!(label(Path::new("/")), "/");
    }

    /// The parts keep their multi-byte characters.
    #[test]
    fn the_label_keeps_multibyte_parts_intact() {
        assert_eq!(label(Path::new("/x/日本語/🎉")), "日本語/🎉");
        assert_eq!(label(Path::new("/café/issue-1")), "café/issue-1");
    }
}
