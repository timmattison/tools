//! The store and the cache.
//!
//! `run` walks the Claude Code store, `<projects>/<folder>/<uuid>.jsonl`, and
//! sends one `Session` event for each transcript. A transcript that did not
//! change since the last run comes from the cache. A transcript that did change
//! is read and extracted again.
//!
//! The cache has one file for each project folder. Two copies of chf can run at
//! the same time and can not corrupt one file, because each write goes to a
//! temporary file first and a `rename` puts it in place. A reader sees the old
//! file or the new file and never half of one.
//!
//! The cache directory has mode 0700 and each cache file has mode 0600. Claude
//! Code keeps its transcripts at mode 0600, and the cache holds their text. The
//! cache must not be more open than the transcripts.
//!
//! The name of a cache file is a hash of the folder name. A folder name can be
//! 255 bytes long. A suffix such as `.json` or a temporary-file tail can then
//! make the name too long for the file system. A hash has a fixed length.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use claude_transcript::SessionId;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::extract::extract;
use crate::session::Session;

/// The version of the cache file format.
///
/// Add 1 to this number each time the shape of `Session` or of the cache file
/// changes. A cache file with another version counts as empty.
const CACHE_VERSION: u32 = 1;

/// The count of hex digits in the name of a cache file.
const HEX_LEN: usize = 32;

/// The extension of a cache file, with its dot.
const CACHE_SUFFIX: &str = ".json";

/// The extension of a temporary cache file, with its dot.
const TMP_SUFFIX: &str = ".tmp";

/// A temporary file older than this was left by a run that died.
const TMP_MAX_AGE: Duration = Duration::from_mins(10);

/// The mode of the cache directory.
#[cfg(unix)]
const DIR_MODE: u32 = 0o700;

/// The mode of a cache file.
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;

/// The counter that keeps the temporary file names of one process apart.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// One event of an indexing run.
#[derive(Debug)]
pub enum IndexEvent {
    /// The count of session files found. Sent once, as the first event of a run.
    /// A folder that chf can not list counts 0.
    Total(usize),
    /// One indexed session.
    Session(Box<Session>),
    /// A transcript that chf can not open or read, or a folder or the store that
    /// it can not list. A store or a folder that chf can not list keeps its cache.
    Unreadable {
        /// The path of the transcript, of the folder, or of the store.
        path: PathBuf,
        /// The text of the io error.
        reason: String,
    },
    /// The last event of a run.
    Done,
}

/// The content of one cache file.
#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    folder: String,
    entries: Vec<CacheEntry>,
}

/// One session in a cache file, with the facts that make it a hit.
#[derive(Serialize, Deserialize)]
struct CacheEntry {
    file: String,
    size: u64,
    mtime_ns: u64,
    session: Session,
}

impl CacheEntry {
    fn key(&self) -> EntryKey {
        (self.file.clone(), self.size, self.mtime_ns)
    }
}

/// The file name, the size, and the mtime in nanoseconds.
type EntryKey = (String, u64, u64);

/// What chf learned about one session file in the store.
struct Candidate {
    id: SessionId,
    path: PathBuf,
    file: String,
    stat: Result<Stat, String>,
}

/// The size and the mtime of a file.
#[derive(Clone, Copy)]
struct Stat {
    size: u64,
    mtime: SystemTime,
}

impl Stat {
    fn mtime_ns(self) -> u64 {
        self.mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
            })
    }
}

/// A project folder of the store and its session files.
struct Folder {
    name: String,
    hex: String,
    /// The session files, or `None` when chf could not list the folder.
    candidates: Option<Vec<Candidate>>,
    newest: SystemTime,
}

/// What `discover` learned about the store.
struct Store {
    folders: Vec<Folder>,
    /// Each store or folder that chf could not list, with the io error text.
    unreadable: Vec<(PathBuf, String)>,
    /// True only when chf listed the store from start to end. A missing store
    /// counts as listed, because it holds no sessions.
    complete: bool,
}

/// Index the store synchronously. Each event goes to `sink`.
///
/// `projects` is the Claude Code `projects` directory. `cache` is the cache
/// directory. The run sends `Total` first. Then it sends one `Unreadable` for
/// the store, if chf can not list it, and one for each folder that chf can not
/// list. Then it sends one `Session` or `Unreadable` for each session file.
/// `Done` is the last event. A failure to write the cache is not an event.
///
/// The run removes stale cache files only when it listed the whole store. A
/// folder that chf did not see is not a folder that is gone. A folder that chf
/// can not list keeps its cache file as it is.
pub fn run(projects: &Path, cache: &Path, sink: &(dyn Fn(IndexEvent) + Sync)) {
    let Store {
        mut folders,
        unreadable,
        complete,
    } = discover(projects);
    let total: usize = folders
        .iter()
        .map(|folder| folder.candidates.as_ref().map_or(0, Vec::len))
        .sum();

    let cache = prepare_cache_dir(cache).then_some(cache);
    if let Some(cache) = cache.filter(|_| complete) {
        let live: HashSet<&str> = folders.iter().map(|folder| folder.hex.as_str()).collect();
        clean_cache_dir(cache, &live);
    }

    sink(IndexEvent::Total(total));
    for (path, reason) in unreadable {
        sink(IndexEvent::Unreadable { path, reason });
    }
    folders.sort_by_key(|folder| std::cmp::Reverse(folder.newest));
    folders
        .par_iter()
        .for_each(|folder| process_folder(folder, cache, sink));
    sink(IndexEvent::Done);
}

