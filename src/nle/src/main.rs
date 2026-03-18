// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("nle {}", buildinfo::version_string!());
        return;
    }
    nle_lib::run();
}
