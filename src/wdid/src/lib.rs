//! `wdid` — "what do i do": count which tools of this workspace the atuin
//! history uses.
//!
//! A workspace of many small tools makes one question hard to answer: which of
//! these tools does the person really use? The shell history holds the answer.
//! `wdid` reads that history through atuin, and it counts each tool.
//!
//! # The modules
//! - [`catalog`] reads the tools of the workspace: names, crate directories,
//!   added dates, and install state.
//! - [`command_words`] gives the command words of one command line.
//! - [`defs`] counts the uses of each alias and each function of the shell.
//! - [`history`] gets the full atuin history as a list of rows.
//! - [`render`] writes the report as text, as JSON, and as a list of the
//!   commands that did not parse.
//! - [`rules`] gives the runs that one command word makes.
//! - [`shell`] gets the aliases and the functions of the interactive shell.
//! - [`tally`] counts the uses of each tool.
//! - [`tool`] holds the names of the tools of the workspace.
//! - [`workspace`] finds the workspace to report on, and the step that found
//!   it.

pub mod catalog;
pub mod command_words;
pub mod defs;
pub mod history;
pub mod render;
pub mod rules;
pub mod shell;
pub mod tally;
pub mod tool;
pub mod workspace;
