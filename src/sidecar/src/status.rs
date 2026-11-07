//! The state of each sidecar, as a table.
//!
//! `sidecar status` reads the main clone and the link of each sidecar. It makes no
//! network call and writes nothing.

use crate::clone::{inspect_clone, CloneState};
use crate::host::{Host, WORKTREES_SUFFIX};
use crate::link::{inspect_link, LinkState};
use crate::name::SidecarName;
use crate::remote::DATA_INFIX;
use unicode_width::UnicodeWidthStr;

/// The text of the two state columns of a row that the registry does not name.
const NOT_REGISTERED: &str = "not registered";
/// The text of the link column of a row that the registry does not name.
const NO_LINK: &str = "-";
/// The header of the table.
const HEADER: [&str; 3] = ["NAME", "CLONE", "LINK"];
/// The space between two columns.
const GAP: &str = "  ";

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
        matches!(
            &self.kind,
            RowKind::Registered {
                clone: CloneState::Ready,
                link: LinkState::MainClone | LinkState::Worktree { .. },
            }
        )
    }
}

/// Reads the rows: the registered sidecars in registry order, then the directories
/// that look like main clones and are not registered, sorted by name.
#[must_use]
pub fn rows(host: &Host) -> Vec<Row> {
    let mut table: Vec<Row> = host
        .registry()
        .sidecars()
        .iter()
        .map(|name| registered_row(host, name))
        .collect();
    table.extend(unregistered_names(host).into_iter().map(|name| Row {
        name,
        kind: RowKind::NotRegistered,
    }));
    table
}

/// Reads the main clone and the link of one registered sidecar. Nothing here uses the
/// network.
fn registered_row(host: &Host, name: &SidecarName) -> Row {
    let clone = match host.sidecar_url(name) {
        Ok(url) => inspect_clone(host.git(), &url, &host.main_clone(name)),
        Err(error) => CloneState::Foreign {
            reason: error.to_string(),
        },
    };
    let link = inspect_link(host.git(), &host.link(name), &host.main_clone(name));
    Row {
        name: name.to_string(),
        kind: RowKind::Registered { clone, link },
    }
}

/// The names of the directories next to the main checkout that look like the main
/// clone of a sidecar that the registry does not name. The list is sorted. A
/// directory of sidecar worktrees is not in the list.
fn unregistered_names(host: &Host) -> Vec<String> {
    let prefix = format!("{}{DATA_INFIX}", host.name());
    let Ok(entries) = std::fs::read_dir(host.parent()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let rest = file_name.strip_prefix(&prefix)?.to_string();
            let name = SidecarName::parse(&rest).ok()?;
            let is_unregistered = !host.registry().contains(&name);
            (is_unregistered && !is_worktrees_directory(host, &prefix, &rest)).then_some(rest)
        })
        .collect();
    names.sort();
    names
}

/// True when `rest` ends in the worktrees suffix and the part before it names a
/// registered sidecar or a directory that is there.
fn is_worktrees_directory(host: &Host, prefix: &str, rest: &str) -> bool {
    let Some(base) = rest.strip_suffix(WORKTREES_SUFFIX) else {
        return false;
    };
    SidecarName::parse(base).is_ok_and(|name| host.registry().contains(&name))
        || host.parent().join(format!("{prefix}{base}")).exists()
}

/// The three cells of a row.
fn cells(row: &Row) -> [String; 3] {
    let (clone, link) = match &row.kind {
        RowKind::NotRegistered => (NOT_REGISTERED.to_string(), NO_LINK.to_string()),
        RowKind::Registered { clone, link } => (clone_text(clone), link_text(link)),
    };
    [row.name.clone(), clone, link]
}

/// The text of the clone column for `state`.
fn clone_text(state: &CloneState) -> String {
    match state {
        CloneState::Missing => "missing",
        CloneState::Foreign { .. } => "not a clone of the sidecar",
        CloneState::Incomplete => "clone did not end",
        CloneState::Ready => "ready",
    }
    .to_string()
}

/// The text of the link column for `state`.
fn link_text(state: &LinkState) -> String {
    match state {
        LinkState::Missing => "missing".to_string(),
        LinkState::MainClone => "at the main clone".to_string(),
        LinkState::Worktree {
            branch: Some(branch),
        } => {
            format!("at a worktree on branch {branch}")
        }
        LinkState::Worktree { branch: None } => "at a worktree on a detached HEAD".to_string(),
        LinkState::Broken { .. } => "broken".to_string(),
        LinkState::Foreign(_) => "foreign".to_string(),
    }
}

/// Renders the rows as a table. The text ends with a newline. No line ends in a space.
///
/// Each column pads to its widest cell. The width is the display width in terminal
/// columns, not the count of characters, so a wide name keeps the columns in line.
#[must_use]
pub fn render(rows: &[Row]) -> String {
    let mut lines: Vec<[String; 3]> = vec![HEADER.map(str::to_string)];
    lines.extend(rows.iter().map(cells));
    let widths: Vec<usize> = (0..HEADER.len())
        .map(|column| {
            lines
                .iter()
                .map(|line| line[column].width())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut text = String::new();
    for line in &lines {
        let padded: Vec<String> = line
            .iter()
            .zip(&widths)
            .map(|(cell, width)| {
                format!("{cell}{}", " ".repeat(width.saturating_sub(cell.width())))
            })
            .collect();
        text.push_str(padded.join(GAP).trim_end());
        text.push('\n');
    }
    text
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
    fn render_pads_emoji_and_accented_names_by_display_width() {
        let rows = [
            registered("🎉", CloneState::Ready, LinkState::MainClone),
            registered("café", CloneState::Missing, LinkState::Missing),
        ];
        assert_eq!(
            render(&rows),
            "NAME  CLONE    LINK\n\
             🎉    ready    at the main clone\n\
             café  missing  missing\n"
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