/// Run `run` on a new thread and return the receiver of its events.
///
/// A send to a dropped receiver is ignored, so the thread ends its work quietly.
#[must_use]
pub fn spawn(projects: PathBuf, cache: PathBuf) -> std::sync::mpsc::Receiver<IndexEvent> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        run(&projects, &cache, &|event| {
            let _ = sender.send(event);
        });
    });
    receiver
}

/// List each project folder and stat each session file. This function sends no
/// event. It returns what chf could not list, and whether the listing of the
/// store is complete.
///
/// A store that does not exist has no sessions, and its listing is complete.
/// Any other error of the store, or an error while chf reads its entries, makes
/// the listing not complete.
fn discover(projects: &Path) -> Store {
    let mut store = Store {
        folders: Vec::new(),
        unreadable: Vec::new(),
        complete: true,
    };
    let entries = match fs::read_dir(projects) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return store,
        Err(error) => {
            store.complete = false;
            store
                .unreadable
                .push((projects.to_path_buf(), error.to_string()));
            return store;
        }
    };
    let mut entry_error = None;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                entry_error.get_or_insert(error.to_string());
                continue;
            }
        };
        let path = entry.path();
        let is_dir = fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir());
        if !is_dir {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let hex = folder_hex(&entry.file_name());
        let candidates = match list_candidates(&path) {
            Ok(candidates) => Some(candidates),
            Err(error) => {
                store.unreadable.push((path, error.to_string()));
                None
            }
        };
        let newest = candidates
            .iter()
            .flatten()
            .filter_map(|candidate| candidate.stat.as_ref().ok())
            .map(|stat| stat.mtime)
            .max()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        store.folders.push(Folder {
            name,
            hex,
            candidates,
            newest,
        });
    }
    if let Some(reason) = entry_error {
        store.complete = false;
        store.unreadable.insert(0, (projects.to_path_buf(), reason));
    }
    store
}

/// The session files directly inside one project folder.
fn list_candidates(folder: &Path) -> std::io::Result<Vec<Candidate>> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(folder)? {
        let Ok(entry) = entry else {
            continue;
        };
        let file_name = entry.file_name();
        let Some(id) = SessionId::from_file_name(&file_name) else {
            continue;
        };
        let path = entry.path();
        let stat = match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() => Ok(Stat {
                size: meta.len(),
                mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            }),
            Ok(_) => continue,
            Err(error) => Err(error.to_string()),
        };
        candidates.push(Candidate {
            id,
            path,
            file: file_name.to_string_lossy().into_owned(),
            stat,
        });
    }
    Ok(candidates)
}

/// Send the sessions of one folder and update its cache file.
///
/// A folder that chf could not list sends nothing, and its cache file stays as
/// it is. `run` already sent the `Unreadable` event of that folder.
fn process_folder(folder: &Folder, cache: Option<&Path>, sink: &(dyn Fn(IndexEvent) + Sync)) {
    let Some(candidates) = &folder.candidates else {
        return;
    };
    let old = cache
        .map(|cache| load_cache(cache, folder))
        .unwrap_or_default();
    let old_keys: HashSet<EntryKey> = old.iter().map(CacheEntry::key).collect();

    let mut kept: Vec<CacheEntry> = Vec::new();
    let mut misses: Vec<(&Candidate, Stat)> = Vec::new();
    for candidate in candidates {
        let stat = match &candidate.stat {
            Ok(stat) => *stat,
            Err(reason) => {
                sink(IndexEvent::Unreadable {
                    path: candidate.path.clone(),
                    reason: reason.clone(),
                });
                continue;
            }
        };
        let hit = old.iter().find(|entry| {
            entry.file == candidate.file
                && entry.size == stat.size
                && entry.mtime_ns == stat.mtime_ns()
        });
        if let Some(entry) = hit {
            let mut session = entry.session.clone();
            session.path.clone_from(&candidate.path);
            sink(IndexEvent::Session(Box::new(session.clone())));
            kept.push(CacheEntry {
                file: entry.file.clone(),
                size: entry.size,
                mtime_ns: entry.mtime_ns,
                session,
            });
        } else {
            misses.push((candidate, stat));
        }
    }

    let read: Vec<CacheEntry> = misses
        .par_iter()
        .filter_map(|(candidate, stat)| read_candidate(candidate, *stat, sink))
        .collect();
    kept.extend(read);

    let new_keys: HashSet<EntryKey> = kept.iter().map(CacheEntry::key).collect();
    if let Some(cache) = cache {
        if new_keys != old_keys {
            // A failure to write the cache is not an event. The next run tries again.
            let _ = write_cache(cache, folder, kept);
        }
    }
}

