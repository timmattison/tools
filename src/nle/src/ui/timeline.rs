//! The view of the timeline: the zoom buttons, the ruler, the tracks with a
//! block for each clip, the event density of each clip, and the marks where
//! the terminal size changes.
//!
//! The view shows project time, before the filters. `App` holds the zoom and
//! the scroll, so a new recording opens with the whole project in the window.
//! The layout math is in `crate::timeline`, and this module only draws it and
//! turns the events of the webview into changes of the view.

use dioxus::prelude::*;

use super::Document;
use crate::timeline::View;

/// The view of the timeline of the open document. `view` holds the zoom and
/// the scroll.
#[component]
pub(super) fn TimelineView(loaded: ReadSignal<Option<Document>>, view: Signal<View>) -> Element {
    let _ = (loaded, view);
    rsx! {}
}

#[cfg(test)]
mod tests;
