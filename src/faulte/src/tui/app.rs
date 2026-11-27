//! The state of the view of `faulte tui`.
//!
//! [`App`] is one pure value. A key, an update, or the room that the draw
//! reports changes it, and nothing else does. It holds no terminal and no
//! clock, so a test drives it with plain data and reads the result.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::history::{EntryId, Gap, History, ProcessKey, Reading, SpanWidth, Update};

/// How far `[` and `]` jump.
const JUMP: Duration = Duration::from_secs(60);

/// The rows of one screen before the view states its size.
const DEFAULT_PAGE_ROWS: usize = 20;

/// The notice that tells the person that the selected frame left the history.
const LEFT_NOTICE: &str =
    "the selected frame left the history, so the view moved to the oldest frame";

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
    /// How many lines the command area of the detail view had at the last
    /// draw.
    detail_page: u16,
    /// The largest scroll of the detail text at the last draw: the wrapped
    /// lines of the command minus `detail_page`, never below 0.
    detail_last: u16,
    /// The position of the first row that the table draws.
    top_row: usize,
    /// How many table rows fit on one screen. It is never 0.
    page_rows: usize,
    /// Whether the person asked to leave.
    quit: bool,
    /// A note for the person. The next key clears it.
    notice: Option<String>,
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
            detail_page: 0,
            detail_last: 0,
            top_row: 0,
            page_rows: DEFAULT_PAGE_ROWS,
            quit: false,
            notice: None,
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
    /// row. A move stops at each end. A move that selects another process
    /// scrolls the detail text back to its first line.
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
            if self.row_key() != Some(view.key) {
                self.detail_scroll = 0;
            }
            self.row = Some(RowSelection {
                key: view.key,
                index: target,
            });
        }
    }

    /// Adds an update to the history.
    pub fn on_update(&mut self, update: Update) {
        self.history.add(update);
        let left = matches!(self.selection, Selection::Past(id) if !self.history.contains(id));
        if left {
            if let Some(oldest) = self.history.oldest() {
                self.selection = Selection::Past(oldest);
            }
            self.notice = Some(LEFT_NOTICE.to_string());
        }
        self.refresh();
        self.keep_selection_on_screen();
    }

    /// Applies a key.
    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        self.notice = None;
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
                    self.detail_scroll = self.detail_scroll.saturating_sub(self.detail_page.max(1));
                } else {
                    self.move_row(Way::Back, self.page_rows);
                }
            }
            KeyCode::PageDown => {
                if self.detail_open {
                    self.detail_scroll = self
                        .detail_scroll
                        .saturating_add(self.detail_page.max(1))
                        .min(self.detail_last);
                } else {
                    self.move_row(Way::Forward, self.page_rows);
                }
            }
            KeyCode::Char('w') => {
                self.span = self.span.next();
                self.refresh();
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.quit = true;
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
            KeyCode::Enter if self.row_key().is_some() => {
                self.detail_open = true;
                self.detail_scroll = 0;
            }
            _ => {}
        }
        self.keep_selection_on_screen();
    }

    /// Moves `top_row` so that the selected row is on the screen. With no
    /// selected row, `top_row` is 0.
    fn keep_selection_on_screen(&mut self) {
        self.top_row = match self.selected_row() {
            None => 0,
            Some(selected) if selected < self.top_row => selected,
            Some(selected) if selected >= self.top_row + self.page_rows => {
                selected + 1 - self.page_rows
            }
            Some(_) => self.top_row,
        };
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

    /// Tells whether the person asked to leave.
    #[must_use]
    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// Gives the notice that the view shows until the next key, if any.
    #[must_use]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
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

    /// Gives the entry that the marker of the timeline shows. The live view
    /// gives the newest frame, which the table shows, also when a gap came
    /// after it. With no frame, it gives the newest entry.
    #[must_use]
    pub fn selected_entry(&self) -> Option<EntryId> {
        match self.selection {
            Selection::Live => self
                .history
                .newest_frame()
                .or_else(|| self.history.newest()),
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
        self.keep_selection_on_screen();
    }

    /// Tells the app how the detail view drew its command: `page` lines fit in
    /// the command area, and the wrapped command has `lines` lines. The page
    /// keys step by `page` and stop at the end of the text. The scroll is
    /// clamped now, so a resize never leaves the text scrolled past its end.
    pub fn set_detail_room(&mut self, page: u16, lines: u16) {
        self.detail_page = page;
        self.detail_last = lines.saturating_sub(page);
        self.detail_scroll = self.detail_scroll.min(self.detail_last);
    }

    /// Gives the position of the first row that the table draws.
    #[must_use]
    pub fn top_row(&self) -> usize {
        self.top_row
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

    /// The live view shows the newest frame, also when a gap came after it.
    /// The marker of the timeline stands on that frame, and Left goes to the
    /// entry before it, so the key changes the table.
    #[test]
    fn the_live_view_marks_the_newest_frame_and_not_a_later_gap() {
        let (mut app, ids) = app_with(vec![
            frame(0, vec![row(1, 5)]),
            frame(2, vec![row(2, 9)]),
            gap(4, 2),
        ]);

        assert_eq!(app.reading().map(Reading::end), Some(ids[1]));
        assert_eq!(app.selected_entry(), Some(ids[1]));

        app.on_key(press(KeyCode::Left));

        assert_eq!(app.selection(), Selection::Past(ids[0]));
        assert_eq!(app.reading().map(Reading::end), Some(ids[0]));
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
    /// included. The live view counts from the newest frame.
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

    /// A process that made no faults in the newest frame has no row there.
    /// The person selected that process, so `Enter` still opens it. On a Mac
    /// with short-lived processes, the frame changed before the key arrived,
    /// and the key did nothing.
    #[test]
    fn enter_opens_a_selected_process_that_has_no_row_in_this_frame() {
        let mut app = three_rows();
        app.on_key(press(KeyCode::Down));
        let selected = app.row_key();
        app.on_update(frame(2, vec![row(99, 5)]));
        assert_eq!(app.selected_row(), None);

        app.on_key(press(KeyCode::Enter));

        assert!(app.detail_open());
        assert_eq!(app.row_key(), selected);
    }

    /// Builds one live frame with ten rows, from the most faults down.
    fn ten_rows() -> App {
        let rows = (0..10)
            .map(|step| row(100 + step, 50 - u64::from(step)))
            .collect();
        app_with(vec![frame(0, rows)]).0
    }

    /// `PageUp` and `PageDown` move the selected row by one screen. With the
    /// detail view open they scroll its text by the lines of the command area
    /// instead.
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
        app.set_detail_room(4, 20);
        app.on_key(press(KeyCode::PageDown));
        app.on_key(press(KeyCode::PageDown));
        assert_eq!((app.detail_scroll(), app.selected_row()), (8, Some(5)));
        app.on_key(press(KeyCode::PageUp));
        assert_eq!(app.detail_scroll(), 4);
        app.on_key(press(KeyCode::PageUp));
        app.on_key(press(KeyCode::PageUp));
        assert_eq!(app.detail_scroll(), 0);
        for _ in 0..6 {
            app.on_key(press(KeyCode::PageDown));
        }
        assert_eq!(app.detail_scroll(), 16, "the scroll stops at the last line");
    }

    /// `w` cycles the span, and the cycle ends at one frame again.
    #[test]
    fn w_cycles_the_span() {
        let (mut app, _) = forty_apart();
        let mut seen = Vec::new();

        for _ in 0..5 {
            app.on_key(press(KeyCode::Char('w')));
            seen.push(app.span());
        }
        assert_eq!(
            seen,
            [
                SpanWidth::OneMinute,
                SpanWidth::FiveMinutes,
                SpanWidth::FifteenMinutes,
                SpanWidth::Whole,
                SpanWidth::OneFrame,
            ]
        );
    }

    /// `q` and `Ctrl-C` ask to quit. A plain `c` does not.
    #[test]
    fn q_and_control_c_quit() {
        let (mut app, _) = five();
        app.on_key(press(KeyCode::Char('c')));
        assert!(!app.should_quit());

        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit());

        let (mut app, _) = five();
        app.on_key(press(KeyCode::Char('q')));
        assert!(app.should_quit());
    }

    /// A new frame moves the live view, and does not move a selected past
    /// frame.
    #[test]
    fn a_new_frame_does_not_move_a_past_frame() {
        let (mut live, _) = five();
        live.on_update(frame(10, vec![row(1, 1)]));
        let newest = live.history().newest_frame();
        assert_eq!(live.reading().map(Reading::end), newest);

        let (mut past, ids) = five();
        past.on_key(press(KeyCode::Left));
        past.on_update(frame(10, vec![row(1, 1)]));
        assert_eq!(past.selection(), Selection::Past(ids[3]));
        assert_eq!(past.reading().map(Reading::end), Some(ids[3]));
        assert_ne!(past.history().newest(), Some(ids[4]));
    }

    /// Builds a history that is full: one more frame makes the oldest leave.
    fn full_history() -> (History, Vec<EntryId>) {
        let make = |budget: usize| {
            let mut history = History::with_budget(budget);
            let ids: Vec<EntryId> = (0..3)
                .map(|step| history.add(frame(step * 2, vec![row(1, 1), row(2, 2)])))
                .collect();
            (history, ids)
        };
        let (probe, _) = make(usize::MAX);
        make(probe.bytes_used())
    }

    /// A selected frame that leaves the history moves the selection to the
    /// oldest frame and sets a notice. The next key clears the notice.
    #[test]
    fn a_selected_frame_that_leaves_moves_to_the_oldest() {
        let (history, ids) = full_history();
        let mut app = App::new(history);
        app.on_key(press(KeyCode::Home));
        assert_eq!(app.selection(), Selection::Past(ids[0]));
        assert_eq!(app.notice(), None);

        app.on_update(frame(6, vec![row(1, 1), row(2, 2)]));
        assert!(!app.history().contains(ids[0]));
        let oldest = app.history().oldest().expect("the history is not empty");
        assert_eq!(app.selection(), Selection::Past(oldest));
        assert_eq!(app.reading().map(Reading::end), Some(oldest));
        assert_eq!(
            app.notice(),
            Some("the selected frame left the history, so the view moved to the oldest frame")
        );

        app.on_key(press(KeyCode::Char('w')));
        assert_eq!(app.notice(), None);
    }

    /// `top_row` keeps the selected row on the screen, and is 0 with no
    /// selection. A new page size keeps the row on the screen too.
    #[test]
    fn the_top_row_keeps_the_selection_on_screen() {
        let mut app = ten_rows();
        app.set_page_rows(4);
        assert_eq!(app.top_row(), 0);

        app.on_key(press(KeyCode::Down));
        assert_eq!(app.top_row(), 0);
        app.on_key(press(KeyCode::PageDown));
        assert_eq!((app.selected_row(), app.top_row()), (Some(4), 1));
        app.on_key(press(KeyCode::PageDown));
        app.on_key(press(KeyCode::PageDown));
        assert_eq!((app.selected_row(), app.top_row()), (Some(9), 6));

        app.set_page_rows(2);
        assert_eq!(app.top_row(), 8);

        for _ in 0..4 {
            app.on_key(press(KeyCode::Up));
        }
        assert_eq!((app.selected_row(), app.top_row()), (Some(5), 5));
    }

    /// A move stops at each end. `Right` at the newest entry stays in the past
    /// and does not return to the live view.
    #[test]
    fn a_move_stops_at_each_end() {
        let (mut app, ids) = five();

        app.on_key(press(KeyCode::Home));
        app.on_key(press(KeyCode::Left));
        app.on_key(press(KeyCode::Char('[')));
        assert_eq!(app.selection(), Selection::Past(ids[0]));

        for _ in 0..6 {
            app.on_key(press(KeyCode::Right));
        }
        assert_eq!(app.selection(), Selection::Past(ids[4]));
        app.on_key(press(KeyCode::Char(']')));
        assert_eq!(app.selection(), Selection::Past(ids[4]));
    }

    /// The row selection stays on one process while the rows reorder. A
    /// process with no row stays selected, and its row comes back with the
    /// frame that holds it.
    #[test]
    fn the_selection_stays_on_one_process() {
        let (mut app, _) = app_with(vec![frame(0, vec![row(1, 9), row(2, 5)])]);
        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::Down));
        assert_eq!((selected_pid(&app), app.selected_row()), (Some(2), Some(1)));

        app.on_update(frame(2, vec![row(2, 20), row(1, 9)]));
        assert_eq!((selected_pid(&app), app.selected_row()), (Some(2), Some(0)));

        app.on_update(frame(4, vec![row(1, 9)]));
        assert_eq!((selected_pid(&app), app.selected_row()), (Some(2), None));

        app.on_key(press(KeyCode::Left));
        assert_eq!((selected_pid(&app), app.selected_row()), (Some(2), Some(0)));
    }

    /// A span ends at the selected frame, in the past and in the live view.
    #[test]
    fn a_span_ends_at_the_selected_frame() {
        let (mut app, ids) = forty_apart();

        app.on_key(press(KeyCode::Char('w')));
        assert_eq!(app.reading().map(Reading::end), Some(ids[5]));

        app.on_key(press(KeyCode::Char('[')));
        assert_eq!(app.selection(), Selection::Past(ids[3]));
        assert_eq!(app.reading().map(Reading::end), Some(ids[3]));
    }

    /// A key that is not a press changes nothing.
    #[test]
    fn a_key_release_changes_nothing() {
        let (mut app, _) = five();

        app.on_key(KeyEvent::new_with_kind(
            KeyCode::Left,
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        app.on_key(KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        ));
        assert_eq!(app.selection(), Selection::Live);
        assert!(!app.should_quit());
    }

    /// With no history, each key does nothing and none panics.
    #[test]
    fn no_history_and_no_panic_for_each_key() {
        let mut app = App::new(History::new());
        let codes = [
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Char('h'),
            KeyCode::Char('l'),
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('['),
            KeyCode::Char(']'),
            KeyCode::Char('w'),
        ];
        for code in codes {
            app.on_key(press(code));
        }
        assert_eq!(app.selection(), Selection::Live);
        assert!(app.reading().is_none());
        assert_eq!(app.selected_entry(), None);
        assert_eq!(app.selected_row(), None);
        assert!(!app.detail_open());
        assert_eq!(app.top_row(), 0);

        app.set_page_rows(0);
        app.on_update(gap(0, 2));
        app.on_key(press(KeyCode::Down));
        assert!(app.selected_gap().is_some());
        assert!(app.reading().is_none());
    }
}
