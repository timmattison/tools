//! The state of the view of `faulte tui`.
//!
//! [`App`] is one pure value. A key or an update changes it, and nothing else
//! does. It holds no terminal and no clock, so a test drives it with plain
//! data and reads the result.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::history::{EntryId, Gap, History, ProcessKey, Reading, SpanWidth, Update};

/// How far `[` and `]` jump.
const JUMP: Duration = Duration::from_secs(60);

/// The rows of one screen before the view states its size.
const DEFAULT_PAGE_ROWS: usize = 20;

/// Which time the view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// The newest frame. A new frame moves the view.
    Live,
    /// One entry of the past. A new frame does not move the view.
    Past(EntryId),
}

/// The row that the person selected. It names a process, so that the
/// selection stays on that process while the rows reorder.
#[derive(Debug, Clone, Copy)]
struct RowSelection {
    /// The process of the row.
    key: ProcessKey,
    /// The last known position of the row. When the process has no row in the
    /// reading, this is a position inside the rows, so that a move starts
    /// near the place where the row was.
    index: usize,
}

/// The way that a move of the row selection goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Way {
    /// Toward the first row.
    Back,
    /// Toward the last row.
    Forward,
}

/// The state of the view.
pub struct App {
    /// Every entry that the sampler gave.
    history: History,
    /// The time that the view shows.
    selection: Selection,
    /// How much history the table sums.
    span: SpanWidth,
    /// The selected row, if any.
    row: Option<RowSelection>,
    /// Whether the detail view of the selected row is open.
    detail_open: bool,
    /// How many lines the detail text is scrolled.
    detail_scroll: u16,
    /// How many table rows fit on one screen. It is never 0.
    page_rows: usize,
    /// The reading that the table shows.
    reading: Option<Reading>,
}

impl App {
    /// Makes the state over `history`, in the live view.
    #[must_use]
    pub fn new(history: History) -> Self {
        let mut app = Self {
            history,
            selection: Selection::Live,
            span: SpanWidth::OneFrame,
            row: None,
            detail_open: false,
            detail_scroll: 0,
            page_rows: DEFAULT_PAGE_ROWS,
            reading: None,
        };
        app.refresh();
        app
    }

    /// Computes the reading again. Each change of the selection, the span, or
    /// the history calls it, so `reading` never goes stale.
    fn refresh(&mut self) {
        let end = match self.selection {
            Selection::Live => self.history.newest_frame(),
            Selection::Past(id) => Some(id),
        };
        self.reading = end.and_then(|end| self.history.span(end, self.span));
        self.follow_row();
    }

    /// Keeps the row selection on its process. When the process has a row,
    /// the index is that position. When it has none, the key stays and the
    /// index moves into the rows, so that the row comes back when the person
    /// moves to a frame that holds it.
    fn follow_row(&mut self) {
        let Some(selection) = &mut self.row else {
            return;
        };
        let rows = self
            .reading
            .as_ref()
            .map_or(0, |reading| reading.rows().len());
        selection.index = self
            .reading
            .as_ref()
            .and_then(|reading| reading.position_of(selection.key))
            .unwrap_or_else(|| selection.index.min(rows.saturating_sub(1)));
    }

    /// Moves the row selection by `by` rows. The first move selects the first
    /// row. A move stops at each end.
    fn move_row(&mut self, way: Way, by: usize) {
        let Some(reading) = &self.reading else {
            return;
        };
        let rows = reading.rows().len();
        let target = match (&self.row, way) {
            (None, _) => 0,
            (Some(selection), Way::Back) => selection.index.saturating_sub(by),
            (Some(selection), Way::Forward) => {
                (selection.index.saturating_add(by)).min(rows.saturating_sub(1))
            }
        };
        if let Some(view) = reading.row(target) {
            self.row = Some(RowSelection {
                key: view.key,
                index: target,
            });
        }
    }

    /// Adds an update to the history.
    pub fn on_update(&mut self, update: Update) {
        let _ = update;
    }

