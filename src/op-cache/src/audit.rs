//! The audit trail: a JSON Lines file that records each run of `op`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// `<state home>/op-cache/audit.jsonl`, or `None` when no state home resolves.
///
/// `XDG_STATE_HOME` wins when it is set, not empty, and an absolute path.
/// Otherwise `$HOME/.local/state` is the state home, when `HOME` is set and
/// not empty. This function is pure: it never reads the process environment.
pub(crate) fn audit_path(xdg_state_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let state_home = match xdg_state_home.map(Path::new) {
        Some(xdg) if xdg.is_absolute() => xdg.to_path_buf(),
        _ => {
            let home = home.filter(|value| !value.is_empty())?;
            Path::new(home).join(".local").join("state")
        }
    };
    Some(state_home.join("op-cache").join("audit.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::Path;

    fn os(text: &str) -> Option<&OsStr> {
        Some(OsStr::new(text))
    }

    #[test]
    fn an_absolute_xdg_state_home_wins() {
        assert_eq!(
            audit_path(os("/xdg/state"), os("/home/tim")),
            Some(Path::new("/xdg/state/op-cache/audit.jsonl").to_path_buf())
        );
    }

    #[test]
    fn an_empty_xdg_state_home_falls_back_to_home() {
        assert_eq!(
            audit_path(os(""), os("/home/tim")),
            Some(Path::new("/home/tim/.local/state/op-cache/audit.jsonl").to_path_buf())
        );
    }

    #[test]
    fn a_relative_xdg_state_home_falls_back_to_home() {
        assert_eq!(
            audit_path(os("state"), os("/home/tim")),
            Some(Path::new("/home/tim/.local/state/op-cache/audit.jsonl").to_path_buf())
        );
    }

    #[test]
    fn an_unset_xdg_state_home_falls_back_to_home() {
        assert_eq!(
            audit_path(None, os("/home/tim")),
            Some(Path::new("/home/tim/.local/state/op-cache/audit.jsonl").to_path_buf())
        );
    }

    #[test]
    fn no_usable_xdg_and_no_home_gives_none() {
        assert_eq!(audit_path(None, None), None);
        assert_eq!(audit_path(os(""), os("")), None);
        assert_eq!(audit_path(os("relative"), None), None);
    }
}
