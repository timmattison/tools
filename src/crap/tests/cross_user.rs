//! End-to-end tests for `crap <id> --user <name>` cross-user resume, driving
//! the real binary against throwaway sibling homes.
//!
//! Each test builds a `root/` holding a fake current user (`home`) and a fake
//! sibling user (`other`) as siblings — the exact `<home>/../<name>` layout the
//! binary resolves `--user` against — sets `HOME=root/home`, and runs the
//! compiled `crap` from a fresh working directory. These exercise CLI parsing,
//! cross-user discovery, the copy-into-own-tree import, and the
//! `__CRAP_FORK_AT__` wire protocol together, which the in-crate unit tests
//! cannot reach because `run_resume` calls `exit`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

/// A foreign session id planted under another user's tree.
const FOREIGN_ID: &str = "11111111-2222-3333-4444-555555555555";
/// A session id planted under the current user's own tree.
const SELF_ID: &str = "aaaaaaaa-1111-2222-3333-444444444444";

// These mirror the binary's cross-user wire protocol (see `format_fork_at_output`
// in `main.rs`); an integration test re-states the contract it is pinning rather
// than reaching into private constants.
const FORK_AT_SENTINEL: &str = "__CRAP_FORK_AT__";
const NO_NEW_ID_SENTINEL: &str = "__CRAP_NO_NEW_ID__";

/// A process-unique temp directory, keyed on pid + nanoseconds so concurrent
/// runs of this test never share state.
fn unique_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("crap-cu-{tag}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes a transcript recording `cwd` for `id` under
/// `<projects>/<project_folder>/<id>.jsonl`.
fn plant_session(projects: &Path, project_folder: &str, id: &str, cwd: &Path) {
    let folder = projects.join(project_folder);
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join(format!("{id}.jsonl")),
        format!("{{\"cwd\":\"{}\"}}\n", cwd.display()),
    )
    .unwrap();
}

/// Runs the real `crap` binary with `HOME` set to `root/home` and the given
/// arguments, from a fresh working directory under `root`.
fn run_crap(root: &Path, args: &[&str]) -> Output {
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    Command::new(env!("CARGO_BIN_EXE_crap"))
        .env("HOME", root.join("home"))
        .current_dir(&work)
        .args(args)
        .output()
        .expect("crap binary should run")
}

#[test]
fn user_flag_cross_user_forks_at_original_dir() {
    let root = unique_root("fork-at");
    // A readable session under the sibling user `other`, whose recorded cwd is a
    // real (shared) directory both users can reach.
    let shared = root.join("shared-cwd");
    fs::create_dir_all(&shared).unwrap();
    let other_projects = root.join("other/.claude/projects");
    plant_session(&other_projects, "-proj", FOREIGN_ID, &shared);
    // The current user's own tree exists but does not contain this id.
    fs::create_dir_all(root.join("home/.claude/projects")).unwrap();
    let foreign_before =
        fs::read_to_string(other_projects.join("-proj").join(format!("{FOREIGN_ID}.jsonl")))
            .unwrap();

    let out = run_crap(&root, &[FOREIGN_ID, "--user", "other"]);
    assert!(
        out.status.success(),
        "exit {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.first().copied(), Some(FORK_AT_SENTINEL));
    assert_eq!(lines.get(1).copied(), Some(FOREIGN_ID));
    // No forced id in default cross-user resume.
    assert_eq!(lines.get(2).copied(), Some(NO_NEW_ID_SENTINEL));
    // The link field points at a real copy under the CURRENT user's tree.
    let link = Path::new(lines.get(3).copied().expect("a link field"));
    assert!(
        link.starts_with(root.join("home/.claude/projects")),
        "the copy must live under the current user's tree, got {}",
        link.display()
    );
    assert!(
        fs::symlink_metadata(link).unwrap().file_type().is_file(),
        "cross-user import must be a real copy, not a symlink"
    );
    assert_eq!(
        fs::read_to_string(link).unwrap(),
        foreign_before,
        "the copy must snapshot the foreign transcript"
    );
    // The directory field (last) is the session's original recorded cwd.
    assert_eq!(lines.get(4).copied(), Some(shared.to_str().unwrap()));

    // The foreign original was only read, never written.
    assert_eq!(
        fs::read_to_string(other_projects.join("-proj").join(format!("{FOREIGN_ID}.jsonl")))
            .unwrap(),
        foreign_before,
        "the foreign transcript must be left untouched"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn user_flag_self_resumes_in_place() {
    let root = unique_root("self");
    // `--user home` names the current account (home dir file name is "home"), so
    // it is a same-user hit: resume in place, no copy, no fork.
    let self_cwd = root.join("self-cwd");
    fs::create_dir_all(&self_cwd).unwrap();
    let home_projects = root.join("home/.claude/projects");
    plant_session(&home_projects, "-proj", SELF_ID, &self_cwd);

    let out = run_crap(&root, &[SELF_ID, "--user", "home"]);
    assert!(
        out.status.success(),
        "exit {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    // Default resume shape: `<id>\n<dir>`, no sentinel, no fork.
    assert_eq!(lines.first().copied(), Some(SELF_ID));
    assert_eq!(lines.get(1).copied(), Some(self_cwd.to_str().unwrap()));
    assert!(
        !stdout.contains(FORK_AT_SENTINEL),
        "a same-user hit must not fork: {stdout}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn user_flag_skips_current_user_tree() {
    let root = unique_root("skip-self");
    // The SAME id exists under both users, with different recorded cwds. With
    // `--user other`, only the sibling's tree is searched, so the resume must
    // land in the sibling's cwd — proving the current user's tree is skipped.
    let self_cwd = root.join("self-cwd");
    let other_cwd = root.join("other-cwd");
    fs::create_dir_all(&self_cwd).unwrap();
    fs::create_dir_all(&other_cwd).unwrap();
    plant_session(
        &root.join("home/.claude/projects"),
        "-proj",
        FOREIGN_ID,
        &self_cwd,
    );
    plant_session(
        &root.join("other/.claude/projects"),
        "-proj",
        FOREIGN_ID,
        &other_cwd,
    );

    let out = run_crap(&root, &[FOREIGN_ID, "--user", "other"]);
    assert!(
        out.status.success(),
        "exit {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.first().copied(), Some(FORK_AT_SENTINEL));
    // The directory (last field) is the SIBLING's cwd, not the current user's.
    assert_eq!(lines.get(4).copied(), Some(other_cwd.to_str().unwrap()));

    let _ = fs::remove_dir_all(&root);
}
