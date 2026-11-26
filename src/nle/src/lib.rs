pub mod commands;
pub mod editor;
pub mod error;
mod format;
mod json_file;
pub mod pipeline;
mod project;
pub mod types;
mod ui;

use buildinfo::version_string;

/// Run the NLE desktop application.
///
/// It opens the Dioxus desktop window and returns when the user closes it.
pub fn run() {
    eprintln!("nle {}", version_string!());

    dioxus::LaunchBuilder::desktop()
        .with_cfg(ui::window_config())
        .with_context(ui::Services::desktop())
        .launch(ui::App);
}
