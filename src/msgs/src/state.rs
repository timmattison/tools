//! The state of the app and the logic of the commands, without Tauri types,
//! so tests can drive it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;

use crate::cache::{TextCache, PROGRESS_INTERVAL};
use crate::db::MessageDb;
use crate::error::MsgsError;
use crate::export::export_conversation_range;
use crate::types::{Conversation, DbStatus, ExportResult, Message, SearchResult};

/// Where chat.db lives, relative to the home directory.
const CHAT_DB_PATH: &str = "Library/Messages/chat.db";

/// The folder, inside the local data directory, where an older msgs kept its
/// search index.
const OLD_CACHE_DIR_NAME: &str = "msgs";

/// The file where an older msgs kept the text of each message.
const OLD_CACHE_DB_NAME: &str = "text-cache.db";

/// The suffixes of the old cache file and of its SQLite side files. The side
/// files hold message text too.
const OLD_CACHE_SUFFIXES: [&str; 4] = ["", "-journal", "-wal", "-shm"];

/// The places that the app reads and cleans up.
#[derive(Debug, Clone)]
pub struct DataPaths {
    pub chat_db: PathBuf,
    /// The folder where an older msgs kept `text-cache.db`. `None` when the
    /// local data directory is unknown.
    pub old_cache_dir: Option<PathBuf>,
}

impl DataPaths {
    /// The paths of a real install. Creates no folder.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` when the home directory is unknown.
    pub fn default_paths() -> Result<Self, MsgsError> {
        let home = dirs::home_dir().ok_or(MsgsError::DatabaseError(
            "Cannot determine home directory".to_string(),
        ))?;
        Ok(Self {
            chat_db: home.join(CHAT_DB_PATH),
            old_cache_dir: dirs::data_local_dir().map(|dir| dir.join(OLD_CACHE_DIR_NAME)),
        })
    }
}

/// Delete the text cache of an older msgs: the file, its SQLite side files,
/// and the folder when it is empty. The file holds the text of each message
/// outside the TCC protection of `~/Library/Messages`.
fn remove_old_text_cache(dir: &Path) {
    for suffix in OLD_CACHE_SUFFIXES {
        let file = dir.join(format!("{OLD_CACHE_DB_NAME}{suffix}"));
        match std::fs::remove_file(&file) {
            Ok(()) => log::info!("Deleted the old text cache {}", file.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("Cannot delete the old text cache {}: {e}", file.display()),
        }
    }
    // The folder is missing, or it holds other files. Both are fine.
    let _ = std::fs::remove_dir(dir);
}

/// The state of the search index, as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CacheStatus {
    NotStarted,
    Building { indexed: i64, total: i64 },
    Ready,
    Failed { error: String },
}

/// The search index and its state.
enum CacheSlot {
    NotStarted,
    Building { indexed: i64, total: i64 },
    Ready(TextCache),
    Failed(String),
}

impl CacheSlot {
    fn status(&self) -> CacheStatus {
        match self {
            Self::NotStarted => CacheStatus::NotStarted,
            Self::Building { indexed, total } => CacheStatus::Building {
                indexed: *indexed,
                total: *total,
            },
            Self::Ready(_) => CacheStatus::Ready,
            Self::Failed(error) => CacheStatus::Failed {
                error: error.clone(),
            },
        }
    }
}

