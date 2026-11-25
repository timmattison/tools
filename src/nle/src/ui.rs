//! The Dioxus user interface of the NLE application.
//!
//! This module holds the window settings and the root component. The editor
//! views come in a later change.

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;

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
    rsx! {
        div { class: "app-layout" }
    }
}
