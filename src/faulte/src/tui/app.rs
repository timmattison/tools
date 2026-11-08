//! The state of the view of `faulte tui`.
//!
//! [`App`] is one pure value. A key or an update changes it, and nothing else
//! does. It holds no terminal and no clock, so a test drives it with plain
//! data and reads the result.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::history::{EntryId, Gap, History, ProcessKey, Reading, SpanWidth, Update};

/// Which time the view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// The newest frame. A new frame moves the view.
    Live,
    /// One entry of the past. A new frame does not move the view.
    Past(EntryId),
}

/// The state of the view.
pub struct App {
    /// Every entry that the sampler gave.
    history: History,
    /// The time that the view shows.
    selection: Selection,
    /// How much history the table sums.
    span: SpanWidth,
    /// The reading that the table shows.
    reading: Option<Reading>,
}

impl App {
    /// Makes the state over `history`, in the live view.
    #[must_use]
    pub fn new(history: History) -> Self {
        Self {
            history,
            selection: Selection::Live,
            span: SpanWidth::OneFrame,
            reading: None,
        }
    }

    /// Adds an update to the history.
    pub fn on_update(&mut self, update: Update) {
        let _ = update;
    }

    /// Applies a key.
    pub fn on_key(&mut self, key: KeyEvent) {
        let _ = (key, KeyCode::Esc, KeyEventKind::Press, KeyModifiers::NONE);
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
        None
    }

    /// Gives the gap at the selected entry.
    #[must_use]
    pub fn selected_gap(&self) -> Option<&Gap> {
        None
    }

    /// Gives the key of the selected row.
    #[must_use]
    pub fn row_key(&self) -> Option<ProcessKey> {
        None
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
}