/// The app state: the open chat.db and the search index.
pub struct AppState {
    paths: Result<DataPaths, String>,
    db: Mutex<Option<MessageDb>>,
    cache: Arc<Mutex<CacheSlot>>,
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, MsgsError> {
    mutex
        .lock()
        .map_err(|e| MsgsError::DatabaseError(e.to_string()))
}

fn not_initialized() -> MsgsError {
    MsgsError::DatabaseError("Database not initialized".to_string())
}

/// Build the search index in memory from the chat.db at `chat_db`. Uses its
/// own read-only connection to chat.db, thus it holds no lock while it reads.
/// Locks the slot only to store a report or the result.
fn run_build(chat_db: &Path, slot: &Mutex<CacheSlot>, on_status: &dyn Fn(CacheStatus)) {
    let set = |new: CacheSlot| {
        if let Ok(mut guard) = slot.lock() {
            *guard = new;
        }
    };
    let built = (|| -> Result<TextCache, MsgsError> {
        let db = MessageDb::open_at(chat_db)?;
        TextCache::build(db.connection(), PROGRESS_INTERVAL, &mut |indexed, total| {
            set(CacheSlot::Building { indexed, total });
            on_status(CacheStatus::Building { indexed, total });
        })
    })();
    match built {
        Ok(cache) => {
            set(CacheSlot::Ready(cache));
            on_status(CacheStatus::Ready);
        }
        Err(e) => {
            let error = e.to_string();
            set(CacheSlot::Failed(error.clone()));
            on_status(CacheStatus::Failed { error });
        }
    }
}

/// The error of a build whose thread panicked.
const BUILD_PANICKED: &str = "The build of the search index stopped unexpectedly";

/// Run `run_build`, and turn a panic of it into a failed build. A build that
/// panics after it stored a result keeps that result.
fn run_build_catching_panics(
    chat_db: &Path,
    slot: &Mutex<CacheSlot>,
    on_status: &dyn Fn(CacheStatus),
) {
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_build(chat_db, slot, on_status);
    }));
    if built.is_ok() {
        return;
    }
    {
        let mut guard = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(*guard, CacheSlot::Building { .. }) {
            return;
        }
        *guard = CacheSlot::Failed(BUILD_PANICKED.to_string());
    }
    on_status(CacheStatus::Failed {
        error: BUILD_PANICKED.to_string(),
    });
}

impl AppState {
    /// Make the state. `paths` is an error text when the home directory is
    /// unknown, and the first `open` reports it.
    pub fn new(paths: Result<DataPaths, String>) -> Self {
        Self {
            paths,
            db: Mutex::new(None),
            cache: Arc::new(Mutex::new(CacheSlot::NotStarted)),
        }
    }

    /// Open chat.db and start the build of the search index on a new thread.
    /// Each call first deletes the text cache of an older msgs. Returns when
    /// chat.db is open. Calls `on_status` from the build thread with each
    /// change of the index state. A second call starts no second build,
    /// unless the last build failed.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError` when the access check fails. A chat.db that cannot
    /// open gives `Ok` with `accessible: false`. A failed build is a
    /// `CacheStatus::Failed`, not an error. A build that panics is a failed
    /// build too.
    pub fn open(
        &self,
        on_status: impl Fn(CacheStatus) + Send + 'static,
    ) -> Result<DbStatus, MsgsError> {
        if let Ok(DataPaths {
            old_cache_dir: Some(dir),
            ..
        }) = &self.paths
        {
            remove_old_text_cache(dir);
        }
        let mut db_slot = lock(&self.db)?;
        if let Some(db) = db_slot.as_ref() {
            let status = db.check_access()?;
            drop(db_slot);
            if matches!(*lock(&self.cache)?, CacheSlot::Failed(_)) {
                self.start_build(on_status)?;
            }
            return Ok(status);
        }
        let inaccessible = |error: String| DbStatus {
            accessible: false,
            message_count: None,
            error: Some(error),
        };
        let paths = match &self.paths {
            Ok(paths) => paths,
            Err(error) => return Ok(inaccessible(error.clone())),
        };
        match MessageDb::open_at(&paths.chat_db) {
            Ok(db) => {
                let status = db.check_access()?;
                *db_slot = Some(db);
                drop(db_slot);
                self.start_build(on_status)?;
                Ok(status)
            }
            Err(e) => Ok(inaccessible(e.to_string())),
        }
    }

