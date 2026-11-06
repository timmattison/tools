//! The state of each sidecar, as a table.
//!
//! `sidecar status` reads the main clone and the link of each sidecar. It makes no
//! network call and writes nothing.

use crate::clone::CloneState;
use crate::host::Host;
use crate::link::LinkState;

/// What a row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// The registry names the sidecar.
    Registered {
        /// The state of the main clone.
        clone: CloneState,
        /// The state of the link.
        link: LinkState,
    },
    /// A directory looks like a main clone, but the registry does not name it.
    NotRegistered,
}

/// One row of the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The name of the sidecar.
    pub name: String,
    /// What the row shows.
    pub kind: RowKind,
}

impl Row {
    /// True when the clone is ready and the link points at the main clone or at a
    /// worktree of it.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        false
    }
}

/// Reads the rows: the registered sidecars in registry order, then the directories
/// that look like main clones and are not registered, sorted by name.
#[must_use]
pub fn rows(host: &Host) -> Vec<Row> {
    let _ = host;
    Vec::new()
}

/// Renders the rows as a table. The text ends with a newline.
#[must_use]
pub fn render(rows: &[Row]) -> String {
    let _ = rows;
    String::new()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::link::ForeignLink;

    fn registered(name: &str, clone: CloneState, link: LinkState) -> Row {
        Row {
            name: name.to_string(),
            kind: RowKind::Registered { clone, link },
        }
    }

    #[test]
    fn render_pads_each_column_to_its_widest_cell() {
        let rows = [
            registered("a", CloneState::Ready, LinkState::MainClone),
            registered("日本語", CloneState::Missing, LinkState::Missing),
        ];
        assert_eq!(
            render(&rows),
            "NAME    CLONE    LINK\n\
             a       ready    at the main clone\n\
             日本語  missing  missing\n"
        );
    }

    #[test]
    fn render_shows_every_state_and_leaves_no_trailing_space() {
        let rows = [
            registered(
                "one",
                CloneState::Foreign {
                    reason: String::new(),
                },
                LinkState::Foreign(ForeignLink::File),
            ),
            registered(
                "two",
                CloneState::Incomplete,
                LinkState::Broken {
                    target: PathBuf::from("/x"),
                },
            ),
            registered(
                "three",
                CloneState::Ready,
                LinkState::Worktree {
                    branch: Some("b".to_string()),
                },
            ),
            registered(
                "four",
                CloneState::Ready,
                LinkState::Worktree { branch: None },
            ),
            Row {
                name: "five".to_string(),
                kind: RowKind::NotRegistered,
            },
        ];
        let text = render(&rows);
        assert!(text.ends_with('\n'));
        assert!(text.lines().all(|line| line == line.trim_end()), "{text}");
        for cell in [
            "not a clone of the sidecar",
            "foreign",
            "clone did not end",
            "broken",
            "at a worktree on branch b",
            "at a worktree on a detached HEAD",
            "not registered",
        ] {
            assert!(text.contains(cell), "{cell} is missing in {text}");
        }
    }

    #[test]
    fn is_ready_needs_a_ready_clone_and_a_good_link() {
        let ready = registered("a", CloneState::Ready, LinkState::MainClone);
        let detached = registered("a", CloneState::Ready, LinkState::Worktree { branch: None });
        let missing = registered("a", CloneState::Ready, LinkState::Missing);
        let incomplete = registered("a", CloneState::Incomplete, LinkState::MainClone);
        let stray = Row {
            name: "a".to_string(),
            kind: RowKind::NotRegistered,
        };
        assert!(ready.is_ready());
        assert!(detached.is_ready());
        assert!(!missing.is_ready());
        assert!(!incomplete.is_ready());
        assert!(!stray.is_ready());
    }
}
