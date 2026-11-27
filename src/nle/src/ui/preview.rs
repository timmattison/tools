//! The preview pane: the terminal of the edited recording at the playhead.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use dioxus::prelude::*;

use super::Document;
use crate::preview::screen::{hex, Screen};
use crate::preview::{Preview, PreviewTheme};
use crate::types::{Recording, Seconds};

/// The text of the pane when no recording is open.
const EMPTY_PREVIEW: &str = "Open a recording to preview it here";
/// The start of the message for a project that does not render.
pub(super) const PREVIEW_FAILED_PREFIX: &str = "The project does not render";

/// The rendered recording of the open document. Two values are equal only
/// when they hold the same `Arc`, so a memo of it compares in O(1) and never
/// compares the events.
#[derive(Clone)]
pub(super) struct Rendered(pub(super) Arc<Recording>);

impl PartialEq for Rendered {
    /// Two values are equal when they share one recording.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What the preview shows: nothing (no document), the recording, or the text
/// of the error of the render.
pub(super) type RenderedState = Option<Result<Rendered, String>>;

/// The render of the project of `document`. The answer is `None` for no
/// document, and the text of the error for a project that does not render.
pub(super) fn render_of(document: &Option<Document>) -> RenderedState {
    document.as_ref().map(|open| {
        open.editor
            .render()
            .map(|recording| Rendered(Arc::new(recording)))
            .map_err(|error| error.to_string())
    })
}

/// The end of the timeline of the render: its duration, or zero for no
/// recording or a failed render.
pub(super) fn end_of(rendered: &RenderedState) -> Seconds {
    match rendered {
        Some(Ok(Rendered(recording))) => Seconds::new(recording.duration).unwrap_or(Seconds::ZERO),
        _ => Seconds::ZERO,
    }
}

/// The preview of the open recording: the terminal at the playhead. The pane
/// keeps one `Preview` engine between renders. It replaces the engine when
/// the rendered recording changes.
#[component]
pub(super) fn PreviewPane(
    rendered: ReadSignal<RenderedState>,
    playhead: ReadSignal<Seconds>,
) -> Element {
    // The engine is not reactive. A write to it never starts a render.
    let engine = use_hook(|| Rc::new(RefCell::new(None::<Preview>)));
    let screen = match &*rendered.read() {
        Some(Ok(recording)) => {
            let at = playhead();
            let theme = PreviewTheme::default();
            let mut slot = engine.borrow_mut();
            let current = slot.take().filter(|preview| {
                Arc::ptr_eq(preview.recording(), &recording.0) && preview.theme() == theme
            });
            let preview = slot
                .insert(current.unwrap_or_else(|| Preview::new(Arc::clone(&recording.0), theme)));
            Some(Ok(Screen::of(preview.seek(at))))
        }
        Some(Err(error)) => Some(Err(error.clone())),
        None => None,
    };
    rsx! {
        div { class: "preview-pane",
            match screen {
                None => rsx! {
                    div { class: "placeholder", {EMPTY_PREVIEW} }
                },
                Some(Err(error)) => rsx! {
                    div { class: "placeholder", "{PREVIEW_FAILED_PREFIX}: {error}" }
                },
                Some(Ok(screen)) => rsx! {
                    div {
                        class: "preview-screen",
                        style: "background-color:{hex(screen.background)}",
                        div {
                            class: "terminal",
                            style: "--columns:{screen.columns};--rows:{screen.rows.len()}",
                            for (index, row) in screen.rows.iter().enumerate() {
                                div { class: "terminal-row", key: "{index}",
                                    for (column, run) in row.runs.iter().enumerate() {
                                        span {
                                            key: "{column}",
                                            class: if run.single_cell { "cell" } else { "" },
                                            style: "{run.style.css()}",
                                            "{run.text}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}
