#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(test)]
mod bindings;
mod cache;
mod commands;
mod db;
mod error;
mod export;
mod parser;
mod state;
#[cfg(test)]
mod test_fixture;
mod types;
mod util;

use state::{AppState, DataPaths};

fn main() {
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("msgs {}", buildinfo::version_string!());
        return;
    }

    env_logger::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new(
            DataPaths::default_paths().map_err(|e| e.to_string()),
        ))
        .invoke_handler(tauri::generate_handler![
            commands::check_db_access,
            commands::get_cache_status,
            commands::list_conversations,
            commands::get_messages,
            commands::get_message_position,
            commands::search_messages,
            commands::export_messages,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
