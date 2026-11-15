//! Behavior tests for `chf::index`.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error"
)]

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Barrier, Mutex};
use std::time::{Duration, SystemTime};

use chf::index::{self, IndexEvent};
use chf::session::Session;
use serde_json::{json, Value};

const UUID_A: &str = "4733ee2a-1ad6-4619-a01a-11840b8e1901";
const UUID_B: &str = "5733ee2a-1ad6-4619-a01a-11840b8e1902";
const UUID_C: &str = "6733ee2a-1ad6-4619-a01a-11840b8e1903";
const UUID_D: &str = "7733ee2a-1ad6-4619-a01a-11840b8e1904";
const UUID_E: &str = "8733ee2a-1ad6-4619-a01a-11840b8e1905";
const FOLDER: &str = "-Users-someone-project";

fn transcript(word: &str) -> String {
    let value = json!({"type": "user", "timestamp": "2026-01-01T00:00:00Z",
        "message": {"role": "user", "content": word}});
    format!("{}\n", serde_json::to_string(&value).unwrap())
}

fn write_session(projects: &Path, folder: &str, uuid: &str, word: &str) -> PathBuf {
    let dir = projects.join(folder);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{uuid}.jsonl"));
    fs::write(&path, transcript(word)).unwrap();
    path
}

fn collect(projects: &Path, cache: &Path) -> Vec<IndexEvent> {
    let events = Mutex::new(Vec::new());
    index::run(projects, cache, &|event| events.lock().unwrap().push(event));
    events.into_inner().unwrap()
}

fn sessions(events: &[IndexEvent]) -> Vec<&Session> {
    events
        .iter()
        .filter_map(|event| match event {
            IndexEvent::Session(session) => Some(&**session),
            _ => None,
        })
        .collect()
}

fn unreadable(events: &[IndexEvent]) -> Vec<&Path> {
    events
        .iter()
        .filter_map(|event| match event {
            IndexEvent::Unreadable { path, .. } => Some(path.as_path()),
            _ => None,
        })
        .collect()
}

fn total(events: &[IndexEvent]) -> Option<usize> {
    events.iter().find_map(|event| match event {
        IndexEvent::Total(count) => Some(*count),
        _ => None,
    })
}

fn assert_framed(events: &[IndexEvent]) {
    assert!(
        matches!(events.first(), Some(IndexEvent::Total(_))),
        "first event: {:?}",
        events.first()
    );
    assert!(
        matches!(events.last(), Some(IndexEvent::Done)),
        "last event: {:?}",
        events.last()
    );
    let totals = events
        .iter()
        .filter(|event| matches!(event, IndexEvent::Total(_)))
        .count();
    assert_eq!(totals, 1);
}

fn has_word(events: &[IndexEvent], word: &str) -> bool {
    sessions(events)
        .iter()
        .any(|session| session.text.contains(word))
}

fn json_files(cache: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(cache)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    found.sort();
    found
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// True when a mode 000 file stays readable, which is the case for root.
#[cfg(unix)]
fn mode_zero_stays_readable(path: &Path) -> bool {
    set_mode(path, 0o000);
    let readable = File::open(path).is_ok();
    set_mode(path, 0o600);
    readable
}

fn set_mtime(path: &Path, time: SystemTime) {
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

fn epoch_plus(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

#[test]
fn two_sessions_in_one_folder() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write_session(store.path(), FOLDER, UUID_A, "alpha");
    write_session(store.path(), FOLDER, UUID_B, "bravo");

    let events = collect(store.path(), cache.path());

    assert_framed(&events);
    assert_eq!(total(&events), Some(2));
    assert_eq!(sessions(&events).len(), 2);
    assert!(has_word(&events, "alpha"));
    assert!(has_word(&events, "bravo"));
    assert!(unreadable(&events).is_empty());
    assert_eq!(events.len(), 4);
}

#[cfg(unix)]
#[test]
fn files_that_are_not_sessions_are_skipped() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    write_session(store.path(), FOLDER, UUID_A, "alpha");

    let folder = store.path().join(FOLDER);
    let nested = folder.join(UUID_B).join("subagents");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("agent-x.jsonl"), transcript("subagent")).unwrap();
    fs::write(
        folder.join(UUID_B).join(format!("{UUID_C}.jsonl")),
        transcript("deeper"),
    )
    .unwrap();
    let real = elsewhere.path().join("real.jsonl");
    fs::write(&real, transcript("linked")).unwrap();
    std::os::unix::fs::symlink(&real, folder.join(format!("{UUID_D}.jsonl"))).unwrap();
    fs::write(folder.join("notes.jsonl"), transcript("notes")).unwrap();
    fs::write(folder.join(format!("{UUID_E}.json")), transcript("json")).unwrap();
    fs::write(
        store.path().join(format!("{UUID_E}.jsonl")),
        transcript("toplevel"),
    )
    .unwrap();

    let events = collect(store.path(), cache.path());

    assert_framed(&events);
    assert_eq!(total(&events), Some(1));
    assert_eq!(sessions(&events).len(), 1);
    assert!(has_word(&events, "alpha"));
    assert!(unreadable(&events).is_empty());
}

