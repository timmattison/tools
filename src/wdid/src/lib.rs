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
//! - [`history`] gets the full atuin history as a list of rows.
//! - [`rules`] gives the runs that one command word makes.
//! - [`shell`] gets the aliases and the functions of the interactive shell.
//! - [`tally`] counts the uses of each tool.
//! - [`tool`] holds the names of the tools of the workspace.

pub mod catalog;
pub mod command_words;
pub mod history;
pub mod rules;
pub mod shell;
pub mod tally;
pub mod tool;
