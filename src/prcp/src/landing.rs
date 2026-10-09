//! The landing preview: where each source of a recursive run lands, shown before the first copy.

use crate::plan::CopyPlan;

/// What the run does to its sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Copy the sources and keep them.
    Copy,
    /// Copy the sources, then remove them through the move gate.
    Move,
}

/// Describe where each source of `plan` lands.
pub(crate) fn describe(plan: &CopyPlan, action: Action) -> Option<String> {
    let _ = (plan, action);
    None
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
    const SLASH_NOTE: &str = "Note: a '/' or a '/.' at the end of a source changes nothing.\n      \
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
        lines.push(format!("  ... and 5 more, each into {}/", landing.display()));
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
