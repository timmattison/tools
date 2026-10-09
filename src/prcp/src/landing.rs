//! The landing preview: where each source of a recursive run lands, shown before the first copy.
//!
//! # Why
//!
//! With `-R`, the place where a tree lands depends on the destination. An
//! existing directory takes the tree under the name of the source. A missing
//! destination becomes the copy itself. A slash at the end of the source
//! changes nothing, but `rsync` and BSD `cp` read that slash as "only the
//! contents". A person with those habits can thus send a tree one level too
//! deep. The preview shows the result of these rules before any byte moves.
//!
//! # Rules
//!
//! A plan gets a preview when it holds at least one directory source. The
//! preview lists every source in the order given, also the files and symlinks
//! beside the directory. Each line shows the source as the user typed it, and
//! the absolute path that it becomes.
//!
//! The absolute path resolves every symlink in the directories above the
//! landing place. It thus names the real directory that receives the files. A
//! part of the path that does not exist yet stays as written. The preview
//! reads the file system, but it makes and changes nothing.
//!
//! A directory source also shows if its landing directory is new or already
//! exists, how many files it holds, and one example file. The example is the
//! file nearest the top of the tree, and the first one in plan order when
//! more than one is that near. More than [`MAX_LISTED_SOURCES`] sources show
//! as one count after the first lines. They all land in the same directory,
//! because more than one source makes the destination a container.

use crate::plan::{canonicalize_lenient, CopyPlan, EntryKind, Operand, OperandKind, PlanEntry};
use std::fmt::Write as _;
use std::fs;
use std::path::{is_separator, Path, PathBuf, MAIN_SEPARATOR};

/// What the run does to its sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Copy the sources and keep them.
    Copy,
    /// Copy the sources, then remove them through the move gate.
    Move,
}

impl Action {
    /// Return the verb of the action, as the headline of the preview uses it.
    fn verb(self) -> &'static str {
        match self {
            Action::Copy => "copy",
            Action::Move => "move",
        }
    }
}

/// The most sources that the preview lists one by one.
const MAX_LISTED_SOURCES: usize = 20;

/// The note for a directory source that ends with a slash or with `/.`.
const SLASH_NOTE: &str = "Note: a '/' or a '/.' at the end of a source changes nothing.\n      \
                          prcp copies the directory itself, not only its contents.\n";

/// The files of one source: how many there are, and the one that the preview shows.
#[derive(Debug, Clone, Default)]
struct FileTally<'plan> {
    count: usize,
    example: Option<&'plan PlanEntry>,
}

impl<'plan> FileTally<'plan> {
    /// Count one File entry, and keep it as the example when it is nearer the top.
    fn add(&mut self, entry: &'plan PlanEntry) {
        self.count += 1;
        let nearer = self.example.is_none_or(|best| depth(entry) < depth(best));
        if nearer {
            self.example = Some(entry);
        }
    }
}

/// Return the number of components in the destination of an entry.
fn depth(entry: &PlanEntry) -> usize {
    entry.destination.components().count()
}

/// Describe where each source of `plan` lands. See the module docs for the rules.
///
/// # Returns
/// The text of the preview, each line with a newline at its end. `None` when
/// the plan holds no directory source.
pub(crate) fn describe(plan: &CopyPlan, action: Action) -> Option<String> {
    let operands = plan.operands();
    if !operands
        .iter()
        .any(|operand| operand.kind == OperandKind::Directory)
    {
        return None;
    }

    let mut tallies = vec![FileTally::default(); operands.len()];
    for entry in plan.entries() {
        if entry.kind != EntryKind::File {
            continue;
        }
        if let Some(tally) = tallies.get_mut(entry.operand.index()) {
            tally.add(entry);
        }
    }

    let mut text = format!("prcp will {}:\n", action.verb());
    for (operand, tally) in operands.iter().zip(&tallies).take(MAX_LISTED_SOURCES) {
        push_operand(&mut text, operand, tally);
    }
    if let Some(first_unlisted) = operands.get(MAX_LISTED_SOURCES) {
        let unlisted = operands.len() - MAX_LISTED_SOURCES;
        let landing = shown(&first_unlisted.destination);
        let container = landing.parent().unwrap_or(&landing);
        let _ = writeln!(
            text,
            "  ... and {unlisted} more, each into {}",
            as_directory(container)
        );
    }
    if operands
        .iter()
        .any(|operand| operand.kind == OperandKind::Directory && ends_with_a_slash(&operand.source))
    {
        text.push_str(SLASH_NOTE);
    }
    Some(text)
}

