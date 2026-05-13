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
//! Zellij currently emits **KDL v1** for `session-layout.kdl`, but the wider
//! Rust ecosystem is migrating to KDL v2. This crate enables `kdl`'s
//! `v1-fallback` feature, so documents are parsed as v2 first and re-parsed
//! as v1 on failure — either dialect is accepted, with the same surface
//! semantics. The parser accepts the subset of Zellij's `session-layout.kdl`
//! format that is observably stable across recent releases:
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

use kdl::{KdlDocument, KdlNode};
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
pub fn derive_tuple(env: &Env, layout: &LayoutText) -> Result<Tuple, TupleError> {
    let doc: KdlDocument = layout
        .0
        .parse()
        .map_err(|e: kdl::KdlError| TupleError::LayoutParse(e.to_string()))?;

    // Collect tabs. The document may be wrapped in a top-level `layout { ... }`
    // node, or it may contain `tab` nodes directly.
    let tabs: Vec<&KdlNode> = collect_tabs(&doc);
    if tabs.is_empty() {
        return Err(TupleError::NoTabs);
    }

    // For every tab, run a fresh walker and record any ordinals at which the
    // target pane id was matched. Multiple matches across tabs (or within a
    // single tab) constitute an ambiguous layout.
    let mut matches: Vec<(&KdlNode, u32)> = Vec::new();
    for tab in &tabs {
        let mut walker = TabWalker::new(&env.zellij_pane_id);
        walker.walk_tab(tab);
        for ord in walker.matched_ordinals {
            matches.push((tab, ord));
        }
    }

    if matches.len() > 1 {
        return Err(TupleError::AmbiguousPaneId {
            pane_id: env.zellij_pane_id.clone(),
        });
    }

    let (tab, ordinal) = matches
        .into_iter()
        .next()
        .ok_or_else(|| TupleError::PaneNotFound {
            pane_id: env.zellij_pane_id.clone(),
        })?;

    let tab_name = tab_name_of(tab).ok_or_else(|| TupleError::TabNameMissing {
        pane_id: env.zellij_pane_id.clone(),
    })?;

    Ok(Tuple {
        zellij_session_name: ZellijSessionName(env.zellij_session_name.clone()),
        tab_name: TabName(tab_name),
        pane_ordinal_within_tab: PaneOrdinal(ordinal),
    })
}

/// Find all `tab` nodes in the document. Tolerates both
/// `layout { tab ...; tab ...; }` and bare top-level `tab` nodes.
fn collect_tabs(doc: &KdlDocument) -> Vec<&KdlNode> {
    let mut out = Vec::new();
    for node in doc.nodes() {
        match node.name().value() {
            "tab" => out.push(node),
            "layout" => {
                if let Some(children) = node.children() {
                    for inner in children.nodes() {
                        if inner.name().value() == "tab" {
                            out.push(inner);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Extract the `name=` attribute of a tab node, if present.
fn tab_name_of(tab: &KdlNode) -> Option<String> {
    tab.entries()
        .iter()
        .find(|e| e.name().map(kdl::KdlIdentifier::value) == Some("name"))
        .and_then(|e| e.value().as_string().map(String::from))
}

/// Walks a single tab and records ordinals + which ordinals matched the
/// caller's target pane id.
struct TabWalker<'a> {
    target_pane_id: &'a str,
    next_ordinal: u32,
    matched_ordinals: Vec<u32>,
}

impl<'a> TabWalker<'a> {
    fn new(target_pane_id: &'a str) -> Self {
        Self {
            target_pane_id,
            next_ordinal: 0,
            matched_ordinals: Vec::new(),
        }
    }

    fn walk_tab(&mut self, tab: &KdlNode) {
        let Some(children) = tab.children() else {
            return;
        };

        // First pass: tiled panes. Recurse into every direct `pane` child.
        for node in children.nodes() {
            if node.name().value() == "pane" {
                self.visit_pane(node);
            }
        }

        // Second pass: floating panes, appended after tiled.
        for node in children.nodes() {
            if node.name().value() == "floating_panes" {
                if let Some(fc) = node.children() {
                    for inner in fc.nodes() {
                        if inner.name().value() == "pane" {
                            self.visit_pane(inner);
                        }
                    }
                }
            }
        }
    }

    /// Visit one pane. Decides whether it is a plugin pane (skip),
    /// a container (recurse without ordinaling), or a terminal pane
    /// (assign ordinal, possibly recording a match).
    fn visit_pane(&mut self, pane: &KdlNode) {
        if is_plugin_pane(pane) {
            return;
        }

        if let Some(children) = pane.children() {
            // A pane with `pane` child nodes is a container. Walk in
            // document order; non-`pane` children (e.g. comments, future
            // metadata) are skipped without affecting numbering.
            let has_pane_child = children
                .nodes()
                .iter()
                .any(|n| n.name().value() == "pane");

            if has_pane_child {
                for inner in children.nodes() {
                    if inner.name().value() == "pane" {
                        self.visit_pane(inner);
                    }
                }
                return;
            }
            // Otherwise treat as a terminal pane (children are some
            // non-pane block other than `plugin`, which was caught above).
        }

        // Terminal pane (tiled, floating, or suppressed — all ordinaled).
        let ordinal = self.next_ordinal;
        self.next_ordinal += 1;

        if let Some(id) = pane_id_string(pane) {
            if id == self.target_pane_id {
                self.matched_ordinals.push(ordinal);
            }
        }
    }
}

/// A pane is a plugin pane if either:
/// - it has an attribute named `plugin`, or
/// - it has a child node named `plugin`.
fn is_plugin_pane(pane: &KdlNode) -> bool {
    let has_plugin_attr = pane
        .entries()
        .iter()
        .any(|e| e.name().map(kdl::KdlIdentifier::value) == Some("plugin"));
    if has_plugin_attr {
        return true;
    }
    if let Some(children) = pane.children() {
        if children
            .nodes()
            .iter()
            .any(|n| n.name().value() == "plugin")
        {
            return true;
        }
    }
    false
}

/// Stringify the `id=` attribute on a pane, regardless of whether KDL parsed
/// it as integer or string.
fn pane_id_string(pane: &KdlNode) -> Option<String> {
    let entry = pane
        .entries()
        .iter()
        .find(|e| e.name().map(kdl::KdlIdentifier::value) == Some("id"))?;
    let v = entry.value();
    if let Some(s) = v.as_string() {
        return Some(s.to_string());
    }
    if let Some(i) = v.as_integer() {
        return Some(i.to_string());
    }
    None
}
