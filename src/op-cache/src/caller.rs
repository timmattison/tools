//! Who asked: the ancestors of this process and the Claude session.

use serde::Serialize;

/// One process in the chain above op-cache.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ancestor {
    pub(crate) pid: u32,
    pub(crate) name: String,
    pub(crate) command: Option<String>,
}

/// The Claude Code session that started the chain.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudeSession {
    pub(crate) pid: u32,
    pub(crate) session_id: Option<String>,
    pub(crate) name: Option<String>,
}
