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
use std::path::Path;
use std::process::{Command, Output};

/// A foreign session id planted under another user's tree.
const FOREIGN_ID: &str = "11111111-2222-3333-4444-555555555555";
/// A session id planted under the current user's own tree.
const SELF_ID: &str = "aaaaaaaa-1111-2222-3333-444444444444";
/// A well-formed session id that is never planted anywhere.
const MISSING_ID: &str = "99999999-8888-7777-6666-555555555555";

/// `crap`'s exit code for "no session with that id" (`exit_codes::SESSION_NOT_FOUND`
/// in `main.rs`); re-stated here rather than reaching into the binary's private
/// constants.
const SESSION_NOT_FOUND_EXIT: i32 = 1;

// These mirror the binary's cross-user wire protocol (see `format_fork_at_output`
// in `main.rs`); an integration test re-states the contract it is pinning rather
// than reaching into private constants.
const FORK_AT_SENTINEL: &str = "__CRAP_FORK_AT__";
const NO_NEW_ID_SENTINEL: &str = "__CRAP_NO_NEW_ID__";
/// The `--here` wire sentinel (see `format_here_output` in `main.rs`). Unlike
/// `__CRAP_FORK_AT__`, `--here` stays in the current directory, so its output
/// leads with this and carries no trailing directory field.
const HERE_SENTINEL: &str = "__CRAP_HERE__";

/// A process-unique temp directory that removes itself on drop — including when
/// a test panics — so failing runs never leak directories under `$TMPDIR`. The
/// `tag` is preserved in the directory name for debuggability while `O_EXCL`
/// creation guarantees uniqueness across concurrent runs.
fn unique_root(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("crap-cu-{tag}-"))
        .tempdir()
        .unwrap()
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

/// Sets `dir`'s permission bits, used to make a project directory owner-only and
/// to put it back afterwards.
#[cfg(unix)]
fn set_mode(dir: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(mode)).unwrap();
}

/// Asserts the guidance printed for a miss whose scan was refused entry to
/// owner-only project directories: the hedged headline, a count line naming the
/// owning account, and a copy-paste remedy ending in a resume of the copy.
///
/// `crap` only ever *prints* these commands — the assertions pin the text, and
/// nothing here (or in the binary) executes `sudo`.
fn assert_owner_only_guidance(stderr: &str) {
    // "readable" is the whole point: the scan cannot claim the id is absent from
    // a machine when it was refused entry to part of it.
    assert!(
        stderr.contains("no readable Claude session found"),
        "the headline must hedge on what was unreadable: {stderr}"
    );
    // One count line per account with skipped dirs, correctly pluralized.
    assert!(
        stderr.contains("1 project dir under user 'other' is owner-only and was skipped."),
        "must count the skipped dirs and name whose they are: {stderr}"
    );
    // The remedy names the owning account in both halves: locate, then copy.
    assert!(
        stderr.contains("sudo -u other find"),
        "must show how to locate the transcript as that account: {stderr}"
    );
    assert!(
        stderr.contains("sudo -u other cat"),
        "must show how to copy the transcript into our own tree: {stderr}"
    );
    assert!(
        stderr.contains(&format!("crap --here {MISSING_ID}")),
        "must end by resuming the copy that was just made: {stderr}"
    );
    // The copy lands in the project folder for the directory the user is standing
    // in. The test deliberately does not recompute that encoding: on macOS
    // `$TMPDIR` lives under `/var`, which `getcwd` resolves to `/private/var`, so
    // a recomputed name would not match. Pin the shape instead.
    assert!(
        stderr
            .lines()
            .any(|line| line.contains("> ~/.claude/projects/")
                && line.ends_with(&format!("/{MISSING_ID}.jsonl"))),
        "the remedy must write into our own project folder for this directory: {stderr}"
    );
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
    let tmp = unique_root("fork-at");
    let root = tmp.path();
    // A readable session under the sibling user `other`, whose recorded cwd is a
    // real (shared) directory both users can reach.
    let shared = root.join("shared-cwd");
    fs::create_dir_all(&shared).unwrap();
    let other_projects = root.join("other/.claude/projects");
    plant_session(&other_projects, "-proj", FOREIGN_ID, &shared);
    // The current user's own tree exists but does not contain this id.
    fs::create_dir_all(root.join("home/.claude/projects")).unwrap();
    let foreign_before = fs::read_to_string(
        other_projects
            .join("-proj")
            .join(format!("{FOREIGN_ID}.jsonl")),
    )
    .unwrap();

    let out = run_crap(root, &[FOREIGN_ID, "--user", "other"]);
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
        fs::read_to_string(
            other_projects
                .join("-proj")
                .join(format!("{FOREIGN_ID}.jsonl"))
        )
        .unwrap(),
        foreign_before,
        "the foreign transcript must be left untouched"
    );
}

