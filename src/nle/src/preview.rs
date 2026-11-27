//! The terminal preview: the state of the screen at any time of a recording.

use std::sync::Arc;

use termstate::{TerminalState, TerminalTheme};

use crate::types::{EventType, Recording, Seconds};

pub mod playback;
pub mod screen;

/// The `--theme` value of `beta` for `PreviewTheme::Auto`.
const THEME_NAME_AUTO: &str = "auto";
/// The `--theme` value of `beta` for `PreviewTheme::Dracula`.
const THEME_NAME_DRACULA: &str = "dracula";
/// The `--theme` value of `beta` for `PreviewTheme::Monokai`.
const THEME_NAME_MONOKAI: &str = "monokai";
/// The `--theme` value of `beta` for `PreviewTheme::SolarizedDark`.
const THEME_NAME_SOLARIZED_DARK: &str = "solarized-dark";
/// The `--theme` value of `beta` for `PreviewTheme::SolarizedLight`.
const THEME_NAME_SOLARIZED_LIGHT: &str = "solarized-light";

/// One theme of the preview: one palette of `beta export --theme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreviewTheme {
    /// The default palette.
    #[default]
    Auto,
    /// The Dracula palette.
    Dracula,
    /// The Monokai palette.
    Monokai,
    /// The Solarized Dark palette.
    SolarizedDark,
    /// The Solarized Light palette.
    SolarizedLight,
}

impl PreviewTheme {
    /// Each theme, in the order of the theme list.
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::Dracula,
        Self::Monokai,
        Self::SolarizedDark,
        Self::SolarizedLight,
    ];

    /// The `--theme` value of `beta` for this theme.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => THEME_NAME_AUTO,
            Self::Dracula => THEME_NAME_DRACULA,
            Self::Monokai => THEME_NAME_MONOKAI,
            Self::SolarizedDark => THEME_NAME_SOLARIZED_DARK,
            Self::SolarizedLight => THEME_NAME_SOLARIZED_LIGHT,
        }
    }

    /// The text that a person reads for this theme.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Dracula => "Dracula",
            Self::Monokai => "Monokai",
            Self::SolarizedDark => "Solarized Dark",
            Self::SolarizedLight => "Solarized Light",
        }
    }

    /// The theme with this exact `name`, or `None`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|theme| theme.name() == name)
    }

    /// The palette of the emulator for this theme.
    #[must_use]
    pub fn palette(self) -> TerminalTheme {
        match self {
            Self::Auto => TerminalTheme::auto(),
            Self::Dracula => TerminalTheme::dracula(),
            Self::Monokai => TerminalTheme::monokai(),
            Self::SolarizedDark => TerminalTheme::solarized_dark(),
            Self::SolarizedLight => TerminalTheme::solarized_light(),
        }
    }
}

/// The cells that all snapshots of one preview may hold in total. A snapshot is
/// one full grid, so this number limits the memory of the preview.
const SNAPSHOT_CELL_BUDGET: usize = 8_000_000;

/// The most snapshots that the spacing rule allows for the time axis and for
/// the event axis. A small grid reaches this limit. A large grid stays below it,
/// because of `SNAPSHOT_CELL_BUDGET`.
const MAX_SNAPSHOTS: usize = 256;

/// The shortest time in seconds between two snapshots.
const MIN_SNAPSHOT_INTERVAL: f64 = 1.0;

/// The fewest events between two snapshots.
const MIN_EVENTS_PER_SNAPSHOT: usize = 1000;

/// The most columns that a preview draws.
const MAX_COLUMNS: usize = 500;

/// The most rows that a preview draws.
const MAX_ROWS: usize = 500;

/// The grid size of the preview for `recording`, as `(columns, rows)`.
///
/// Each side is at least 1, because `TerminalState::new` computes `height - 1`
/// and a height of 0 panics. Each side is at most 500, because a corrupt file
/// can state 65535 by 65535, which asks for billions of cells. A value from
/// outside the program must not cause a panic or a huge allocation.
fn grid_size(recording: &Recording) -> (usize, usize) {
    (
        usize::from(recording.width).clamp(1, MAX_COLUMNS),
        usize::from(recording.height).clamp(1, MAX_ROWS),
    )
}

/// The terminal at one point of the replay.
struct Snapshot {
    /// The count of events that the state has applied.
    applied: usize,
    /// The time of the last applied event, or 0.0 when none was applied.
    time: f64,
    /// The terminal after those events.
    state: TerminalState,
}