/// Add the line of one source, and the line of its example file when it has one.
fn push_operand(text: &mut String, operand: &Operand, tally: &FileTally<'_>) {
    let exists = fs::symlink_metadata(&operand.destination).is_ok();
    let landing = shown(&operand.destination);
    let (landing, detail) = match operand.kind {
        OperandKind::Directory => {
            let detail = if exists {
                format!("existing directory, {}", merging(tally.count))
            } else {
                format!("new directory, {}", files(tally.count))
            };
            (as_directory(&landing), detail)
        }
        OperandKind::File => (landing.display().to_string(), leaf_detail("file", exists)),
        OperandKind::Symlink => (
            landing.display().to_string(),
            leaf_detail("symlink", exists),
        ),
    };
    let _ = writeln!(
        text,
        "  {} -> {landing}  ({detail})",
        operand.source.display()
    );
    // A file source is its own example, so only a directory source shows one.
    let example = tally
        .example
        .filter(|_| operand.kind == OperandKind::Directory);
    if let Some(example) = example {
        let _ = writeln!(
            text,
            "    for example: {} -> {}",
            example.source.display(),
            shown(&example.destination).display()
        );
    }
}

/// Describe a file or symlink source: new, or onto a destination that exists.
fn leaf_detail(kind: &str, exists: bool) -> String {
    if exists {
        format!("{kind}, the destination exists")
    } else {
        format!("new {kind}")
    }
}

/// Count files: `1 file`, `2 files`.
fn files(count: usize) -> String {
    if count == 1 {
        "1 file".to_string()
    } else {
        format!("{count} files")
    }
}

/// Count the files that go into an existing directory: `1 file merges into it`.
fn merging(count: usize) -> String {
    if count == 1 {
        "1 file merges into it".to_string()
    } else {
        format!("{count} files merge into it")
    }
}

/// Return the absolute path that `path` names, with every symlink above it resolved.
///
/// The function resolves the parent directory, and keeps the last component as
/// written. A destination that is itself a symlink thus shows as the link, and
/// not as its target. When the path has no last component, or when the parent
/// cannot be resolved, the function returns the absolute form of the path.
fn shown(path: &Path) -> PathBuf {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    match (path.file_name(), canonicalize_lenient(parent)) {
        (Some(name), Ok(resolved)) => resolved.join(name),
        _ => std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
    }
}

/// Show `path` as a directory: its text with one separator at the end.
fn as_directory(path: &Path) -> String {
    let mut text = path.display().to_string();
    if !text.ends_with(is_separator) {
        text.push(MAIN_SEPARATOR);
    }
    text
}

