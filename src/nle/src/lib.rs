mod commands;
mod error;
mod types;

use commands::{export_video, export_web, list_recordings, load_recording, save_recording};

/// Run the Tauri application.
///
/// # Panics
/// Panics if the Tauri application fails to initialize.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            load_recording,
            save_recording,
            list_recordings,
            export_video,
            export_web,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