#[cfg(unix)]
#[test]
fn a_cache_hit_does_not_read_the_transcript() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let path = write_session(store.path(), FOLDER, UUID_A, "alpha");
    if mode_zero_stays_readable(&path) {
        eprintln!("note: running as root, a mode 000 file stays readable, test skipped");
        return;
    }
    let first = collect(store.path(), cache.path());
    assert!(has_word(&first, "alpha"));

    set_mode(&path, 0o000);
    let second = collect(store.path(), cache.path());
    set_mode(&path, 0o600);

    assert_framed(&second);
    assert!(has_word(&second, "alpha"));
    assert!(unreadable(&second).is_empty());
}

#[test]
fn a_change_of_size_reads_the_transcript_again() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let path = write_session(store.path(), FOLDER, UUID_A, "alpha");
    set_mtime(&path, epoch_plus(1_700_000_000));
    let first = collect(store.path(), cache.path());
    assert!(!has_word(&first, "zulu"));

    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(&transcript("zulu"));
    fs::write(&path, text).unwrap();
    set_mtime(&path, epoch_plus(1_700_000_000));
    let second = collect(store.path(), cache.path());

    assert!(has_word(&second, "zulu"));
    assert!(has_word(&second, "alpha"));
}

#[test]
fn a_change_of_mtime_with_the_same_size_reads_the_transcript_again() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let path = write_session(store.path(), FOLDER, UUID_A, "alpha");
    set_mtime(&path, epoch_plus(1_700_000_000));
    let first = collect(store.path(), cache.path());
    assert!(has_word(&first, "alpha"));

    fs::write(&path, transcript("omega")).unwrap();
    set_mtime(&path, epoch_plus(1_700_005_000));
    let second = collect(store.path(), cache.path());

    assert!(has_word(&second, "omega"));
    assert!(!has_word(&second, "alpha"));
}

#[test]
fn a_deleted_session_goes_away() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write_session(store.path(), FOLDER, UUID_A, "alpha");
    let gone = write_session(store.path(), FOLDER, UUID_B, "bravo");
    let first = collect(store.path(), cache.path());
    assert_eq!(sessions(&first).len(), 2);
    let files = json_files(cache.path());
    assert_eq!(files.len(), 1);
    assert!(fs::read_to_string(&files[0]).unwrap().contains(UUID_B));

    fs::remove_file(&gone).unwrap();
    let second = collect(store.path(), cache.path());

    assert_eq!(total(&second), Some(1));
    assert_eq!(sessions(&second).len(), 1);
    assert!(!has_word(&second, "bravo"));
    let files = json_files(cache.path());
    assert_eq!(files.len(), 1);
    let cached = fs::read_to_string(&files[0]).unwrap();
    assert!(!cached.contains(UUID_B));
    assert!(cached.contains(UUID_A));
}

#[test]
fn a_removed_folder_loses_its_cache_file() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write_session(store.path(), "-one", UUID_A, "alpha");
    write_session(store.path(), "-two", UUID_B, "bravo");
    collect(store.path(), cache.path());
    assert_eq!(json_files(cache.path()).len(), 2);

    fs::remove_dir_all(store.path().join("-two")).unwrap();
    let events = collect(store.path(), cache.path());

    assert_eq!(json_files(cache.path()).len(), 1);
    assert!(has_word(&events, "alpha"));
}

