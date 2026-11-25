use std::path::{Path, PathBuf};

/// Expand a leading `~/` in a path to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

/// The folder that holds the attachments of Messages.
/// Returns `None` when the home directory is unknown.
pub fn default_attachments_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Library/Messages/Attachments"))
}

/// Resolve the attachment path from chat.db and keep it only when it lies
/// inside `root`. A leading `~/` expands to the home directory. The check
/// runs on canonical paths, thus `..` and symlinks resolve before it
/// compares. A missing file gives `None`, and so does a path outside `root`.
pub fn resolve_attachment(raw: &str, root: &Path) -> Option<PathBuf> {
    let path = std::fs::canonicalize(expand_tilde(raw)).ok()?;
    let root = std::fs::canonicalize(root).ok()?;
    path.starts_with(&root).then_some(path)
}

/// The largest page that a command returns.
pub const MAX_PAGE_SIZE: i64 = 500;

/// Clamp the paging arguments of the frontend. A negative limit means no
/// limit in SQLite and a negative offset means zero, so neither goes through.
/// Returns `(offset, limit)`.
pub fn page_bounds(offset: i64, limit: i64) -> (i64, i64) {
    (offset.max(0), limit.clamp(1, MAX_PAGE_SIZE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("Attachments");
        std::fs::create_dir_all(&root).expect("root");
        (dir, root)
    }

    /// Messages keeps each attachment path as `~/Library/Messages/...`.
    /// The test reads the home directory and changes no environment.
    #[test]
    fn expand_tilde_puts_the_home_directory_in_place_of_a_leading_tilde() {
        let home = dirs::home_dir().expect("home directory");
        assert_eq!(
            expand_tilde("~/Library/Messages/Attachments/ab/a.jpg"),
            home.join("Library/Messages/Attachments/ab/a.jpg")
        );
    }

    #[test]
    fn resolve_attachment_accepts_a_file_inside_the_root() {
        let (dir, root) = temp_root();
        let file = root.join("ab").join("photo.jpg");
        std::fs::create_dir_all(file.parent().expect("parent")).expect("subdir");
        std::fs::write(&file, "x").expect("write");
        let resolved = resolve_attachment(file.to_str().expect("utf-8"), &root);
        assert_eq!(resolved, Some(std::fs::canonicalize(&file).expect("canon")));
        drop(dir);
    }

    #[test]
    fn resolve_attachment_refuses_dot_dot_out_of_the_root() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("outside.txt"), "x").expect("write");
        let raw = format!("{}/../outside.txt", root.display());
        assert_eq!(resolve_attachment(&raw, &root), None);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_attachment_refuses_a_symlink_that_points_outside() {
        let (dir, root) = temp_root();
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "x").expect("write");
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");
        assert_eq!(
            resolve_attachment(link.to_str().expect("utf-8"), &root),
            None
        );
    }

    #[test]
    fn resolve_attachment_gives_none_for_a_missing_file() {
        let (_dir, root) = temp_root();
        let raw = format!("{}/missing.jpg", root.display());
        assert_eq!(resolve_attachment(&raw, &root), None);
    }

    #[test]
    fn page_bounds_clamps_both_arguments() {
        assert_eq!(page_bounds(-5, -1), (0, 1));
        assert_eq!(page_bounds(3, 0), (3, 1));
        assert_eq!(page_bounds(3, 50), (3, 50));
        assert_eq!(page_bounds(3, 100_000), (3, MAX_PAGE_SIZE));
    }
}
