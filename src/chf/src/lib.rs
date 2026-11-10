//! chf finds a Claude Code session by a word and resumes it.
//!
//! Claude Code keeps each session as a JSONL transcript. A person who
//! remembers a word from a session can not find it by its id. chf reads every
//! transcript once, keeps the text of the user and assistant messages in an
//! index, and searches that index for the word. The person then picks a
//! session, and `ch` or `che` resumes it.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod excerpt;
pub mod extract;
pub mod index;
pub mod query;
pub mod search;
pub mod session;
