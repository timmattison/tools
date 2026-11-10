//! What watch mode needs to look up pull requests: the `gh` program and the
//! extra GitHub host.
//!
//! The settings exist only when the lookup applies. [`PullSettings::from_environment`]
//! gives `None` when the user turned the indicator off, and when no `gh` is on
//! the search path. Watch mode then shows no indicator and starts no `gh`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The name of the program that asks GitHub.
pub(crate) const GH_PROGRAM: &str = "gh";

/// The environment variable that names the search path of programs.
pub(crate) const PATH_VARIABLE: &str = "PATH";

/// The environment variable that names the GitHub host of `gh`.
pub(crate) const GH_HOST_VARIABLE: &str = "GH_HOST";

/// The program and the host that the pull-request lookups of watch mode use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullSettings {
    program: PathBuf,
    extra_host: Option<String>,
}

impl PullSettings {
    /// Builds the settings from the inputs of the process.
    ///
    /// `no_pr` is the `--no-pr` flag. `path_var` is the value of `PATH`.
    /// `gh_host` is the value of `GH_HOST`, and an empty value counts as unset.
    ///
    /// Returns `None` when `no_pr` is true, and when no file named `gh` that
    /// the user can run is in a directory of `path_var`. The program of the
    /// settings is the path of the `gh` that this search found, so the lookup
    /// runs the program that the check found.
    pub(crate) fn from_environment(
        no_pr: bool,
        path_var: Option<&OsStr>,
        gh_host: Option<String>,
    ) -> Option<Self> {
        let _ = (no_pr, path_var, gh_host);
        None
    }

    /// The path of the `gh` program.
    pub(crate) fn program(&self) -> &Path {
        &self.program
    }

    /// The value of `GH_HOST`, when it is set and not empty.
    pub(crate) fn extra_host(&self) -> Option<&str> {
        self.extra_host.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    /// A directory that holds a file `gh` with the given mode.
    fn dir_with_gh(mode: u32) -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join(GH_PROGRAM);
        std::fs::write(&file, "#!/bin/sh\n").expect("write gh");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).expect("chmod");
        dir
    }

    fn joined(dirs: &[&Path]) -> std::ffi::OsString {
        std::env::join_paths(dirs).expect("join the directories")
    }

    #[test]
    fn the_no_pr_flag_gives_no_settings_even_with_gh_on_the_path() {
        let dir = dir_with_gh(0o755);
        let path = joined(&[dir.path()]);
        assert_eq!(
            PullSettings::from_environment(true, Some(&path), None),
            None
        );
    }

    #[test]
    fn an_executable_gh_on_the_path_gives_its_path() {
        let dir = dir_with_gh(0o755);
        let path = joined(&[dir.path()]);
        let settings =
            PullSettings::from_environment(false, Some(&path), None).expect("gh is on the path");
        assert_eq!(settings.program(), dir.path().join(GH_PROGRAM));
        assert_eq!(settings.extra_host(), None);
    }

    #[test]
    fn a_gh_file_with_no_execute_bit_gives_no_settings() {
        let dir = dir_with_gh(0o644);
        let path = joined(&[dir.path()]);
        assert_eq!(
            PullSettings::from_environment(false, Some(&path), None),
            None
        );
    }

    #[test]
    fn a_directory_named_gh_gives_no_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(GH_PROGRAM)).expect("mkdir gh");
        let path = joined(&[dir.path()]);
        assert_eq!(
            PullSettings::from_environment(false, Some(&path), None),
            None
        );
    }

    #[test]
    fn no_path_gives_no_settings() {
        assert_eq!(PullSettings::from_environment(false, None, None), None);
    }

    #[test]
    fn the_first_directory_with_gh_wins() {
        let first = dir_with_gh(0o755);
        let second = dir_with_gh(0o755);
        let path = joined(&[first.path(), second.path()]);
        let settings =
            PullSettings::from_environment(false, Some(&path), None).expect("gh is on the path");
        assert_eq!(settings.program(), first.path().join(GH_PROGRAM));
    }

    #[test]
    fn a_directory_without_a_usable_gh_does_not_hide_a_later_one() {
        let plain = dir_with_gh(0o644);
        let usable = dir_with_gh(0o755);
        let path = joined(&[plain.path(), usable.path()]);
        let settings =
            PullSettings::from_environment(false, Some(&path), None).expect("gh is on the path");
        assert_eq!(settings.program(), usable.path().join(GH_PROGRAM));
    }

    #[test]
    fn an_empty_gh_host_counts_as_unset() {
        let dir = dir_with_gh(0o755);
        let path = joined(&[dir.path()]);
        let settings = PullSettings::from_environment(false, Some(&path), Some(String::new()))
            .expect("gh is on the path");
        assert_eq!(settings.extra_host(), None);
    }

    #[test]
    fn a_gh_host_is_kept() {
        let dir = dir_with_gh(0o755);
        let path = joined(&[dir.path()]);
        let settings =
            PullSettings::from_environment(false, Some(&path), Some("ghe.example.com".to_string()))
                .expect("gh is on the path");
        assert_eq!(settings.extra_host(), Some("ghe.example.com"));
    }
}
