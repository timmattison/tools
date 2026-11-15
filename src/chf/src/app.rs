//! The state machine of the chf screen.
//!
//! The app has no terminal access, no thread, and no clock. Its input is an
//! event: a key, a batch of index events, a result of the search, or the size
//! of the last draw. Its output is a new state and a list of effects. A test
//! drives the app with key events and reads its state.

use std::sync::Arc;

use claude_transcript::SessionId;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

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
    /// The generation of the hits that the list holds.
    shown_generation: u64,
    catalog: Vec<Arc<Session>>,
    hits: Vec<Hit>,
    selected: usize,
    /// The id of the session that the person moved the selection to. It is
    /// `None` until the first move, so an untouched selection stays on row 0,
    /// the newest session. After a move, the app follows this session when the
    /// list changes.
    selected_id: Option<SessionId>,
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
            shown_generation: 0,
            catalog: Vec::new(),
            hits: Vec::new(),
            selected: 0,
            selected_id: None,
            list_offset: 0,
            list_rows: 1,
            mode: Mode::List,
            progress: Progress::default(),
            preview_scroll: 0,
        }
    }

    /// Take a key and give the effects.
    ///
    /// A key release does nothing. A press and a repeat act. The mode decides
    /// which keys the app takes.
    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        match self.mode {
            Mode::List => self.on_list_key(key),
            Mode::Ask { session, choice } => self.on_ask_key(key, session, choice),
        }
    }

    /// Take a batch of index events and give the effects.
    ///
    /// When the batch holds sessions, the result is one search over only the
    /// new sessions, at the current generation.
    pub fn on_index(&mut self, events: Vec<IndexEvent>) -> Vec<Effect> {
        let first_new = self.catalog.len();
        for event in events {
            match event {
                IndexEvent::Total(total) => self.progress.total = Some(total),
                IndexEvent::Session(session) => {
                    self.catalog.push(Arc::new(*session));
                    self.progress.read += 1;
                }
                IndexEvent::Unreadable { .. } => {
                    self.progress.read += 1;
                    self.progress.unreadable += 1;
                }
                IndexEvent::Done => self.progress.done = true,
            }
        }
        if self.catalog.len() == first_new {
            return Vec::new();
        }
        vec![self.search_from(first_new)]
    }

    /// Take the result of a search.
    ///
    /// Hits of an old generation are dropped. The first hits of the current
    /// generation replace the list. Later hits merge into it.
    pub fn on_hits(&mut self, hits: Hits) {
        if hits.generation != self.generation {
            return;
        }
        let shown_before = self.id_at(self.selected);
        if self.shown_generation == self.generation {
            for hit in hits.hits {
                match self.hits.iter_mut().find(|old| old.session == hit.session) {
                    Some(old) => *old = hit,
                    None => self.hits.push(hit),
                }
            }
        } else {
            self.hits = hits.hits;
            self.shown_generation = self.generation;
        }
        let catalog = &self.catalog;
        self.hits.sort_by(|a, b| {
            let (a, b) = (&catalog[a.session], &catalog[b.session]);
            b.last_activity
                .cmp(&a.last_activity)
                .then_with(|| a.id.cmp(&b.id))
        });
        self.reselect();
        if self.id_at(self.selected) != shown_before {
            self.preview_scroll = 0;
        }
    }

    /// Take the size that the last draw measured.
    pub fn on_drawn(&mut self, drawn: Drawn) {
        self.list_rows = drawn.list_rows.max(1);
        self.preview_scroll = drawn.preview_scroll;
        self.fix_offset();
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

impl App {
    /// A search job at the current generation over the catalog from `first`.
    fn search_from(&self, first: usize) -> Effect {
        let sessions = self
            .catalog
            .iter()
            .enumerate()
            .skip(first)
            .map(|(index, session)| (index, Arc::clone(session)))
            .collect();
        Effect::Search(Job {
            generation: self.generation,
            query: Arc::clone(&self.query),
            sessions,
        })
    }

    /// Start a new generation for the current query text.
    fn change_query(&mut self) -> Vec<Effect> {
        self.generation += 1;
        self.query = Arc::new(Query::parse(&self.query_text));
        self.preview_scroll = 0;
        vec![self.search_from(0)]
    }

    fn on_list_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let shifted = key.modifiers.contains(KeyModifiers::SHIFT);
        let page = isize::try_from(self.list_rows).unwrap_or(isize::MAX);
        match key.code {
            KeyCode::Char('c') if control => vec![Effect::Quit],
            KeyCode::Esc => vec![Effect::Quit],
            KeyCode::Char('u') if control => {
                if self.query_text.is_empty() {
                    return Vec::new();
                }
                self.query_text.clear();
                self.change_query()
            }
            KeyCode::Char('p') if control => self.move_selection(-1),
            KeyCode::Char('n') if control => self.move_selection(1),
            KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => {
                self.query_text.push(c);
                self.change_query()
            }
            KeyCode::Backspace => {
                if self.query_text.pop().is_none() {
                    return Vec::new();
                }
                self.change_query()
            }
            KeyCode::Up if shifted => self.scroll_preview(-1),
            KeyCode::Down if shifted => self.scroll_preview(1),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-page),
            KeyCode::PageDown => self.move_selection(page),
            KeyCode::Enter => self.choose(),
            _ => Vec::new(),
        }
    }

    fn on_ask_key(&mut self, key: KeyEvent, session: usize, choice: Version) -> Vec<Effect> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
        let launch = |version| {
            vec![Effect::Launch {
                id: self.catalog[session].id.clone(),
                version,
            }]
        };
        match key.code {
            KeyCode::Char('c') if control => vec![Effect::Quit],
            KeyCode::Char('c') if plain => launch(Version::Compacted),
            KeyCode::Char('e') if plain => launch(Version::Expanded),
            KeyCode::Enter => launch(choice),
            KeyCode::Up | KeyCode::Down => {
                let other = match choice {
                    Version::Compacted => Version::Expanded,
                    Version::Expanded => Version::Compacted,
                };
                self.mode = Mode::Ask {
                    session,
                    choice: other,
                };
                Vec::new()
            }
            KeyCode::Esc => {
                self.mode = Mode::List;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Enter in the list: launch the selected session, or ask which version.
    fn choose(&mut self) -> Vec<Effect> {
        let Some(hit) = self.selected_hit() else {
            return Vec::new();
        };
        let index = hit.session;
        let session = &self.catalog[index];
        if session.compaction.compacted {
            self.mode = Mode::Ask {
                session: index,
                choice: Version::Compacted,
            };
            return Vec::new();
        }
        vec![Effect::Launch {
            id: session.id.clone(),
            version: Version::Compacted,
        }]
    }

    fn scroll_preview(&mut self, delta: i64) -> Vec<Effect> {
        self.preview_scroll = self.preview_scroll.saturating_add(delta);
        Vec::new()
    }

    /// Move the selection by `delta` rows and stop at both ends.
    fn move_selection(&mut self, delta: isize) -> Vec<Effect> {
        let last = self.hits.len().saturating_sub(1);
        let row = self.selected.saturating_add_signed(delta).min(last);
        if row != self.selected {
            self.selected = row;
            self.preview_scroll = 0;
            self.selected_id = self.id_at(row);
            self.fix_offset();
        }
        Vec::new()
    }

    fn id_at(&self, row: usize) -> Option<SessionId> {
        self.hits
            .get(row)
            .map(|hit| self.catalog[hit.session].id.clone())
    }

    /// Keep the selection right after the list changes. An untouched selection
    /// stays on its row, which is row 0. A moved selection follows its session.
    /// When that session left the list, the selection keeps its row number,
    /// clamped, and the app then follows the session now in that row. This
    /// function does not touch the preview scroll.
    fn reselect(&mut self) {
        let followed = self.selected_id.as_ref().and_then(|id| {
            self.hits
                .iter()
                .position(|hit| &self.catalog[hit.session].id == id)
        });
        if let Some(row) = followed {
            self.selected = row;
        } else {
            self.selected = self.selected.min(self.hits.len().saturating_sub(1));
            if self.selected_id.is_some() {
                self.selected_id = self.id_at(self.selected);
            }
        }
        self.fix_offset();
    }

    /// Keep the selected row inside the rows that the list shows.
    fn fix_offset(&mut self) {
        let rows = usize::from(self.list_rows);
        if self.selected < self.list_offset {
            self.list_offset = self.selected;
        } else if self.selected >= self.list_offset + rows {
            self.list_offset = self.selected + 1 - rows;
        }
    }
}
