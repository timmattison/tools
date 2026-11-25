//! The Dioxus user interface of the NLE application.
//!
//! This module holds the window settings, the root component, and the four
//! regions of the window: the menu bar, the preview pane, the timeline, and
//! the properties panel. The open recording lives in the state of `App`.

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;

use crate::format::{file_name, format_duration, format_recorded};
use crate::types::{LoadedRecording, RecordingMetadata};

/// The style sheet of the window. The `style` element at the top of `App` holds
/// it, so the webview needs no extra file.
const STYLE: &str = include_str!("../assets/main.css");
/// The text for a recording with no title.
const UNTITLED: &str = "(untitled)";

/// The title of the application window.
const WINDOW_TITLE: &str = "NLE - Terminal Recording Editor";
/// The width of the application window, in logical pixels.
const WINDOW_WIDTH: f64 = 1280.0;
/// The height of the application window, in logical pixels.
const WINDOW_HEIGHT: f64 = 800.0;

/// The window settings of the application: a resizable window with the title
/// and the size above.
pub(crate) fn window_config() -> Config {
    Config::new().with_window(
        WindowBuilder::new()
            .with_title(WINDOW_TITLE)
            .with_resizable(true)
            .with_inner_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT)),
    )
}

/// The root component of the application.
#[component]
pub(crate) fn App() -> Element {
    let mut loaded = use_signal(|| None::<LoadedRecording>);
    let mut error = use_signal(|| None::<String>);

    rsx! {
        style { {STYLE} }
        div { class: "app-layout",
            MenuBar {
                on_loaded: move |recording| {
                    loaded.set(Some(recording));
                    error.set(None);
                },
                on_error: move |message| error.set(Some(message)),
            }
            PreviewPane { loaded }
            Timeline { loaded }
            PropertiesPanel { loaded }
        }
    }
}

/// The menu bar with the Open Recording button.
#[component]
fn MenuBar(on_loaded: EventHandler<LoadedRecording>, on_error: EventHandler<String>) -> Element {
    let _ = (on_loaded, on_error);
    rsx! {
        div { class: "menu-bar",
            button { "Open Recording" }
        }
    }
}

/// The preview of the open recording.
#[component]
fn PreviewPane(loaded: ReadSignal<Option<LoadedRecording>>) -> Element {
    let loaded = loaded.read();
    rsx! {
        div { class: "preview-pane",
            if let Some(open) = loaded.as_ref() {
                div { class: "placeholder",
                    "Terminal preview \u{2014} coming in #151"
                    br {}
                    "{open.recording.width}x{open.recording.height}"
                }
            } else {
                div { class: "placeholder", "Open a recording to preview it here" }
            }
        }
    }
}

/// The timeline of the open recording.
#[component]
fn Timeline(loaded: ReadSignal<Option<LoadedRecording>>) -> Element {
    let loaded = loaded.read();
    rsx! {
        div { class: "timeline",
            if let Some(open) = loaded.as_ref() {
                h3 { "Timeline" }
                div { class: "placeholder",
                    "Timeline editor \u{2014} coming in #150"
                    br {}
                    "{open.recording.events.len()} events | {format_duration(open.recording.duration)}"
                }
            } else {
                div { class: "placeholder", "No recording loaded" }
            }
        }
    }
}

/// The metadata of the open recording.
#[component]
fn PropertiesPanel(loaded: ReadSignal<Option<LoadedRecording>>) -> Element {
    let loaded = loaded.read();
    rsx! {
        div { class: "properties-panel",
            h3 { "Properties" }
            if let Some(open) = loaded.as_ref() {
                {metadata_rows(&open.metadata)}
            } else {
                div { class: "placeholder", "No recording loaded" }
            }
        }
    }
}

/// The seven rows of the properties panel for one recording.
fn metadata_rows(metadata: &RecordingMetadata) -> Element {
    let title = if metadata.title.is_empty() {
        UNTITLED
    } else {
        metadata.title.as_str()
    };
    let rows = [
        ("Title", title.to_owned(), None),
        ("Command", metadata.command.clone(), None),
        ("Duration", format_duration(metadata.duration), None),
        ("Events", metadata.event_count.to_string(), None),
        (
            "Size",
            format!("{}x{}", metadata.width, metadata.height),
            None,
        ),
        ("Recorded", format_recorded(metadata.timestamp), None),
        (
            "File",
            file_name(&metadata.file_path).to_owned(),
            Some(metadata.file_path.clone()),
        ),
    ];
    rsx! {
        for (label, value, hover) in rows {
            div { class: "metadata-item", key: "{label}",
                span { class: "metadata-label", "{label}" }
                span { class: "metadata-value", title: hover, "{value}" }
            }
        }
    }
}

#[cfg(test)]
mod tests;
