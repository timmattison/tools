//! The commands `pbcopy` and `pbpaste`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::ClipboardError;

pub(crate) struct Commands {
    search_path: Option<OsString>,
}

impl Commands {
    pub(crate) fn on(search_path: Option<OsString>) -> Self {
        Self { search_path }
    }

    pub(crate) fn none() -> Self {
        Self { search_path: None }
    }

    pub(crate) fn find(&self, _name: &str) -> Option<PathBuf> {
        let _ = &self.search_path;
        None
    }
}

pub(crate) fn copy(_program: &Path, _text: &str) -> Result<(), ClipboardError> {
    Err(ClipboardError::new(&"placeholder"))
}

pub(crate) fn paste(_program: &Path) -> Result<String, ClipboardError> {
    Err(ClipboardError::new(&"placeholder"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// Writes a file in `dir` with `mode`.
    #[cfg(unix)]
    fn stub(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    #[cfg(unix)]
    fn joined(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn find_takes_the_first_match_on_the_path() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let wanted = stub(first.path(), "pbcopy", 0o755);
        stub(second.path(), "pbcopy", 0o755);
        let commands = Commands::on(Some(joined(&[first.path(), second.path()])));
        assert_eq!(commands.find("pbcopy"), Some(wanted));
    }

    #[cfg(unix)]
    #[test]
    fn find_skips_a_file_that_is_not_executable() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        stub(first.path(), "pbcopy", 0o644);
        let wanted = stub(second.path(), "pbcopy", 0o755);
        let commands = Commands::on(Some(joined(&[first.path(), second.path()])));
        assert_eq!(commands.find("pbcopy"), Some(wanted));
    }

    #[cfg(unix)]
    #[test]
    fn find_skips_a_directory_with_the_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("pbcopy")).unwrap();
        let commands = Commands::on(Some(joined(&[dir.path()])));
        assert_eq!(commands.find("pbcopy"), None);
    }

    #[cfg(unix)]
    #[test]
    fn find_skips_a_relative_and_an_empty_entry() {
        let dir = tempfile::tempdir().unwrap();
        stub(dir.path(), "pbcopy", 0o755);
        let relative = OsString::from(format!(
            ":relative/dir:{}",
            dir.path().strip_prefix("/").unwrap().display()
        ));
        let commands = Commands::on(Some(relative));
        assert_eq!(commands.find("pbcopy"), None);
    }

    #[test]
    fn find_finds_nothing_on_a_missing_or_empty_path() {
        assert_eq!(Commands::on(None).find("pbcopy"), None);
        assert_eq!(Commands::on(Some(OsString::new())).find("pbcopy"), None);
        assert_eq!(Commands::none().find("pbcopy"), None);
    }
}
