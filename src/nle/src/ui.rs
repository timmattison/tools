//! The Dioxus user interface of the NLE application.
//!
//! This module holds the window settings, the root component, and the four
//! regions of the window: the menu bar, the preview pane, the timeline, and
//! the properties panel. The open recording lives in the state of `App`.

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;

use crate::types::LoadedRecording;

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
    rsx! {}
}

/// The preview of the open recording.
#[component]
fn PreviewPane(loaded: ReadSignal<Option<LoadedRecording>>) -> Element {
    let _ = loaded;
    rsx! {}
}

/// The timeline of the open recording.
#[component]
fn Timeline(loaded: ReadSignal<Option<LoadedRecording>>) -> Element {
    let _ = loaded;
    rsx! {}
}

/// The metadata of the open recording.
#[component]
fn PropertiesPanel(loaded: ReadSignal<Option<LoadedRecording>>) -> Element {
    let _ = loaded;
    rsx! {}
}

#[cfg(test)]
mod tests;