#[test]
fn user_flag_self_resumes_in_place() {
    let tmp = unique_root("self");
    let root = tmp.path();
    // `--user home` names the current account (home dir file name is "home"), so
    // it is a same-user hit: resume in place, no copy, no fork.
    let self_cwd = root.join("self-cwd");
    fs::create_dir_all(&self_cwd).unwrap();
    let home_projects = root.join("home/.claude/projects");
    plant_session(&home_projects, "-proj", SELF_ID, &self_cwd);

    let out = run_crap(root, &[SELF_ID, "--user", "home"]);
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
}

#[test]
fn here_user_cross_user_copies_into_current_tree() {
    let tmp = unique_root("here-cross");
    let root = tmp.path();
    // A readable session under the sibling user `other`.
    let shared = root.join("shared-cwd");
    fs::create_dir_all(&shared).unwrap();
    let other_projects = root.join("other/.claude/projects");
    plant_session(&other_projects, "-proj", FOREIGN_ID, &shared);
    // Our own tree exists but does not contain this id.
    fs::create_dir_all(root.join("home/.claude/projects")).unwrap();
    let foreign_before = fs::read_to_string(
        other_projects
            .join("-proj")
            .join(format!("{FOREIGN_ID}.jsonl")),
    )
    .unwrap();

    let out = run_crap(root, &["--here", FOREIGN_ID, "--user", "other"]);
    assert!(
        out.status.success(),
        "exit {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    // `--here` stays put: the here sentinel, never the `cd`-first fork-at one.
    assert_eq!(lines.first().copied(), Some(HERE_SENTINEL));
    assert!(
        !stdout.contains(FORK_AT_SENTINEL),
        "--here must not cd into the original directory: {stdout}"
    );
    assert_eq!(lines.get(1).copied(), Some(FOREIGN_ID));
    assert_eq!(lines.get(2).copied(), Some(NO_NEW_ID_SENTINEL));
    // The link field is a real copy under the CURRENT user's tree (never a
    // symlink into the foreign home), snapshotting the foreign transcript.
    let link = Path::new(lines.get(3).copied().expect("a link field"));
    assert!(
        link.starts_with(root.join("home/.claude/projects")),
        "the copy must live under the current user's tree, got {}",
        link.display()
    );
    assert!(
        fs::symlink_metadata(link).unwrap().file_type().is_file(),
        "cross-user --here import must be a real copy, not a symlink"
    );
    assert_eq!(
        fs::read_to_string(link).unwrap(),
        foreign_before,
        "the copy must snapshot the foreign transcript"
    );
    // `--here` emits no trailing directory field (it does not cd).
    assert_eq!(lines.get(4).copied(), None);

    // The foreign original was only read, never written.
    assert_eq!(
        fs::read_to_string(
            other_projects
                .join("-proj")
                .join(format!("{FOREIGN_ID}.jsonl"))
        )
        .unwrap(),
        foreign_before,
        "the foreign transcript must be left untouched"
    );
}

#[test]
fn here_user_same_user_still_symlinks() {
    let tmp = unique_root("here-self");
    let root = tmp.path();
    // `--user home` names the current account, so `--here` is a same-user hit and
    // must still SYMLINK (regression guard), not copy. Plant the session in a
    // project folder that is not the current working directory's, so an import is
    // actually created rather than resolving in place.
    let home_projects = root.join("home/.claude/projects");
    let shared = root.join("shared-cwd");
    fs::create_dir_all(&shared).unwrap();
    plant_session(&home_projects, "-elsewhere", FOREIGN_ID, &shared);

    let out = run_crap(root, &["--here", FOREIGN_ID, "--user", "home"]);
    assert!(
        out.status.success(),
        "exit {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.first().copied(), Some(HERE_SENTINEL));
    let link = Path::new(lines.get(3).copied().expect("a link field"));
    // Same-user: a symlink, not a copy — and it resolves to the original bytes.
    assert!(
        fs::symlink_metadata(link).unwrap().file_type().is_symlink(),
        "same-user --here must symlink, got a non-symlink at {}",
        link.display()
    );
    assert_eq!(
        fs::read_to_string(link).unwrap(),
        fs::read_to_string(
            home_projects
                .join("-elsewhere")
                .join(format!("{FOREIGN_ID}.jsonl"))
        )
        .unwrap(),
        "the symlink must resolve to the original transcript"
    );
}

#[test]
fn user_flag_skips_current_user_tree() {
    let tmp = unique_root("skip-self");
    let root = tmp.path();
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

    let out = run_crap(root, &[FOREIGN_ID, "--user", "other"]);
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
}

#[test]
fn no_flag_falls_back_to_sibling_on_self_miss() {
    let tmp = unique_root("fallback");
    let root = tmp.path();
    // A readable session under sibling `other`, whose recorded cwd is a real
    // (shared) directory both users can reach. The current user's own tree
    // exists but does NOT contain this id, so a self-only search would miss it.
    let shared = root.join("shared-cwd");
    fs::create_dir_all(&shared).unwrap();
    let other_projects = root.join("other/.claude/projects");
    plant_session(&other_projects, "-proj", FOREIGN_ID, &shared);
    fs::create_dir_all(root.join("home/.claude/projects")).unwrap();

    // No `--user`: on a self-miss `crap` must automatically fall back to the
    // sibling home, copy the foreign transcript into our own tree, and fork it
    // at its original recorded directory.
    let out = run_crap(root, &[FOREIGN_ID]);
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
    assert_eq!(lines.get(2).copied(), Some(NO_NEW_ID_SENTINEL));
    // The copy lives under the CURRENT user's tree, and is a real file.
    let link = Path::new(lines.get(3).copied().expect("a link field"));
    assert!(
        link.starts_with(root.join("home/.claude/projects")),
        "the copy must live under the current user's tree, got {}",
        link.display()
    );
    assert!(fs::symlink_metadata(link).unwrap().file_type().is_file());
    // The directory field (last) is the sibling session's original recorded cwd.
    assert_eq!(lines.get(4).copied(), Some(shared.to_str().unwrap()));
    // The foreign original was only ever read.
    assert!(other_projects
        .join("-proj")
        .join(format!("{FOREIGN_ID}.jsonl"))
        .is_file());
}

#[test]
fn no_flag_not_found_summarizes_the_extra_search_roots() {
    let tmp = unique_root("not-found");
    let root = tmp.path();
    // Three search roots — the current user plus two siblings that have run
    // Claude — and the id is planted in none of them, so this is the plain
    // not-found path. The auto-fallback must not turn that into a per-account
    // recital of every home on the machine.
    for user in ["home", "alice", "bob"] {
        fs::create_dir_all(root.join(user).join(".claude/projects")).unwrap();
    }

    let out = run_crap(root, &[MISSING_ID]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(SESSION_NOT_FOUND_EXIT),
        "stderr: {stderr}"
    );
    // The current user's own tree is named in full — the one path they can act on.
    assert!(
        stderr.contains(&format!(
            "looked under {}",
            root.join("home/.claude/projects").display()
        )),
        "must name the current user's tree: {stderr}"
    );
    // The two siblings collapse into a single count, correctly pluralized.
    assert!(
        stderr.contains("…and 2 other accounts on this machine"),
        "must summarize the sibling roots: {stderr}"
    );
    // No sibling root is listed individually, and no account name leaks.
    for user in ["alice", "bob"] {
        assert!(
            !stderr.contains(
                &root
                    .join(user)
                    .join(".claude/projects")
                    .display()
                    .to_string()
            ),
            "must not list the sibling root for {user}: {stderr}"
        );
    }
    assert_eq!(
        stderr
            .lines()
            .filter(|l| l.contains("looked under"))
            .count(),
        1,
        "exactly one `looked under` line however many roots were searched: {stderr}"
    );
}

#[test]
#[cfg(unix)]
fn not_found_with_owner_only_dirs_prints_actionable_guidance() {
    let tmp = unique_root("owner-only");
    let root = tmp.path();
    // Our own tree is empty and sibling `other` has exactly one project folder,
    // owner-only. The scan can list `other`'s tree but is refused entry to that
    // folder, so the id being absent from everything readable is NOT the same as
    // the id being absent from the machine. The miss must say so and hand back a
    // remedy naming the account — never run `sudo` on the user's behalf.
    fs::create_dir_all(root.join("home/.claude/projects")).unwrap();
    let locked = root.join("other/.claude/projects/-locked");
    fs::create_dir_all(&locked).unwrap();
    set_mode(&locked, 0o000);
    // A privileged process reads straight through `0o000`, which makes the
    // refusal — the entire subject of this test — unobservable. Probe for that
    // directly rather than taking a uid dependency, and skip when it happens.
    if fs::metadata(locked.join("probe")).err().map(|e| e.kind())
        != Some(std::io::ErrorKind::PermissionDenied)
    {
        set_mode(&locked, 0o755);
        return;
    }

    // No flag: the auto-fallback enumerates sibling homes, so `other` is scanned.
    let out = run_crap(root, &[MISSING_ID]);
    // Restore before asserting anything, so a failing assertion still leaves the
    // TempDir removable on drop.
    set_mode(&locked, 0o755);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(SESSION_NOT_FOUND_EXIT),
        "stderr: {stderr}"
    );
    assert_owner_only_guidance(&stderr);
}

#[test]
#[cfg(unix)]
fn here_not_found_with_owner_only_dirs_prints_actionable_guidance() {
    let tmp = unique_root("owner-only-here");
    let root = tmp.path();
    // The same layout, reached through `--here`: both not-found call sites owe
    // the user the same guidance, and `--here` additionally owes them the same
    // discretion about other accounts as the default resume.
    fs::create_dir_all(root.join("home/.claude/projects")).unwrap();
    let locked = root.join("other/.claude/projects/-locked");
    fs::create_dir_all(&locked).unwrap();
    set_mode(&locked, 0o000);
    // Skip under a privileged process, which bypasses the mode bits entirely.
    if fs::metadata(locked.join("probe")).err().map(|e| e.kind())
        != Some(std::io::ErrorKind::PermissionDenied)
    {
        set_mode(&locked, 0o755);
        return;
    }

    let out = run_crap(root, &["--here", MISSING_ID]);
    // Restore before asserting, so the TempDir can always clean itself up.
    set_mode(&locked, 0o755);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(SESSION_NOT_FOUND_EXIT),
        "stderr: {stderr}"
    );
    assert_owner_only_guidance(&stderr);
    // A `--here` miss must summarize the other accounts searched, not recite
    // them: one `looked under` line, and no sibling root path in the output.
    assert_eq!(
        stderr
            .lines()
            .filter(|l| l.contains("looked under"))
            .count(),
        1,
        "exactly one `looked under` line however many roots were searched: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "looked under {}",
            root.join("home/.claude/projects").display()
        )),
        "the one named root must be our own tree: {stderr}"
    );
    // The sibling's root may only appear inside the remedy — that account is
    // already named because its directories were skipped, and the path is what
    // makes the remedy runnable. It must never appear as a `looked under` line,
    // which is how the roster of accounts on a shared machine used to leak.
    assert!(
        !stderr.contains(&format!(
            "looked under {}",
            root.join("other/.claude/projects").display()
        )),
        "must not recite the sibling's search root: {stderr}"
    );
}

#[test]
fn no_flag_prefers_self_when_id_exists_in_both_trees() {
    let tmp = unique_root("prefer-self");
    let root = tmp.path();
    // The SAME id exists under both the current user and a sibling, with
    // different recorded cwds. With no flag, self-first ordering must win: the
    // search short-circuits on our own copy and resumes it in place (the default
    // `<id>\n<dir>` shape, no fork), at the CURRENT user's recorded cwd.
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

    let out = run_crap(root, &[FOREIGN_ID]);
    assert!(
        out.status.success(),
        "exit {:?}, stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    // Default same-user resume shape: id then dir, no fork sentinel, self's cwd.
    assert_eq!(lines.first().copied(), Some(FOREIGN_ID));
    assert_eq!(lines.get(1).copied(), Some(self_cwd.to_str().unwrap()));
    assert!(
        !stdout.contains(FORK_AT_SENTINEL),
        "a self hit must resume in place, not fork: {stdout}"
    );
}
