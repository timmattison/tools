//! `occ` — "old Claude Code": find the Claude Code sessions running on this
//! machine and report them oldest release first.
//!
//! A long-lived Claude Code session keeps running the release it started on. A
//! machine that upgrades often therefore accumulates sessions spread across many
//! releases, and the oldest of them are the ones worth attention. This crate
//! answers, for every running session: which release it runs, where it works,
//! which recorded session it belongs to, and how long it has been open. One
//! [`Report`] carries the whole answer, because it also counts the Claude Code
//! processes that are not sessions, so a run can say what it left out.
//!
//! The crate separates two concerns that fail in different ways. Gathering
//! process facts from the operating system lives in [`scan`], and every rule
//! applied to those facts is a pure function over [`ProcessFact`] values, so the
//! classification rules are testable without a running session.
//!
//! Which session a process belongs to is not worked out at all. A live session
//! records its own identity in `~/.claude/sessions/<pid>.json`, and [`registry`]
//! reads it. A process that recorded nothing is reported without a session
//! rather than given a guessed one.

pub mod process;
pub mod registry;
pub mod report;
pub mod scan;
pub mod session;
pub mod version;

pub use scan::gather_processes;

pub use process::{classify, version_of, ProcessFact, Role};
pub use registry::{Registry, SessionRegistry};
pub use report::{build, format_uptime, Report, SessionReport};
pub use session::SessionId;
pub use version::ClaudeVersion;