/// Return true when `source` ends with a separator, or with a separator and a `.`.
///
/// `rsync` and BSD `cp` read either ending as "only the contents of the
/// directory". `prcp` copies the directory itself.
fn ends_with_a_slash(source: &Path) -> bool {
    let text = source.to_string_lossy();
    let text = text
        .strip_suffix('.')
        .filter(|rest| rest.ends_with(is_separator))
        .unwrap_or(&text);
    text.ends_with(is_separator)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests use unwrap for brevity and clear failure messages"
)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    /// Make a file with the given text. Make the parent directories first.
    fn write_file(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    /// Make a tree with one file at the top and one file in a subdirectory.
    fn make_tree(root: &Path) {
        write_file(&root.join("one.txt"), "1");
        write_file(&root.join("sub").join("two.txt"), "2");
    }

    /// Return the temporary directory with every symlink resolved, as the preview shows it.
    fn real(temp: &TempDir) -> PathBuf {
        fs::canonicalize(temp.path()).unwrap()
    }

    /// Join lines into the text of a preview. Each line ends with a newline.
    fn text(lines: &[String]) -> String {
        lines.iter().map(|line| format!("{line}\n")).collect()
    }

    /// Build the plan for one source and describe it as a copy.
    fn describe_copy(source: &Path, destination: &Path) -> Option<String> {
        let plan = CopyPlan::build(&[source.to_path_buf()], destination, true).unwrap();
        describe(&plan, Action::Copy)
    }

    #[test]
    fn a_plan_without_a_directory_source_gives_no_preview() {
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("a.txt");
        let second = temp.path().join("b.txt");
        write_file(&first, "a");
        write_file(&second, "b");
        let dest = temp.path().join("dest");

        let plan = CopyPlan::build(&[first, second], &dest, true).unwrap();

        assert_eq!(describe(&plan, Action::Copy), None);
    }

    #[test]
    fn a_directory_into_an_existing_directory_lands_under_its_name() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        make_tree(&src);
        let dest = temp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let landing = real(&temp).join("dest").join("src");

        let preview = describe_copy(&src, &dest);

        assert_eq!(
            preview.as_deref(),
            Some(
                text(&[
                    "prcp will copy:".to_string(),
                    format!(
                        "  {} -> {}/  (new directory, 2 files)",
                        src.display(),
                        landing.display()
                    ),
                    format!(
                        "    for example: {} -> {}",
                        src.join("one.txt").display(),
                        landing.join("one.txt").display()
                    ),
                ])
                .as_str()
            )
        );
    }

    #[test]
    fn a_directory_to_a_missing_destination_becomes_the_destination() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        make_tree(&src);
        let dest = temp.path().join("dest");
        let landing = real(&temp).join("dest");

        let preview = describe_copy(&src, &dest);

        assert_eq!(
            preview.as_deref(),
            Some(
                text(&[
                    "prcp will copy:".to_string(),
                    format!(
                        "  {} -> {}/  (new directory, 2 files)",
                        src.display(),
                        landing.display()
                    ),
                    format!(
                        "    for example: {} -> {}",
                        src.join("one.txt").display(),
                        landing.join("one.txt").display()
                    ),
                ])
                .as_str()
            )
        );
    }

    #[test]
    fn a_directory_that_exists_at_the_destination_takes_the_files_into_it() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        make_tree(&src);
        let dest = temp.path().join("dest");
        fs::create_dir_all(dest.join("src")).unwrap();

        let preview = describe_copy(&src, &dest).unwrap();

        let line = format!(
            "  {} -> {}/  (existing directory, 2 files merge into it)\n",
            src.display(),
            real(&temp).join("dest").join("src").display()
        );
        assert!(preview.contains(&line), "preview: {preview}");
    }

    #[test]
    fn a_move_says_that_it_moves() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        make_tree(&src);
        let dest = temp.path().join("dest");

        let plan = CopyPlan::build(std::slice::from_ref(&src), &dest, true).unwrap();
        let preview = describe(&plan, Action::Move).unwrap();

        assert!(
            preview.starts_with("prcp will move:\n"),
            "preview: {preview}"
        );
    }

    /// The note that a preview adds for a directory source with a slash or a `/.` at its end.
    const SLASH_NOTE: &str =
        "Note: a '/' or a '/.' at the end of a source changes nothing.\n      \
                              prcp copies the directory itself, not only its contents.\n";

    #[test]
    fn a_slash_at_the_end_of_a_directory_source_adds_the_note() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        make_tree(&src);
        let dest = temp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let slashed = src.join("");
        assert!(slashed.to_string_lossy().ends_with('/'));

        let preview = describe_copy(&slashed, &dest).unwrap();

        assert!(preview.ends_with(SLASH_NOTE), "preview: {preview}");
        let line = format!(
            "  {} -> {}/  (new directory, 2 files)\n",
            slashed.display(),
            real(&temp).join("dest").join("src").display()
        );
        assert!(preview.contains(&line), "preview: {preview}");
    }

    #[test]
    fn a_slash_dot_at_the_end_of_a_directory_source_adds_the_note() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        make_tree(&src);
        let dest = temp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let mut dotted = src.clone().into_os_string();
        dotted.push("/.");
        let dotted = PathBuf::from(dotted);

        let preview = describe_copy(&dotted, &dest).unwrap();

        assert!(preview.ends_with(SLASH_NOTE), "preview: {preview}");
        let landing = format!("{}/  (", real(&temp).join("dest").join("src").display());
        assert!(preview.contains(&landing), "preview: {preview}");
    }

    #[test]
    fn a_directory_source_without_a_slash_has_no_note() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        make_tree(&src);
        let dest = temp.path().join("dest");

        let preview = describe_copy(&src, &dest).unwrap();

        assert!(!preview.contains("Note:"), "preview: {preview}");
    }

    #[test]
    fn the_example_is_the_file_nearest_the_top_of_the_tree() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("a").join("b").join("deep.txt"), "d");
        write_file(&src.join("z.txt"), "z");
        let dest = temp.path().join("dest");

        let preview = describe_copy(&src, &dest).unwrap();

        let example = format!(
            "    for example: {} -> {}\n",
            src.join("z.txt").display(),
            real(&temp).join("dest").join("z.txt").display()
        );
        assert!(preview.ends_with(&example), "preview: {preview}");
    }

    #[test]
    fn an_empty_directory_has_no_example() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        fs::create_dir_all(src.join("empty")).unwrap();
        let dest = temp.path().join("dest");

        let preview = describe_copy(&src, &dest).unwrap();

        assert_eq!(
            preview,
            text(&[
                "prcp will copy:".to_string(),
                format!(
                    "  {} -> {}/  (new directory, 0 files)",
                    src.display(),
                    real(&temp).join("dest").display()
                ),
            ])
        );
    }

    #[test]
    fn one_file_is_counted_in_the_singular() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("only.txt"), "o");
        let dest = temp.path().join("dest");

        let preview = describe_copy(&src, &dest).unwrap();

        assert!(
            preview.contains("(new directory, 1 file)\n"),
            "preview: {preview}"
        );
    }

    #[test]
    fn files_beside_a_directory_are_listed_in_the_order_given() {
        let temp = TempDir::new().unwrap();
        let fresh = temp.path().join("fresh.txt");
        let old = temp.path().join("old.txt");
        write_file(&fresh, "f");
        write_file(&old, "o");
        let src = temp.path().join("src");
        make_tree(&src);
        let dest = temp.path().join("dest");
        write_file(&dest.join("old.txt"), "already there");
        let landing = real(&temp).join("dest");

        let plan =
            CopyPlan::build(&[fresh.clone(), src.clone(), old.clone()], &dest, true).unwrap();
        let preview = describe(&plan, Action::Copy).unwrap();

        assert_eq!(
            preview,
            text(&[
                "prcp will copy:".to_string(),
                format!(
                    "  {} -> {}  (new file)",
                    fresh.display(),
                    landing.join("fresh.txt").display()
                ),
                format!(
                    "  {} -> {}/  (new directory, 2 files)",
                    src.display(),
                    landing.join("src").display()
                ),
                format!(
                    "    for example: {} -> {}",
                    src.join("one.txt").display(),
                    landing.join("src").join("one.txt").display()
                ),
                format!(
                    "  {} -> {}  (file, the destination exists)",
                    old.display(),
                    landing.join("old.txt").display()
                ),
            ])
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_source_beside_a_directory_is_listed_as_a_symlink() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let src = temp.path().join("src");
        make_tree(&src);
        let dest = temp.path().join("dest");

        let plan = CopyPlan::build(&[link.clone(), src], &dest, true).unwrap();
        let preview = describe(&plan, Action::Copy).unwrap();

        let line = format!(
            "  {} -> {}  (new symlink)\n",
            link.display(),
            real(&temp).join("dest").join("link").display()
        );
        assert!(preview.contains(&line), "preview: {preview}");
    }

    #[cfg(unix)]
    #[test]
    fn a_destination_behind_a_symlink_shows_the_real_directory() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        make_tree(&src);
        let real_dest = temp.path().join("real-dest");
        fs::create_dir(&real_dest).unwrap();
        let dest = temp.path().join("dest");
        std::os::unix::fs::symlink(&real_dest, &dest).unwrap();

        let preview = describe_copy(&src, &dest).unwrap();

        let line = format!(
            "  {} -> {}/  (new directory, 2 files)\n",
            src.display(),
            real(&temp).join("real-dest").join("src").display()
        );
        assert!(preview.contains(&line), "preview: {preview}");
    }

    #[test]
    fn a_relative_destination_shows_as_an_absolute_path() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        make_tree(&src);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let relative = PathBuf::from(format!(
            "prcp-landing-missing-{}-{nanos}",
            std::process::id()
        ));
        assert!(!relative.exists());
        let cwd = fs::canonicalize(std::env::current_dir().unwrap()).unwrap();

        let preview = describe_copy(&src, &relative).unwrap();

        let line = format!(
            "  {} -> {}/  (new directory, 2 files)\n",
            src.display(),
            cwd.join(&relative).display()
        );
        assert!(preview.contains(&line), "preview: {preview}");
        assert!(!relative.exists(), "a preview must make nothing");
    }

    #[test]
    fn multibyte_names_show_whole() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("日本語");
        write_file(&src.join("café 🎉.txt"), "c");
        let dest = temp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let landing = real(&temp).join("dest").join("日本語");

        let preview = describe_copy(&src, &dest).unwrap();

        assert_eq!(
            preview,
            text(&[
                "prcp will copy:".to_string(),
                format!(
                    "  {} -> {}/  (new directory, 1 file)",
                    src.display(),
                    landing.display()
                ),
                format!(
                    "    for example: {} -> {}",
                    src.join("café 🎉.txt").display(),
                    landing.join("café 🎉.txt").display()
                ),
            ])
        );
    }

    #[test]
    fn more_than_twenty_sources_list_twenty_and_count_the_rest() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        make_tree(&src);
        let mut sources = vec![src.clone()];
        for index in 1..=24 {
            let file = temp.path().join(format!("f{index:02}.txt"));
            write_file(&file, "f");
            sources.push(file);
        }
        let dest = temp.path().join("dest");
        let landing = real(&temp).join("dest");

        let plan = CopyPlan::build(&sources, &dest, true).unwrap();
        let preview = describe(&plan, Action::Copy).unwrap();

        let mut lines = vec![
            "prcp will copy:".to_string(),
            format!(
                "  {} -> {}/  (new directory, 2 files)",
                src.display(),
                landing.join("src").display()
            ),
            format!(
                "    for example: {} -> {}",
                src.join("one.txt").display(),
                landing.join("src").join("one.txt").display()
            ),
        ];
        for file in &sources[1..20] {
            let name = file.file_name().unwrap();
            lines.push(format!(
                "  {} -> {}  (new file)",
                file.display(),
                landing.join(name).display()
            ));
        }
        lines.push(format!(
            "  ... and 5 more, each into {}/",
            landing.display()
        ));
        assert_eq!(preview, text(&lines));
    }

    #[test]
    fn a_preview_makes_and_changes_nothing() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("src");
        make_tree(&src);
        let dest = temp.path().join("dest");

        let preview = describe_copy(&src, &dest);

        assert!(preview.is_some());
        assert!(!dest.exists(), "a preview must make nothing");
    }
}