    /// Set the slot to building and start the build thread. Does nothing
    /// when a build runs or the index is ready.
    fn start_build(
        &self,
        on_status: impl Fn(CacheStatus) + Send + 'static,
    ) -> Result<(), MsgsError> {
        let Ok(paths) = &self.paths else {
            return Ok(());
        };
        {
            let mut slot = lock(&self.cache)?;
            if !matches!(*slot, CacheSlot::NotStarted | CacheSlot::Failed(_)) {
                return Ok(());
            }
            *slot = CacheSlot::Building {
                indexed: 0,
                total: 0,
            };
        }
        let chat_db = paths.chat_db.clone();
        let slot = Arc::clone(&self.cache);
        std::thread::spawn(move || run_build_catching_panics(&chat_db, &slot, &on_status));
        Ok(())
    }

    /// The state of the search index.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::DatabaseError` when a lock is poisoned.
    pub fn cache_status(&self) -> Result<CacheStatus, MsgsError> {
        Ok(lock(&self.cache)?.status())
    }

    /// List conversations, paginated.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError` when the database is not open or the query fails.
    pub fn list_conversations(
        &self,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<Conversation>, MsgsError> {
        let guard = lock(&self.db)?;
        guard
            .as_ref()
            .ok_or_else(not_initialized)?
            .list_conversations(offset, limit)
    }

    /// Get the messages of a chat, paginated.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError` when the database is not open or the query fails.
    pub fn get_messages(
        &self,
        chat_id: i64,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<Message>, MsgsError> {
        let guard = lock(&self.db)?;
        guard
            .as_ref()
            .ok_or_else(not_initialized)?
            .get_messages(chat_id, offset, limit)
    }

    /// The position of a message in the newest-first order of `get_messages`.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError` when the database is not open, the message is not
    /// in the chat, or the query fails.
    pub fn message_position(&self, chat_id: i64, message_id: i64) -> Result<i64, MsgsError> {
        let guard = lock(&self.db)?;
        guard
            .as_ref()
            .ok_or_else(not_initialized)?
            .message_position(chat_id, message_id)
    }

    /// Search the text of the messages.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError::CacheError` when the index is not ready, and
    /// `MsgsError` when a query fails.
    pub fn search(
        &self,
        query: &str,
        chat_id: Option<i64>,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<SearchResult>, MsgsError> {
        let hits = {
            let slot = lock(&self.cache)?;
            match &*slot {
                CacheSlot::Ready(cache) => cache.search(query, chat_id, limit, offset)?,
                CacheSlot::NotStarted | CacheSlot::Building { .. } => {
                    return Err(MsgsError::CacheError(
                        "The search index is still building".to_string(),
                    ));
                }
                CacheSlot::Failed(error) => {
                    return Err(MsgsError::CacheError(error.clone()));
                }
            }
        };
        let guard = lock(&self.db)?;
        let db = guard.as_ref().ok_or_else(not_initialized)?;
        let mut results = Vec::new();
        // A hit whose message chat.db no longer holds is skipped. Each other
        // error fails the search.
        for (message_id, hit_chat_id) in hits {
            if let Some(context) = db.get_message_with_context(message_id, hit_chat_id)? {
                results.push(context);
            }
        }
        Ok(results)
    }

    /// Export a range of messages to `export_path`.
    ///
    /// # Errors
    ///
    /// Returns `MsgsError` when the database is not open or the export fails.
    pub fn export(
        &self,
        chat_id: i64,
        start_message_id: i64,
        end_message_id: i64,
        include_attachments: bool,
        export_path: &str,
    ) -> Result<ExportResult, MsgsError> {
        let guard = lock(&self.db)?;
        let db = guard.as_ref().ok_or_else(not_initialized)?;
        // Without a home directory there is no root, and the export copies no attachment.
        let attachments_root = crate::util::default_attachments_root().unwrap_or_default();
        export_conversation_range(
            db,
            chat_id,
            start_message_id,
            end_message_id,
            include_attachments,
            std::path::Path::new(export_path),
            &attachments_root,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::test_fixture::{apple_ns, ChatDbFixture, MessageSpec};

    /// The paths of a start that has no old cache folder.
    fn paths_of(fixture: &ChatDbFixture) -> DataPaths {
        DataPaths {
            chat_db: fixture.path().to_path_buf(),
            old_cache_dir: None,
        }
    }

    /// How long a test waits for a call that must not wait for the build.
    const CALL_TIMEOUT: Duration = Duration::from_secs(20);

    /// An app state whose cache build stops at its first `Building` status
    /// until the test drops this value.
    struct HeldBuild {
        state: Arc<AppState>,
        /// Dropping the sender lets the held build go on.
        _release: mpsc::Sender<()>,
        open_result: mpsc::Receiver<Result<DbStatus, MsgsError>>,
        entered: mpsc::Receiver<()>,
        _fixture: ChatDbFixture,
    }

    impl HeldBuild {
        fn start() -> Self {
            let fixture = ChatDbFixture::new();
            let chat = fixture.add_chat("friend", Some("Friend"), 45);
            fixture.add_message(MessageSpec {
                text: Some("lunch at noon".to_string()),
                date: apple_ns(1_700_000_000, 0),
                chats: vec![chat],
                ..MessageSpec::default()
            });
            let paths = paths_of(&fixture);
            let state = Arc::new(AppState::new(Ok(paths)));
            let (release, gate) = mpsc::channel::<()>();
            let (entered_tx, entered) = mpsc::channel::<()>();
            let (result_tx, open_result) = mpsc::channel();
            let opener = Arc::clone(&state);
            std::thread::spawn(move || {
                let result = opener.open(move |status| {
                    if matches!(status, CacheStatus::Building { .. }) {
                        let _ = entered_tx.send(());
                        // Returns when the test drops the sender.
                        let _ = gate.recv();
                    }
                });
                let _ = result_tx.send(result);
            });
            Self {
                state,
                _release: release,
                open_result,
                entered,
                _fixture: fixture,
            }
        }

        /// Wait until the build has started and stands at the gate.
        fn wait_for_build(&self) {
            self.entered
                .recv_timeout(CALL_TIMEOUT)
                .expect("the build never started");
        }

        /// Run `call` on its own thread and fail when it waits for the build.
        fn answers<T: Send + 'static>(
            &self,
            what: &str,
            call: impl FnOnce(&AppState) -> T + Send + 'static,
        ) -> T {
            let (tx, rx) = mpsc::channel();
            let state = Arc::clone(&self.state);
            std::thread::spawn(move || {
                let _ = tx.send(call(&state));
            });
            rx.recv_timeout(CALL_TIMEOUT)
                .unwrap_or_else(|_| panic!("{what} waited for the cache build"))
        }
    }

    #[test]
    fn open_returns_before_the_cache_build_ends() {
        let held = HeldBuild::start();
        held.wait_for_build();
        let status = held
            .open_result
            .recv_timeout(CALL_TIMEOUT)
            .expect("open waited for the cache build")
            .expect("open");
        assert!(status.accessible);
    }

    #[test]
    fn the_database_answers_while_the_cache_builds() {
        let held = HeldBuild::start();
        held.wait_for_build();
        let chats = held
            .answers("list_conversations", |s| s.list_conversations(0, 50))
            .expect("list conversations");
        assert_eq!(chats.len(), 1);
    }

    #[test]
    fn search_reports_that_the_index_is_building() {
        let held = HeldBuild::start();
        held.wait_for_build();
        let result = held.answers("search", |s| s.search("lunch", None, 0, 50));
        match result {
            Err(MsgsError::CacheError(text)) => assert!(text.contains("building"), "{text}"),
            other => panic!("expected a building error, got {other:?}"),
        }
    }

    /// Collect each status that `open` reports, and wait for the last one.
    fn statuses_of_a_finished_build(paths: DataPaths) -> (AppState, Vec<CacheStatus>) {
        let state = AppState::new(Ok(paths));
        let (tx, rx) = mpsc::channel();
        state
            .open(move |status| {
                let _ = tx.send(status);
            })
            .expect("open");
        let mut seen = Vec::new();
        while let Ok(status) = rx.recv_timeout(CALL_TIMEOUT) {
            let last = matches!(status, CacheStatus::Ready | CacheStatus::Failed { .. });
            seen.push(status);
            if last {
                break;
            }
        }
        (state, seen)
    }

    fn fixture_with_messages(count: i64) -> ChatDbFixture {
        let fixture = ChatDbFixture::new();
        let chat = fixture.add_chat("friend", Some("Friend"), 45);
        for n in 0..count {
            fixture.add_message(MessageSpec {
                text: Some(format!("lunch {n}")),
                date: apple_ns(1_700_000_000 + n, 0),
                chats: vec![chat],
                ..MessageSpec::default()
            });
        }
        fixture
    }

    #[test]
    fn a_finished_build_reports_ready_and_search_works() {
        let fixture = fixture_with_messages(3);
        let paths = paths_of(&fixture);
        let (state, seen) = statuses_of_a_finished_build(paths);
        assert_eq!(seen.last(), Some(&CacheStatus::Ready));
        assert_eq!(state.cache_status().expect("status"), CacheStatus::Ready);
        let hits = state.search("lunch", None, 0, 50).expect("search");
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn progress_reports_never_go_back_and_end_at_the_total() {
        let fixture = fixture_with_messages(3);
        let paths = paths_of(&fixture);
        let (_state, seen) = statuses_of_a_finished_build(paths);
        let indexed: Vec<i64> = seen
            .iter()
            .filter_map(|status| match status {
                CacheStatus::Building { indexed, total } => {
                    assert_eq!(*total, 3);
                    Some(*indexed)
                }
                _ => None,
            })
            .collect();
        assert!(
            indexed.windows(2).all(|pair| pair[0] < pair[1]),
            "{indexed:?}"
        );
        assert_eq!(indexed.last(), Some(&3));
    }

    /// A state whose index holds the 3 messages of `fixture`.
    fn ready_state(fixture: &ChatDbFixture) -> AppState {
        let paths = paths_of(fixture);
        let (state, seen) = statuses_of_a_finished_build(paths);
        assert_eq!(seen.last(), Some(&CacheStatus::Ready));
        state
    }

    #[test]
    fn search_skips_a_hit_whose_message_is_gone_from_the_database() {
        let fixture = fixture_with_messages(3);
        let state = ready_state(&fixture);
        fixture.execute("DELETE FROM message WHERE text = 'lunch 0'", []);
        fixture.execute(
            "DELETE FROM chat_message_join WHERE message_id NOT IN (SELECT ROWID FROM message)",
            [],
        );

        let hits = state.search("lunch", None, 0, 50).expect("search");
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn search_reports_a_failed_query_of_a_hit() {
        let fixture = fixture_with_messages(3);
        let state = ready_state(&fixture);
        // The query of each hit joins the handle table.
        fixture.execute("DROP TABLE handle", []);

        let result = state.search("lunch", None, 0, 50);
        assert!(
            matches!(result, Err(MsgsError::DatabaseError(_))),
            "{result:?}"
        );
    }

    #[test]
    fn a_build_that_panics_reports_failed_and_the_next_open_starts_a_build() {
        let fixture = fixture_with_messages(1);
        let paths = paths_of(&fixture);
        let state = AppState::new(Ok(paths));
        let (tx, rx) = mpsc::channel();
        state
            .open(move |status| {
                assert!(
                    !matches!(status, CacheStatus::Building { .. }),
                    "a panic in the build thread"
                );
                let _ = tx.send(status);
            })
            .expect("open");
        let last = rx
            .recv_timeout(CALL_TIMEOUT)
            .expect("the build thread reported nothing after its panic");
        assert!(matches!(last, CacheStatus::Failed { .. }), "{last:?}");
        assert!(matches!(
            state.cache_status().expect("status"),
            CacheStatus::Failed { .. }
        ));

        let (tx, rx) = mpsc::channel();
        state
            .open(move |status| {
                let _ = tx.send(status);
            })
            .expect("second open");
        let mut last = None;
        while let Ok(status) = rx.recv_timeout(CALL_TIMEOUT) {
            last = Some(status);
            if matches!(last, Some(CacheStatus::Ready | CacheStatus::Failed { .. })) {
                break;
            }
        }
        assert_eq!(last, Some(CacheStatus::Ready));
    }

    #[test]
    fn a_build_that_cannot_open_chat_db_reports_failed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let slot = Mutex::new(CacheSlot::Building {
            indexed: 0,
            total: 0,
        });
        let seen = Mutex::new(Vec::new());
        run_build(&dir.path().join("missing.db"), &slot, &|status| {
            seen.lock().expect("seen").push(status);
        });
        assert!(matches!(*slot.lock().expect("slot"), CacheSlot::Failed(_)));
        assert!(matches!(
            seen.lock().expect("seen").last(),
            Some(CacheStatus::Failed { .. })
        ));
    }

    /// The name of the file where an older msgs kept the search index.
    const OLD_CACHE_FILE: &str = "text-cache.db";

    /// The paths of a start whose application-support folder is `support`.
    /// An older msgs kept its index in `support/msgs`, thus that folder
    /// exists, as on a Mac where an older msgs ran.
    fn paths_with_support_dir(fixture: &ChatDbFixture, support: &Path) -> DataPaths {
        let old_cache_dir = support.join("msgs");
        std::fs::create_dir_all(&old_cache_dir).expect("old cache dir");
        DataPaths {
            chat_db: fixture.path().to_path_buf(),
            old_cache_dir: Some(old_cache_dir),
        }
    }

    /// Each file under `dir`, at each depth.
    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(next) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&next) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files
    }

    #[test]
    fn no_file_under_the_support_dir_holds_message_text_after_a_start_and_a_search() {
        const SECRET: &str = "zanzibar";
        let fixture = ChatDbFixture::new();
        let chat = fixture.add_chat("friend", Some("Friend"), 45);
        fixture.add_message(MessageSpec {
            text: Some(format!("the {SECRET} plan")),
            date: apple_ns(1_700_000_000, 0),
            chats: vec![chat],
            ..MessageSpec::default()
        });
        let support = fixture.dir().join("Application Support");
        let (state, seen) =
            statuses_of_a_finished_build(paths_with_support_dir(&fixture, &support));
        assert_eq!(seen.last(), Some(&CacheStatus::Ready));
        assert_eq!(state.search(SECRET, None, 0, 50).expect("search").len(), 1);

        for file in files_under(&support) {
            let bytes = std::fs::read(&file).expect("read file");
            assert!(
                !bytes.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()),
                "{} holds message text",
                file.display()
            );
        }
    }

    #[test]
    fn a_start_deletes_the_text_cache_of_an_older_msgs() {
        let fixture = fixture_with_messages(1);
        let support = fixture.dir().join("Application Support");
        let paths = paths_with_support_dir(&fixture, &support);
        let old_dir = support.join("msgs");
        let old_files: Vec<PathBuf> = ["", "-journal", "-wal", "-shm"]
            .iter()
            .map(|suffix| old_dir.join(format!("{OLD_CACHE_FILE}{suffix}")))
            .collect();
        for file in &old_files {
            std::fs::write(file, "lunch 0, the text of a message").expect("write old cache");
        }
        let (_state, _seen) = statuses_of_a_finished_build(paths);
        for file in &old_files {
            assert!(!file.exists(), "{} is still there", file.display());
        }
        assert!(!old_dir.exists(), "the empty msgs folder is still there");
    }
}
