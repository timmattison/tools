//! The terminal preview: the state of the screen at any time of a recording.

use std::sync::Arc;

use termstate::{TerminalState, TerminalTheme};

use crate::types::{Recording, Seconds};

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
        ""
    }

    /// The text that a person reads for this theme.
    #[must_use]
    pub fn label(self) -> &'static str {
        ""
    }

    /// The theme with this exact `name`, or `None`.
    #[must_use]
    pub fn from_name(_name: &str) -> Option<Self> {
        None
    }

    /// The palette of the emulator for this theme.
    #[must_use]
    pub fn palette(self) -> TerminalTheme {
        TerminalTheme::auto()
    }
}

/// The terminal state of a recording at any time.
pub struct Preview {
    recording: Arc<Recording>,
    theme: PreviewTheme,
    current: TerminalState,
}

impl Preview {
    /// A preview of `recording` drawn with `theme`. It replays nothing yet.
    #[must_use]
    pub fn new(recording: Arc<Recording>, theme: PreviewTheme) -> Self {
        Self {
            recording,
            theme,
            current: TerminalState::new(80, 24, theme.palette()),
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
    pub fn seek(&mut self, _at: Seconds) -> &TerminalState {
        &self.current
    }

    /// The count of events fed to an emulator since `new`.
    #[cfg(test)]
    pub(crate) fn events_replayed(&self) -> usize {
        0
    }

    /// The count of snapshots that the preview holds.
    #[cfg(test)]
    pub(crate) fn snapshot_count(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests;
