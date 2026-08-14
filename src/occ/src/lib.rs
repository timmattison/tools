//! `occ` — "old Claude Code": find the Claude Code sessions running on this
//! machine and report them oldest release first.
//!
//! A long-lived Claude Code session keeps running the release it started on. A
//! machine that upgrades often therefore accumulates sessions spread across many
//! releases, and the oldest of them are the ones worth attention. This crate
//! answers, for every running session: which release it runs, where it works,
//! which recorded session it belongs to, and how long it has been open.
//!
//! The crate separates two concerns that fail in different ways. Gathering
//! process facts from the operating system lives in [`scan`], and every rule
//! applied to those facts is a pure function over [`ProcessFact`] values, so the
//! classification and attribution rules are testable without a running session.

pub mod process;
pub mod report;
pub mod scan;
pub mod session;
pub mod version;

pub use scan::{gather_processes, ProjectTranscripts};

pub use process::{classify, version_of, ProcessFact, Role};
pub use report::{build, format_uptime, SessionReport, Transcripts};
pub use session::{attribute, session_id_from_arguments, Session, SessionId, Transcript};
pub use version::ClaudeVersion;
