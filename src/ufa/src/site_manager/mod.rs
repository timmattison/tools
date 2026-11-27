// A note goes through `crate::output::Report`, never straight to standard
// error. `Report::print` prints each note after the document, so a note is
// what the user reads last, and a test reads the notes from the report.
#![deny(clippy::print_stderr)]

pub mod client;
pub mod commands;
pub mod models;
pub mod utils;

pub use client::SiteManagerClient;
pub use commands::*;
pub use utils::is_cloud_console_url;
