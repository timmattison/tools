pub mod commands;
pub mod error;
pub mod types;
mod format;
mod ui;

use buildinfo::version_string;

/// Run the NLE desktop application.
///
/// It opens the Dioxus desktop window and returns when the user closes it.
pub fn run() {
    eprintln!("nle {}", version_string!());

    dioxus::LaunchBuilder::desktop()
        .with_cfg(ui::window_config())
        .launch(ui::App);
}
