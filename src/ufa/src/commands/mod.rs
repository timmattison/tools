// A note goes through `crate::output::Report`, never straight to standard
// error. `Report::print` prints each note after the document, so a note is
// what the user reads last, and a test reads the notes from the report.
#![deny(clippy::print_stderr)]

pub mod clients;
pub mod devices;
pub mod info;
pub mod sites;
pub mod vouchers;