    /// Applies a key.
    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                let target = self
                    .selected_entry()
                    .and_then(|base| self.history.previous(base));
                self.go_past(target);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if let Selection::Past(id) = self.selection {
                    self.go_past(self.history.next(id));
                }
            }
            KeyCode::Char('[') => {
                let target = self
                    .selected_entry()
                    .and_then(|base| self.history.back_by(base, JUMP));
                self.go_past(target);
            }
            KeyCode::Char(']') => {
                if let Selection::Past(id) = self.selection {
                    let target = self
                        .history
                        .forward_by(id, JUMP)
                        .or_else(|| self.history.newest());
                    self.go_past(target);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_row(Way::Back, 1),
            KeyCode::Down | KeyCode::Char('j') => self.move_row(Way::Forward, 1),
            KeyCode::PageUp => {
                if self.detail_open {
                    self.detail_scroll = self.detail_scroll.saturating_sub(self.page_lines());
                } else {
                    self.move_row(Way::Back, self.page_rows);
                }
            }
            KeyCode::PageDown => {
                if self.detail_open {
                    self.detail_scroll = self.detail_scroll.saturating_add(self.page_lines());
                } else {
                    self.move_row(Way::Forward, self.page_rows);
                }
            }
            KeyCode::Home => self.go_past(self.history.oldest()),
            KeyCode::End => self.go_live(),
            KeyCode::Esc => {
                if self.detail_open {
                    self.detail_open = false;
                } else {
                    self.go_live();
                }
            }
            KeyCode::Enter => {
                if self.selected_row().is_some() {
                    self.detail_open = true;
                    self.detail_scroll = 0;
                }
            }
            _ => {}
        }
    }

    /// Gives the lines of one screen of the detail text.
    fn page_lines(&self) -> u16 {
        u16::try_from(self.page_rows).unwrap_or(u16::MAX)
    }

    /// Selects the live view.
    fn go_live(&mut self) {
        self.selection = Selection::Live;
        self.refresh();
    }

    /// Selects the past entry `target`. `None` means that the move found no
    /// entry, and the selection stays.
    fn go_past(&mut self, target: Option<EntryId>) {
        if let Some(id) = target {
            self.selection = Selection::Past(id);
            self.refresh();
        }
    }

    /// Gives the reading that the table shows.
    #[must_use]
    pub fn reading(&self) -> Option<&Reading> {
        self.reading.as_ref()
    }

    /// Gives the history.
    #[must_use]
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Gives the selected time.
    #[must_use]
    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// Gives the width of the span.
    #[must_use]
    pub fn span(&self) -> SpanWidth {
        self.span
    }

    /// Gives the entry that the marker of the timeline shows.
    #[must_use]
    pub fn selected_entry(&self) -> Option<EntryId> {
        match self.selection {
            Selection::Live => self.history.newest(),
            Selection::Past(id) => Some(id),
        }
    }

    /// Gives the gap at the selected entry.
    #[must_use]
    pub fn selected_gap(&self) -> Option<&Gap> {
        self.history.gap(self.selected_entry()?)
    }

    /// Gives the position of the selected row in the reading, or `None` when
    /// no row is selected or the selected process has no row.
    #[must_use]
    pub fn selected_row(&self) -> Option<usize> {
        let selection = self.row?;
        self.reading.as_ref()?.position_of(selection.key)
    }

    /// Tells whether the detail view of the selected row is open.
    #[must_use]
    pub fn detail_open(&self) -> bool {
        self.detail_open
    }

    /// Gives how many lines the detail text is scrolled.
    #[must_use]
    pub fn detail_scroll(&self) -> u16 {
        self.detail_scroll
    }

    /// Sets how many table rows fit on one screen.
    pub fn set_page_rows(&mut self, rows: usize) {
        self.page_rows = rows.max(1);
    }

    /// Gives the key of the selected row.
    #[must_use]
    pub fn row_key(&self) -> Option<ProcessKey> {
        self.row.map(|selection| selection.key)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::{Duration, SystemTime};

    use crossterm::event::KeyModifiers;

    use super::*;
    use crate::history::SampledFrame;
    use crate::machine::Observed;
    use crate::pid::{Pid, Uid};
    use crate::ranking::{ClaudeTotal, ClaudeView, RankedRow, Ranking, Skipped};
    use crate::render::Accounts;
    use crate::vm::{SwapUsage, VmDelta};

    /// The window of a test frame, in seconds.
    const WINDOW_SECS: u64 = 2;

    /// The account of every test row.
    const ACCOUNT: Uid = Uid::new(501);

    /// Gives the time `secs` seconds after the Unix epoch.
    fn time(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// Makes a row of a process that is not a session.
    fn row(pid: u32, faults: u64) -> RankedRow {
        RankedRow {
            pid: Pid::new(pid),
            uid: ACCOUNT,
            faults,
            rss_kib: Some(1_000),
            started_at_epoch_secs: Some(1_700_000_000),
            command: format!("/bin/command-{pid}"),
            claude: ClaudeView::NotClaude,
        }
    }

    /// Makes the update that holds one sample at `secs`.
    fn frame(secs: u64, rows: Vec<RankedRow>) -> Update {
        let window = Duration::from_secs(WINDOW_SECS);
        let total_faults = rows.iter().map(|row| row.faults).sum();
        Update::Frame(Box::new(SampledFrame {
            observed: Observed {
                ranking: Ranking {
                    rows,
                    window,
                    total_faults,
                    claude: ClaudeTotal {
                        processes: 0,
                        other_account: 0,
                        faults: 0,
                    },
                    skipped: Skipped::default(),
                },
                table: Vec::new(),
                swap: VmDelta::default(),
                usage: SwapUsage::default(),
                compressor_bytes: 0,
                swap_window: window,
                accounts: Accounts::new(),
                now: time(secs),
                interval: "2s".parse().expect("2s is a span"),
            },
            at: time(secs),
            own: BTreeSet::new(),
        }))
    }

    /// Makes the update that holds one gap at `secs`.
    fn gap(secs: u64, lost_secs: u64) -> Update {
        Update::Gap(Gap {
            at: time(secs),
            lost: Duration::from_secs(lost_secs),
            reason: "top stopped".to_string(),
        })
    }

    /// Makes the state over the updates, and gives the name of each entry.
    fn app_with(updates: Vec<Update>) -> (App, Vec<EntryId>) {
        let mut history = History::new();
        let ids = updates.into_iter().map(|u| history.add(u)).collect();
        (App::new(history), ids)
    }

    /// Makes a key press without modifiers.
    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A new state shows the newest frame, live.
    #[test]
    fn a_new_state_shows_the_newest_frame_live() {
        let (app, ids) = app_with(vec![frame(0, vec![row(1, 5)]), frame(2, vec![row(2, 9)])]);

        assert_eq!(app.selection(), Selection::Live);
        assert_eq!(app.span(), SpanWidth::OneFrame);
        assert_eq!(app.reading().map(Reading::end), Some(ids[1]));
        assert_eq!(app.selected_entry(), Some(ids[1]));
    }

    /// Builds five entries, two seconds apart, with a gap at the third.
    fn five() -> (App, Vec<EntryId>) {
        app_with(vec![
            frame(0, vec![row(1, 1)]),
            frame(2, vec![row(1, 1)]),
            gap(4, 2),
            frame(6, vec![row(1, 1)]),
            frame(8, vec![row(1, 1)]),
        ])
    }

    /// `Left` and `h` select the entry before the selected one, a gap
    /// included. The live view counts from the newest entry.
    #[test]
    fn left_and_h_select_the_previous_entry() {
        let (mut app, ids) = five();

        app.on_key(press(KeyCode::Left));
        assert_eq!(app.selection(), Selection::Past(ids[3]));
        app.on_key(press(KeyCode::Char('h')));
        assert_eq!(app.selection(), Selection::Past(ids[2]));
        assert_eq!(app.selected_entry(), Some(ids[2]));
        assert_eq!(app.selected_gap().map(|gap| gap.lost.as_secs()), Some(2));
        assert_eq!(app.reading().map(Reading::end), None);
    }

    /// `Right` and `l` select the entry after the selected one. In the live
    /// view they do nothing, and at the newest entry they stay in the past.
    #[test]
    fn right_and_l_select_the_next_entry() {
        let (mut app, ids) = five();

        app.on_key(press(KeyCode::Right));
        assert_eq!(app.selection(), Selection::Live);

        for _ in 0..4 {
            app.on_key(press(KeyCode::Left));
        }
        assert_eq!(app.selection(), Selection::Past(ids[0]));
        app.on_key(press(KeyCode::Right));
        assert_eq!(app.selection(), Selection::Past(ids[1]));
        app.on_key(press(KeyCode::Char('l')));
        assert_eq!(app.selection(), Selection::Past(ids[2]));
    }

    /// Builds six frames, forty seconds apart.
    fn forty_apart() -> (App, Vec<EntryId>) {
        app_with(
            (0..6)
                .map(|step| frame(step * 40, vec![row(1, 1)]))
                .collect(),
        )
    }

    /// `[` selects the newest entry that is at least 60 s older than the
    /// selected one, and stays when no entry is old enough.
    #[test]
    fn the_left_bracket_goes_back_sixty_seconds() {
        let (mut app, ids) = forty_apart();

        app.on_key(press(KeyCode::Char('[')));
        assert_eq!(app.selection(), Selection::Past(ids[3]));
        app.on_key(press(KeyCode::Char('[')));
        assert_eq!(app.selection(), Selection::Past(ids[1]));
        app.on_key(press(KeyCode::Char('[')));
        assert_eq!(app.selection(), Selection::Past(ids[1]));
    }

    /// `]` selects the oldest entry at least 60 s newer than the selected one,
    /// or the newest entry when none is far enough. In the live view it does
    /// nothing, and it never returns to the live view.
    #[test]
    fn the_right_bracket_goes_forward_sixty_seconds() {
        let (mut app, ids) = forty_apart();

        app.on_key(press(KeyCode::Char(']')));
        assert_eq!(app.selection(), Selection::Live);

        app.on_key(press(KeyCode::Char('[')));
        app.on_key(press(KeyCode::Char('[')));
        assert_eq!(app.selection(), Selection::Past(ids[1]));
        app.on_key(press(KeyCode::Char(']')));
        assert_eq!(app.selection(), Selection::Past(ids[3]));
        app.on_key(press(KeyCode::Char(']')));
        assert_eq!(app.selection(), Selection::Past(ids[5]));
        app.on_key(press(KeyCode::Char(']')));
        assert_eq!(app.selection(), Selection::Past(ids[5]));

        app.on_key(press(KeyCode::Left));
        assert_eq!(app.selection(), Selection::Past(ids[4]));
        app.on_key(press(KeyCode::Char(']')));
        assert_eq!(app.selection(), Selection::Past(ids[5]));
    }

    /// `Home` selects the oldest entry.
    #[test]
    fn home_selects_the_oldest_entry() {
        let (mut app, ids) = five();

        app.on_key(press(KeyCode::Home));
        assert_eq!(app.selection(), Selection::Past(ids[0]));
        assert_eq!(app.reading().map(Reading::end), Some(ids[0]));
    }

    /// `End` returns to the live view from any past entry.
    #[test]
    fn end_returns_to_the_live_view() {
        let (mut app, ids) = five();

        app.on_key(press(KeyCode::Home));
        app.on_key(press(KeyCode::End));
        assert_eq!(app.selection(), Selection::Live);
        assert_eq!(app.reading().map(Reading::end), Some(ids[4]));
    }

    /// `Esc` returns to the live view when no detail view is open.
    #[test]
    fn escape_returns_to_the_live_view() {
        let (mut app, _) = five();

        app.on_key(press(KeyCode::Home));
        app.on_key(press(KeyCode::Esc));
        assert_eq!(app.selection(), Selection::Live);
    }

    /// Builds one live frame with three rows, from the most faults down.
    fn three_rows() -> App {
        app_with(vec![frame(0, vec![row(10, 9), row(20, 5), row(30, 1)])]).0
    }

    /// Gives the PID of the selected row.
    fn selected_pid(app: &App) -> Option<u32> {
        app.row_key().map(|key| key.pid.get())
    }

    /// The arrows and `j` and `k` move the selected row by one, and stop at
    /// each end. No row is selected at the start, and either key then selects
    /// the first row.
    #[test]
    fn up_and_down_move_the_selected_row() {
        let mut app = three_rows();
        assert_eq!(app.selected_row(), None);

        app.on_key(press(KeyCode::Down));
        assert_eq!(
            (app.selected_row(), selected_pid(&app)),
            (Some(0), Some(10))
        );
        app.on_key(press(KeyCode::Char('j')));
        assert_eq!(
            (app.selected_row(), selected_pid(&app)),
            (Some(1), Some(20))
        );
        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::Down));
        assert_eq!(
            (app.selected_row(), selected_pid(&app)),
            (Some(2), Some(30))
        );
        app.on_key(press(KeyCode::Up));
        app.on_key(press(KeyCode::Char('k')));
        app.on_key(press(KeyCode::Up));
        assert_eq!(
            (app.selected_row(), selected_pid(&app)),
            (Some(0), Some(10))
        );

        let mut fresh = three_rows();
        fresh.on_key(press(KeyCode::Up));
        assert_eq!(fresh.selected_row(), Some(0));
    }

    /// `Enter` opens the detail view only when a row is selected. The first
    /// `Esc` closes it and keeps the time. The second `Esc` goes live.
    #[test]
    fn enter_opens_the_detail_and_escape_closes_it_first() {
        let mut app = three_rows();

        app.on_key(press(KeyCode::Enter));
        assert!(!app.detail_open());

        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::Enter));
        assert!(app.detail_open());

        app.on_key(press(KeyCode::Esc));
        assert!(!app.detail_open());
        assert_eq!(selected_pid(&app), Some(10));

        let (mut app, ids) = five();
        app.on_key(press(KeyCode::Left));
        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::Enter));
        app.on_key(press(KeyCode::Esc));
        assert_eq!(app.selection(), Selection::Past(ids[3]));
        app.on_key(press(KeyCode::Esc));
        assert_eq!(app.selection(), Selection::Live);
    }

    /// Builds one live frame with ten rows, from the most faults down.
    fn ten_rows() -> App {
        let rows = (0..10)
            .map(|step| row(100 + step, 50 - u64::from(step)))
            .collect();
        app_with(vec![frame(0, rows)]).0
    }

    /// `PageUp` and `PageDown` move the selected row by one screen. With the
    /// detail view open they scroll its text by one screen instead.
    #[test]
    fn page_keys_move_a_screen_or_scroll_the_detail() {
        let mut app = ten_rows();
        app.set_page_rows(4);

        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::PageDown));
        assert_eq!(app.selected_row(), Some(4));
        app.on_key(press(KeyCode::PageDown));
        app.on_key(press(KeyCode::PageDown));
        assert_eq!(app.selected_row(), Some(9));
        app.on_key(press(KeyCode::PageUp));
        assert_eq!(app.selected_row(), Some(5));

        app.on_key(press(KeyCode::Enter));
        app.on_key(press(KeyCode::PageDown));
        app.on_key(press(KeyCode::PageDown));
        assert_eq!((app.detail_scroll(), app.selected_row()), (8, Some(5)));
        app.on_key(press(KeyCode::PageUp));
        assert_eq!(app.detail_scroll(), 4);
        app.on_key(press(KeyCode::PageUp));
        app.on_key(press(KeyCode::PageUp));
        assert_eq!(app.detail_scroll(), 0);
    }
}