#[cfg(unix)]
#[test]
fn an_unreadable_transcript_gives_unreadable() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let path = write_session(store.path(), FOLDER, UUID_A, "alpha");
    if mode_zero_stays_readable(&path) {
        eprintln!("note: running as root, a mode 000 file stays readable, test skipped");
        return;
    }
    set_mode(&path, 0o000);

    let events = collect(store.path(), cache.path());
    set_mode(&path, 0o600);

    assert_framed(&events);
    assert_eq!(total(&events), Some(1));
    assert!(sessions(&events).is_empty());
    assert_eq!(unreadable(&events), vec![path.as_path()]);
}

#[test]
fn a_garbage_cache_file_counts_as_empty() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write_session(store.path(), FOLDER, UUID_A, "alpha");
    collect(store.path(), cache.path());
    let files = json_files(cache.path());
    assert_eq!(files.len(), 1);
    fs::write(&files[0], "this is not json {{{").unwrap();

    let events = collect(store.path(), cache.path());

    assert!(has_word(&events, "alpha"));
    assert!(unreadable(&events).is_empty());
}

#[test]
fn a_cache_file_of_another_version_counts_as_empty() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write_session(store.path(), FOLDER, UUID_A, "alpha");
    collect(store.path(), cache.path());
    let files = json_files(cache.path());
    assert_eq!(files.len(), 1);
    let mut value: Value = serde_json::from_str(&fs::read_to_string(&files[0]).unwrap()).unwrap();
    value["version"] = json!(0);
    // The old entry holds a wrong text. Only a re-read can fix it.
    value["entries"][0]["session"]["text"] = json!("stale");
    fs::write(&files[0], serde_json::to_string(&value).unwrap()).unwrap();

    let events = collect(store.path(), cache.path());

    assert!(has_word(&events, "alpha"));
    assert!(!has_word(&events, "stale"));
}

#[cfg(unix)]
#[test]
fn two_concurrent_writers_do_not_corrupt_a_cache_file() {
    const FOLDERS: u64 = 3;
    const PER_FOLDER: u64 = 5;
    const ROUNDS: u64 = 10;

    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for folder in 0..FOLDERS {
        for number in 0..PER_FOLDER {
            let uuid = format!("{:08x}-0000-4000-8000-{:012x}", folder + 1, number + 1);
            paths.push((
                folder,
                write_session(store.path(), &format!("-folder-{folder}"), &uuid, "alpha"),
            ));
        }
    }
    if mode_zero_stays_readable(&paths[0].1) {
        eprintln!("note: running as root, a mode 000 file stays readable, test skipped");
        return;
    }

    for round in 0..ROUNDS {
        // One touched transcript for each folder makes each run write.
        for (folder, path) in &paths {
            if path
                .to_string_lossy()
                .ends_with(&format!("{:012x}.jsonl", round % PER_FOLDER + 1))
            {
                set_mtime(path, epoch_plus(1_700_000_000 + round * 10 + folder));
            }
        }
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    barrier.wait();
                    let events = collect(store.path(), cache.path());
                    assert_eq!(sessions(&events).len(), 15);
                    assert!(unreadable(&events).is_empty());
                });
            }
        });
    }

    for (_, path) in &paths {
        set_mode(path, 0o000);
    }
    let events = collect(store.path(), cache.path());
    for (_, path) in &paths {
        set_mode(path, 0o600);
    }

    assert_eq!(sessions(&events).len(), 15);
    assert!(unreadable(&events).is_empty());
    let leftovers: Vec<_> = fs::read_dir(cache.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[cfg(unix)]
#[test]
fn the_cache_has_private_modes() {
    let store = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("nested").join("cache");
    write_session(store.path(), FOLDER, UUID_A, "alpha");

    collect(store.path(), &cache);

    assert_eq!(mode_of(&cache), 0o700);
    let files = json_files(&cache);
    assert_eq!(files.len(), 1);
    assert_eq!(mode_of(&files[0]), 0o600);
}

#[cfg(unix)]
#[test]
fn an_existing_cache_directory_becomes_private() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    set_mode(cache.path(), 0o755);
    write_session(store.path(), FOLDER, UUID_A, "alpha");

    collect(store.path(), cache.path());

    assert_eq!(mode_of(cache.path()), 0o700);
}

