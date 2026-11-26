//! The report of the definitions.

use super::{Tally, Window};

/// Gives the text report.
#[must_use]
pub fn text(_tally: &Tally, _window: &Window, _shell: &str, _width: Option<u16>) -> String {
    String::new()
}

/// Gives the JSON document.
#[must_use]
pub fn json(_tally: &Tally, _window: &Window, _shell: &str) -> String {
    String::new()
}
