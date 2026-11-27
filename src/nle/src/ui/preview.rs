//! The preview pane: the terminal of the edited recording at the playhead.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use dioxus::prelude::*;

use super::Document;
use crate::format::format_duration;
use crate::preview::playback::{advance, play_from};
use crate::preview::screen::{hex, Screen};
use crate::preview::{Preview, PreviewTheme};
use crate::types::{Recording, Seconds};

/// The text of the pane when no recording is open.
const EMPTY_PREVIEW: &str = "Open a recording to preview it here";
/// The label of the transport button while the recording is paused or stopped.
const PLAY_LABEL: &str = "Play";
/// The label of the transport button while the recording plays.
const PAUSE_LABEL: &str = "Pause";
/// The label of the button that stops playback and returns to the start.
const STOP_LABEL: &str = "Stop";
/// The time between two moves of the playhead during playback. It gives about
/// 30 frames each second.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
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

/// The ticker of the playback. It moves the playhead while `playing` is true.
///
/// The ticker reads `playing` first, so the resource subscribes to it. Each
/// change of `playing` cancels the old task and starts a new one. Thus two
/// tickers never run together. The loop reads the playhead and the render
/// with `peek`. A read that subscribes would restart the resource at each
/// write of the playhead, and the ticker would never reach its next step.
/// When the playhead reaches the end, the ticker sets `playing` to false.
fn use_ticker(
    rendered: ReadSignal<RenderedState>,
    mut playhead: Signal<Seconds>,
    mut playing: Signal<bool>,
) {
    use_resource(move || async move {
        if !playing() {
            return;
        }
        let mut last = tokio::time::Instant::now();
        loop {
            tokio::time::sleep(FRAME_INTERVAL).await;
            let now = tokio::time::Instant::now();
            let elapsed = now.saturating_duration_since(last);
            last = now;
            let end = end_of(&rendered.peek());
            let (next, reached_end) = advance(*playhead.peek(), elapsed, end);
            playhead.set(next);
            if reached_end {
                playing.set(false);
                return;
            }
        }
    });
}

/// The preview of the open recording: the terminal at the playhead, and the
/// transport toolbar. The pane keeps one `Preview` engine between renders. It
/// replaces the engine when the rendered recording changes.
#[component]
pub(super) fn PreviewPane(
    rendered: ReadSignal<RenderedState>,
    mut playhead: Signal<Seconds>,
    mut playing: Signal<bool>,
) -> Element {
    use_ticker(rendered, playhead, playing);
    let end = end_of(&rendered.read());
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
    // The playhead held at or below the end, as the readout shows it.
    let shown = {
        let at = playhead();
        let held = if at > end { end } else { at };
        format!(
            "{} / {}",
            format_duration(held.get()),
            format_duration(end.get())
        )
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
                    div { class: "preview-toolbar",
                        button {
                            class: "transport",
                            onclick: move |_| {
                                if playing() {
                                    playing.set(false);
                                } else {
                                    playhead.set(play_from(playhead(), end));
                                    playing.set(true);
                                }
                            },
                            if playing() {
                                {PAUSE_LABEL}
                            } else {
                                {PLAY_LABEL}
                            }
                        }
                        button {
                            class: "transport",
                            onclick: move |_| {
                                playing.set(false);
                                playhead.set(Seconds::ZERO);
                            },
                            {STOP_LABEL}
                        }
                        span { class: "preview-time", "{shown}" }
                    }
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
