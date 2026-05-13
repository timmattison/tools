//! Pure derivation of the Zellij (session, tab, pane-ordinal) tuple from
//! environment values plus a Zellij `session-layout.kdl` document.
//!
//! This crate is intentionally I/O-free: callers are responsible for reading
//! `$ZELLIJ_SESSION_NAME`, `$ZELLIJ_PANE_ID`, and the layout file from disk,
//! then handing the strings to [`derive_tuple`]. The function returns a typed
//! [`Tuple`] suitable for hashing into a deterministic session id.
//!
//! # Locked-down ordinal policy
//!
//! Within a single tab, terminal panes are assigned monotonically-increasing
//! [`PaneOrdinal`] values starting at `0`, in the following order
//! (also captured in [`POLICY`] for runtime / debugging surfaces):
//!
//! 1. Walk the tab's tiled pane tree depth-first, left-to-right
//!    (the order children appear in the KDL document).
//! 2. **Plugin panes are skipped entirely.** A pane with a `plugin { ... }`
//!    child node, or with a `plugin` attribute, has no shell and therefore
//!    no ordinal. Its sub-tree (typically empty) is not recursed into.
//! 3. **Suppressed panes are included in tree-DFS position.** A pane with
//!    `suppressed=true` is still a terminal pane and still receives an
//!    ordinal at the moment it is visited by the DFS.
//! 4. **Container panes do not get an ordinal.** A `pane` node that has
//!    child `pane` nodes (regardless of `split_direction=`) is a tiling
//!    container; only its leaf descendants are ordinaled.
//! 5. **Floating panes are appended after all tiled panes** within the same
//!    tab, walking the contents of the `floating_panes { ... }` block in
//!    document order.
//!
//! # Format support
//!
//! The parser accepts the subset of Zellij's `session-layout.kdl` format
//! that is observably stable across recent releases:
//!
//! ```kdl
//! layout {
//!     tab name="editor" focus=true {
//!         pane split_direction="vertical" {
//!             pane id=1 command="nvim"
//!             pane id=2 cwd="/tmp"
//!         }
//!         floating_panes {
//!             pane id=99 x=10 y=10 width=80 height=24
//!         }
//!     }
//!     tab name="logs" {
//!         pane id=3
//!         pane id=4 suppressed=true
//!     }
//! }
//! ```
//!
//! Specifically:
//!
//! - The top-level wrapper may be `layout { ... }`. If the document instead
//!   contains bare `tab` nodes, they are accepted as siblings.
//! - `tab name="<utf8>" { ... }` defines a tab. Additional attributes such as
//!   `focus=true` are tolerated and ignored.
//! - `pane` defines either a terminal pane (leaf) or a container.
//!   Attributes such as `id=<n>`, `command=...`, `cwd=...`,
//!   `split_direction=...`, and `borderless=true` are tolerated.
//! - `pane id=N` matches `$ZELLIJ_PANE_ID` by **string equality** on `N`.
//! - `pane { plugin { ... } }` and `pane plugin="..."` mark plugin panes
//!   (skipped, never ordinaled).
//! - `pane suppressed=true` marks a suppressed terminal pane (ordinaled).
//! - `floating_panes { pane ...; pane ...; }` is walked AFTER the tiled tree.

use std::fmt;

use thiserror::Error;

/// Human-readable summary of the ordinal-assignment policy used by
/// [`derive_tuple`]. Suitable for printing from `tsm doctor`.
pub const POLICY: &str = "\
tiled panes ordinaled depth-first left-to-right; \
plugin panes skipped; \
suppressed panes ordinaled at their tree position; \
floating panes appended after all tiled panes in document order";

/// Zellij session name (`$ZELLIJ_SESSION_NAME`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ZellijSessionName(String);

impl From<String> for ZellijSessionName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl AsRef<str> for ZellijSessionName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ZellijSessionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Zellij tab name (the `name=` attribute on a `tab` node).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TabName(String);

impl From<String> for TabName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl AsRef<str> for TabName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TabName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Stable ordinal of a terminal pane within its enclosing tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneOrdinal(u32);

impl PaneOrdinal {
    /// The underlying integer value.
    pub fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for PaneOrdinal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The full coordinate that deterministically identifies a Zellij pane
/// across resurrections of the same session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tuple {
    /// The Zellij session that the pane belongs to.
    pub zellij_session_name: ZellijSessionName,
    /// The tab the pane lives in, by its `name=` attribute.
    pub tab_name: TabName,
    /// Stable ordinal of the pane within its tab (see [`POLICY`]).
    pub pane_ordinal_within_tab: PaneOrdinal,
}

/// Inputs sampled from the Zellij environment.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Env {
    /// Value of `$ZELLIJ_SESSION_NAME`.
    pub zellij_session_name: String,
    /// Value of `$ZELLIJ_PANE_ID`, matched against `pane id=...` in the layout.
    pub zellij_pane_id: String,
}

/// Raw text of a Zellij `session-layout.kdl` document.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LayoutText(pub String);

/// Errors produced by [`derive_tuple`].
#[derive(Debug, Error)]
pub enum TupleError {
    /// The KDL document failed to parse.
    #[error("layout failed to parse as KDL: {0}")]
    LayoutParse(String),

    /// The layout contained no `tab` nodes at all.
    #[error("layout contains no tabs")]
    NoTabs,

    /// No `tab` in the layout contains a pane whose `id=` matches the
    /// environment's `$ZELLIJ_PANE_ID`.
    #[error("no terminal pane matched ZELLIJ_PANE_ID={pane_id:?}")]
    PaneNotFound {
        /// The pane id that failed to match.
        pane_id: String,
    },

    /// The tab that contains the matching pane has no `name=` attribute.
    #[error("tab containing pane id {pane_id:?} has no name= attribute")]
    TabNameMissing {
        /// The pane id whose tab was nameless.
        pane_id: String,
    },

    /// More than one pane in the layout shares the same `id=`.
    #[error("layout contains duplicate pane id {pane_id:?}")]
    AmbiguousPaneId {
        /// The duplicated pane id.
        pane_id: String,
    },
}

/// Derive a [`Tuple`] for the pane identified by `env` using the supplied
/// layout text.
///
/// This function is pure — it performs no I/O. All inputs are passed in.
///
/// # Errors
///
/// Returns a typed [`TupleError`] when the layout cannot be parsed, contains
/// no tabs, has no pane matching `env.zellij_pane_id`, the matching tab is
/// nameless, or the same pane id appears in more than one place.
pub fn derive_tuple(_env: &Env, _layout: &LayoutText) -> Result<Tuple, TupleError> {
    todo!("derive_tuple — implemented in green slice (#214)")
}