#[test]
fn old_temporary_files_are_removed() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let hex = "0123456789abcdef0123456789abcdef";
    let old = cache.path().join(format!(".{hex}.1.2.3.tmp"));
    let fresh = cache.path().join(format!(".{hex}.4.5.6.tmp"));
    let other = cache.path().join("keep.txt");
    for path in [&old, &fresh, &other] {
        fs::write(path, "x").unwrap();
    }
    let now = SystemTime::now();
    set_mtime(&old, now - Duration::from_mins(11));
    set_mtime(&fresh, now - Duration::from_mins(1));

    collect(store.path(), cache.path());

    assert!(!old.exists());
    assert!(fresh.exists());
    assert!(other.exists());
}

#[test]
fn a_missing_projects_directory_gives_total_zero() {
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();

    let events = collect(&root.path().join("absent"), cache.path());

    assert!(matches!(
        events.as_slice(),
        [IndexEvent::Total(0), IndexEvent::Done]
    ));
}

#[test]
fn spawn_delivers_the_same_events() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write_session(store.path(), FOLDER, UUID_A, "alpha");
    write_session(store.path(), FOLDER, UUID_B, "bravo");

    let events: Vec<IndexEvent> =
        index::spawn(store.path().to_path_buf(), cache.path().to_path_buf())
            .into_iter()
            .collect();

    assert_framed(&events);
    assert_eq!(total(&events), Some(2));
    assert!(has_word(&events, "alpha"));
    assert!(has_word(&events, "bravo"));
}

/// True when a mode 000 directory stays listable, which is the case for root.
#[cfg(unix)]
fn mode_zero_stays_listable(path: &Path) -> bool {
    set_mode(path, 0o000);
    let listable = fs::read_dir(path).is_ok();
    set_mode(path, 0o755);
    listable
}

#[cfg(unix)]
#[test]
fn a_store_that_cannot_be_listed_gives_unreadable_and_keeps_the_cache() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write_session(store.path(), FOLDER, UUID_A, "alpha");
    collect(store.path(), cache.path());
    let files = json_files(cache.path());
    assert_eq!(files.len(), 1);
    let before = fs::read(&files[0]).unwrap();
    if mode_zero_stays_listable(store.path()) {
        eprintln!("note: running as root, a mode 000 directory stays listable, test skipped");
        return;
    }

    set_mode(store.path(), 0o000);
    let events = collect(store.path(), cache.path());
    set_mode(store.path(), 0o755);

    assert_eq!(json_files(cache.path()), files);
    assert_eq!(fs::read(&files[0]).unwrap(), before);
    assert_eq!(unreadable(&events), vec![store.path()]);
    assert_eq!(total(&events), Some(0));
    assert_framed(&events);
}

#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_listed_keeps_its_cache_file() {
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write_session(store.path(), "-one", UUID_A, "alpha");
    write_session(store.path(), "-two", UUID_B, "bravo");
    collect(store.path(), cache.path());
    let files = json_files(cache.path());
    assert_eq!(files.len(), 2);
    let before: Vec<Vec<u8>> = files.iter().map(|file| fs::read(file).unwrap()).collect();
    let locked = store.path().join("-two");
    if mode_zero_stays_listable(&locked) {
        eprintln!("note: running as root, a mode 000 directory stays listable, test skipped");
        return;
    }

    set_mode(&locked, 0o000);
    let events = collect(store.path(), cache.path());
    set_mode(&locked, 0o755);

    assert_eq!(json_files(cache.path()), files);
    let after: Vec<Vec<u8>> = files.iter().map(|file| fs::read(file).unwrap()).collect();
    assert_eq!(after, before);
    assert_eq!(unreadable(&events), vec![locked.as_path()]);
    assert!(has_word(&events, "alpha"));
    assert!(!has_word(&events, "bravo"));
    assert_eq!(total(&events), Some(1));
    assert_framed(&events);
}
