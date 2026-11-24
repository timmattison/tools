#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cache;
mod commands;
mod db;
mod error;
mod export;
mod parser;
#[cfg(test)]
mod test_fixture;
mod types;
mod util;

use commands::{AppState, AppStateInner};
use std::sync::Mutex;

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
        .manage(AppState {
            inner: Mutex::new(AppStateInner {
                db: None,
                cache: None,
            }),
        })
        .invoke_handler(tauri::generate_handler![
            commands::check_db_access,
            commands::list_conversations,
            commands::get_messages,
            commands::get_version,
            commands::search_messages,
            commands::rebuild_text_cache,
            commands::export_messages,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
