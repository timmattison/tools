//! The state machine of the chf screen.
//!
//! The app has no terminal access, no thread, and no clock. Its input is an
//! event: a key, a batch of index events, a result of the search, or the size
//! of the last draw. Its output is a new state and a list of effects. A test
//! drives the app with key events and reads its state.

use std::sync::Arc;

use claude_transcript::SessionId;
use crossterm::event::KeyEvent;

use crate::index::IndexEvent;
use crate::query::Query;
use crate::search::{Hit, Hits, Job};
use crate::session::Session;

/// The version of a session to resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    /// The compacted conversation. The `ch` command resumes it.
    Compacted,
    /// The expanded conversation. The `che` command resumes it.
    Expanded,
}

/// What the screen shows and which keys it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The list of sessions, the query, and the preview.
    List,
    /// The question which version of a compacted session to resume.
    Ask {
        /// The catalog index of the session that the question is about.
        session: usize,
        /// The version that Enter chooses.
        choice: Version,
    },
}

/// A job for the caller of the app.
#[derive(Debug, Clone)]
pub enum Effect {
    /// Run this search in the background and give the hits to `App::on_hits`.
    Search(Job),
    /// End the program.
    Quit,
    /// Resume this session and end the program.
    Launch {
        /// The id of the session.
        id: SessionId,
        /// The version to resume.
        version: Version,
    },
}

/// How far the index has read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    /// The count of files read, the unreadable files included.
    pub read: usize,
    /// The count of files found, when the index has said it.
    pub total: Option<usize>,
    /// True when the index has finished.
    pub done: bool,
    /// The count of files that the index could not read.
    pub unreadable: usize,
}

/// What the last draw measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drawn {
    /// The count of rows that the list showed.
    pub list_rows: u16,
    /// The scroll of the preview after the draw clamped it.
    pub preview_scroll: i64,
}

/// The state of the screen.
pub struct App {
    query_text: String,
    query: Arc<Query>,
    generation: u64,
    catalog: Vec<Arc<Session>>,
    hits: Vec<Hit>,
    selected: usize,
    list_offset: usize,
    list_rows: u16,
    mode: Mode,
    progress: Progress,
    preview_scroll: i64,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// An app with an empty catalog, an empty query, and generation 0.
    #[must_use]
    pub fn new() -> Self {
        App {
            query_text: String::new(),
            query: Arc::new(Query::parse("")),
            generation: 0,
            catalog: Vec::new(),
            hits: Vec::new(),
            selected: 0,
            list_offset: 0,
            list_rows: 1,
            mode: Mode::List,
            progress: Progress::default(),
            preview_scroll: 0,
        }
    }

    /// Take a key and give the effects.
    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let _ = key;
        Vec::new()
    }

    /// Take a batch of index events and give the effects.
    pub fn on_index(&mut self, events: Vec<IndexEvent>) -> Vec<Effect> {
        let _ = events;
        Vec::new()
    }

    /// Take the result of a search.
    pub fn on_hits(&mut self, hits: Hits) {
        let _ = hits;
    }

    /// Take the size that the last draw measured.
    pub fn on_drawn(&mut self, drawn: Drawn) {
        let _ = drawn;
    }

    /// The text of the query.
    #[must_use]
    pub fn query_text(&self) -> &str {
        &self.query_text
    }

    /// The parsed query.
    #[must_use]
    pub fn query(&self) -> &Arc<Query> {
        &self.query
    }

    /// The generation of the query.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The list, newest session first.
    #[must_use]
    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }

    /// The session at `index` in the catalog.
    ///
    /// # Panics
    ///
    /// Panics when `index` is not a catalog index. A `Hit` always holds a
    /// good index.
    #[must_use]
    pub fn session(&self, index: usize) -> &Arc<Session> {
        &self.catalog[index]
    }

    /// The count of sessions in the catalog.
    #[must_use]
    pub fn sessions_len(&self) -> usize {
        self.catalog.len()
    }

    /// The selected row of the list. It is 0 for an empty list.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The hit in the selected row.
    #[must_use]
    pub fn selected_hit(&self) -> Option<&Hit> {
        self.hits.get(self.selected)
    }

    /// The first row of the list that shows.
    #[must_use]
    pub fn list_offset(&self) -> usize {
        self.list_offset
    }

    /// The count of rows that the list shows. It is 1 before the first draw.
    #[must_use]
    pub fn list_rows(&self) -> u16 {
        self.list_rows
    }

    /// What the screen shows.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// How far the index has read.
    #[must_use]
    pub fn progress(&self) -> Progress {
        self.progress
    }

    /// The scroll of the preview.
    #[must_use]
    pub fn preview_scroll(&self) -> i64 {
        self.preview_scroll
    }
}