/// The terminal state of a recording at any time.
///
/// A seek replays the events through the emulator of `termstate`. The preview
/// keeps snapshots of the terminal, so a seek does not replay from time zero.
/// A snapshot is one full grid. Thus the spacing of the snapshots depends on the
/// size of the grid, and at most `2 * budget + 1` snapshots exist. One `budget`
/// counts the snapshots of the time axis, and the other counts the snapshots of
/// the event axis. The `budget` is chosen so that the `2 * budget + 1` snapshots
/// stay in `SNAPSHOT_CELL_BUDGET` cells.
pub struct Preview {
    recording: Arc<Recording>,
    theme: PreviewTheme,
    /// The snapshots in order of `applied`. The first one is the blank terminal.
    snapshots: Vec<Snapshot>,
    /// The terminal at the point of the last seek.
    current: TerminalState,
    /// The count of events that `current` has applied.
    applied: usize,
    /// The time in seconds between two snapshots, at the least.
    interval: f64,
    /// The count of events between two snapshots, at the least.
    event_interval: usize,
    /// The count of events fed to an emulator since `new`.
    #[cfg(test)]
    replayed: usize,
}

impl Preview {
    /// A preview of `recording` drawn with `theme`. It replays nothing yet.
    #[must_use]
    pub fn new(recording: Arc<Recording>, theme: PreviewTheme) -> Self {
        let (columns, rows) = grid_size(&recording);
        let blank = TerminalState::new(columns, rows, theme.palette());
        let cells = columns.saturating_mul(rows).max(1);
        // `2 * budget + 1` snapshots (two axes and the blank one) stay in the
        // cell budget. The floor of 1 keeps the divisions below defined.
        let budget = ((SNAPSHOT_CELL_BUDGET / cells).saturating_sub(1) / 2).clamp(1, MAX_SNAPSHOTS);
        let duration = if recording.duration.is_finite() && recording.duration > 0.0 {
            recording.duration
        } else {
            0.0
        };
        // `budget` is at most `MAX_SNAPSHOTS`, so the conversion is exact.
        let interval = (duration / budget as f64).max(MIN_SNAPSHOT_INTERVAL);
        let event_interval = (recording.events.len() / budget).max(MIN_EVENTS_PER_SNAPSHOT);
        Self {
            recording,
            theme,
            snapshots: vec![Snapshot {
                applied: 0,
                time: 0.0,
                state: blank.clone(),
            }],
            current: blank,
            applied: 0,
            interval,
            event_interval,
            #[cfg(test)]
            replayed: 0,
        }
    }

    /// The recording of this preview.
    #[must_use]
    pub fn recording(&self) -> &Arc<Recording> {
        &self.recording
    }

    /// The theme of this preview.
    #[must_use]
    pub fn theme(&self) -> PreviewTheme {
        self.theme
    }

    /// The terminal after each event whose time is at or before `at`.
    ///
    /// The events are in time order. The render of the pipeline sorts them.
    pub fn seek(&mut self, at: Seconds) -> &TerminalState {
        let target = self
            .recording
            .events
            .partition_point(|event| event.time <= at.get());
        self.move_to_start(target);
        let recording = Arc::clone(&self.recording);
        let pending = recording.events.get(self.applied..target);
        for event in pending.unwrap_or_default() {
            if event.event_type == EventType::Output {
                // `process_output` never returns an error today. If it does
                // later, skip the event and keep the rest of the replay.
                self.current.process_output(&event.data).ok();
            }
            self.applied += 1;
            #[cfg(test)]
            {
                self.replayed += 1;
            }
            self.take_snapshot_if_due(event.time);
        }
        &self.current
    }

    /// Puts `current` at the best point to replay forward to `target`.
    ///
    /// The best point is the nearer one of `current` and the last snapshot that
    /// does not go past `target`. `current` is not usable when it is past
    /// `target`.
    fn move_to_start(&mut self, target: usize) {
        let after = self
            .snapshots
            .partition_point(|snapshot| snapshot.applied <= target);
        let nearest = after
            .checked_sub(1)
            .and_then(|index| self.snapshots.get(index));
        let Some(snapshot) = nearest else {
            return;
        };
        if self.applied > target || snapshot.applied > self.applied {
            self.current = snapshot.state.clone();
            self.applied = snapshot.applied;
        }
    }

    /// Pushes a snapshot of `current` when it is past the last snapshot by the
    /// time interval or by the event interval. `time` is the time of the event
    /// that `current` applied last. The snapshots stay sorted, because a
    /// snapshot is taken only past the furthest point replayed.
    fn take_snapshot_if_due(&mut self, time: f64) {
        let Some(last) = self.snapshots.last() else {
            return;
        };
        if self.applied <= last.applied {
            return;
        }
        let due =
            time - last.time >= self.interval || self.applied - last.applied >= self.event_interval;
        if due {
            self.snapshots.push(Snapshot {
                applied: self.applied,
                time,
                state: self.current.clone(),
            });
        }
    }

    /// The count of events fed to an emulator since `new`.
    #[cfg(test)]
    pub(crate) fn events_replayed(&self) -> usize {
        self.replayed
    }

    /// The count of snapshots that the preview holds.
    #[cfg(test)]
    pub(crate) fn snapshot_count(&self) -> usize {
        self.snapshots.len()
    }
}

#[cfg(test)]
mod tests;