/// Read and extract one transcript. Send the `Session` or the `Unreadable` event.
fn read_candidate(
    candidate: &Candidate,
    stat: Stat,
    sink: &(dyn Fn(IndexEvent) + Sync),
) -> Option<CacheEntry> {
    match fs::read(&candidate.path) {
        Ok(bytes) => {
            let session = extract(
                candidate.id.clone(),
                candidate.path.clone(),
                &bytes,
                stat.mtime,
            );
            sink(IndexEvent::Session(Box::new(session.clone())));
            Some(CacheEntry {
                file: candidate.file.clone(),
                size: stat.size,
                mtime_ns: stat.mtime_ns(),
                session,
            })
        }
        Err(error) => {
            sink(IndexEvent::Unreadable {
                path: candidate.path.clone(),
                reason: error.to_string(),
            });
            None
        }
    }
}

/// The entries of the cache file of `folder`. A bad file gives no entries.
fn load_cache(cache: &Path, folder: &Folder) -> Vec<CacheEntry> {
    let Ok(text) = fs::read(cache.join(format!("{}{CACHE_SUFFIX}", folder.hex))) else {
        return Vec::new();
    };
    match serde_json::from_slice::<CacheFile>(&text) {
        Ok(file) if file.version == CACHE_VERSION && file.folder == folder.name => file.entries,
        _ => Vec::new(),
    }
}

/// Write the cache file of `folder` through a temporary file and a rename.
fn write_cache(cache: &Path, folder: &Folder, entries: Vec<CacheEntry>) -> std::io::Result<()> {
    let file = CacheFile {
        version: CACHE_VERSION,
        folder: folder.name.clone(),
        entries,
    };
    let json = serde_json::to_vec(&file).map_err(std::io::Error::other)?;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = cache.join(format!(
        ".{}.{}.{nanos}.{counter}{TMP_SUFFIX}",
        folder.hex,
        std::process::id()
    ));
    let result = write_new_file(&tmp, &json)
        .and_then(|()| fs::rename(&tmp, cache.join(format!("{}{CACHE_SUFFIX}", folder.hex))));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Create `path` with mode 0600 where the platform has modes, and write `bytes`.
fn write_new_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(FILE_MODE);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

/// Make the cache directory with mode 0700. Give `false` when that fails.
fn prepare_cache_dir(cache: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(cache)
            .and_then(|()| fs::set_permissions(cache, fs::Permissions::from_mode(DIR_MODE)))
            .is_ok()
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(cache).is_ok()
    }
}

/// Remove the cache files of folders that are gone and the old temporary files.
/// Touch no other file.
fn clean_cache_dir(cache: &Path, live: &HashSet<&str>) {
    let Ok(entries) = fs::read_dir(cache) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let remove = if let Some(hex) = cache_file_hex(name) {
            !live.contains(hex)
        } else if is_tmp_name(name) {
            meta.modified()
                .ok()
                .and_then(|mtime| now.duration_since(mtime).ok())
                .is_some_and(|age| age > TMP_MAX_AGE)
        } else {
            false
        };
        if remove {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// True for 32 lowercase hex digits.
fn is_hex_name(text: &str) -> bool {
    text.len() == HEX_LEN
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The hex part of `<32 hex>.json`, else `None`.
fn cache_file_hex(name: &str) -> Option<&str> {
    name.strip_suffix(CACHE_SUFFIX)
        .filter(|hex| is_hex_name(hex))
}

/// True for `.<32 hex>.<pid>.<nanos>.<counter>.tmp`.
fn is_tmp_name(name: &str) -> bool {
    let Some(inner) = name
        .strip_prefix('.')
        .and_then(|rest| rest.strip_suffix(TMP_SUFFIX))
    else {
        return false;
    };
    let parts: Vec<&str> = inner.split('.').collect();
    parts.len() == 4
        && is_hex_name(parts[0])
        && parts[1..]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// The first 32 hex digits of the blake3 hash of a folder name.
fn folder_hex(name: &OsStr) -> String {
    #[cfg(unix)]
    let hash = {
        use std::os::unix::ffi::OsStrExt;
        blake3::hash(name.as_bytes())
    };
    #[cfg(not(unix))]
    let hash = blake3::hash(name.to_string_lossy().as_bytes());
    hash.to_hex().chars().take(HEX_LEN).collect()
}
