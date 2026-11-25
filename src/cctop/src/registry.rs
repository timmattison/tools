//! The session registry of Claude Code. cctop reads the name of a session
//! from it.

use std::path::Path;

use proctree::Pid;

/// The name of the session of `pid`, from `<sessions_dir>/<pid>.json`.
///
/// The result is `None` when the file is missing, when it is not a JSON
/// object, or when its `name` is not a string with text in it.
#[must_use]
pub fn session_name(sessions_dir: &Path, pid: Pid) -> Option<String> {
    let path = sessions_dir.join(format!("{}.json", pid.as_u32()));
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let name = value.get("name")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/registry-session.json");

    fn write(dir: &Path, pid: u32, text: &str) {
        std::fs::write(dir.join(format!("{pid}.json")), text).unwrap();
    }

    #[test]
    fn reads_the_name_of_the_fixture() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), 80033, FIXTURE);
        assert_eq!(
            session_name(dir.path(), Pid::new(80033)).as_deref(),
            Some("issue-697-d1")
        );
    }

    #[test]
    fn trims_the_name() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), 7, r#"{"name":"  日本語 café \n"}"#);
        assert_eq!(
            session_name(dir.path(), Pid::new(7)).as_deref(),
            Some("日本語 café")
        );
    }

    #[test]
    fn gives_none_for_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(session_name(dir.path(), Pid::new(9)), None);
    }

    #[test]
    fn gives_none_for_bad_json_or_a_bad_name() {
        let dir = tempfile::tempdir().unwrap();
        for (pid, text) in [
            (1, "not json"),
            (2, r#"{"pid":2}"#),
            (3, r#"{"name":42}"#),
            (4, r#"{"name":null}"#),
            (5, r#"{"name":"   "}"#),
            (6, r#"{"name":""}"#),
            (8, r#"["name"]"#),
        ] {
            write(dir.path(), pid, text);
            assert_eq!(session_name(dir.path(), Pid::new(pid)), None, "{text}");
        }
    }
}
