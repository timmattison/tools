//! The mini-map of a board: one small tile for each session, in the order of
//! the rows of the screen, thus the sessions that need the person come first.
//! A tile is too small to show the state color and the word `frozen` together,
//! thus a frozen tile is always blue.

use crate::board::Board;
use crate::state::{PaneState, Rgb};

/// The number of tiles that the status item shows at most.
pub const TILE_LIMIT: usize = 12;

/// How a tile is painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileStyle {
    /// The tile is a solid square.
    Filled,
    /// The tile is an outline with an empty inside.
    Outline,
}

/// One tile: one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    /// The state of the session.
    pub state: PaneState,
    /// Tell if the session has a handled mark.
    pub handled: bool,
    /// Tell if the process of the session is stopped.
    pub frozen: bool,
}

impl Tile {
    /// The color of the tile: the frozen color when the session is frozen,
    /// else the color of its state.
    #[must_use]
    pub fn color(self) -> Rgb {
        Rgb {
            red: 0,
            green: 0,
            blue: 0,
        }
    }

    /// How the tile is painted. A frozen tile is always filled, also when it
    /// is handled. Else a handled tile is an outline, and the rest are filled.
    #[must_use]
    pub fn style(self) -> TileStyle {
        TileStyle::Filled
    }
}

/// The mini-map of one board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Minimap {
    /// The tiles in the order of `Board::rows`, at most `limit`.
    pub tiles: Vec<Tile>,
    /// The number of sessions past the limit.
    pub overflow: usize,
    /// The number of sessions that need the person, see
    /// [`Board::attention_count`].
    pub attention: usize,
}

impl Minimap {
    /// The mini-map of the board with at most `limit` tiles.
    #[must_use]
    pub fn of(board: &Board, _limit: usize) -> Self {
        let tiles = board
            .rows()
            .into_iter()
            .map(|session| Tile {
                state: session.state(),
                handled: session.is_handled(),
                frozen: session.is_frozen(),
            })
            .collect();
        Self {
            tiles,
            overflow: 0,
            attention: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::liveness::RunStatus;
    use crate::state::{EpochMillis, StateFile, FROZEN_COLOR};
    use proctree::Pid;

    /// A state file in the directory `directory`.
    fn file(pid: u32, state: PaneState, directory: &str) -> StateFile {
        StateFile {
            pid: Pid::new(pid),
            session_id: format!("session-{pid}"),
            started_at: EpochMillis::new(100),
            state,
            stamp: EpochMillis::new(500),
            cwd: std::path::PathBuf::from(format!("/path/to/{directory}")),
            zellij_session: None,
            zellij_pane: None,
        }
    }

    fn mixed_board() -> Board {
        let mut board = Board::new();
        board.load(file(1, PaneState::Idle, "café"), None);
        board.load(file(2, PaneState::NeedsYou, "日本語"), None);
        board.load(file(3, PaneState::Working, "🎉"), None);
        board.load(file(4, PaneState::NeedsYou, "four"), None);
        board.load(file(5, PaneState::Idle, "five"), None);
        board.toggle_handled(Pid::new(4));
        board.set_run_status(Pid::new(5), RunStatus::Stopped, EpochMillis::new(1_000));
        board
    }

    #[test]
    fn tiles_follow_the_rows_of_the_board() {
        let board = mixed_board();
        let map = Minimap::of(&board, TILE_LIMIT);
        let expected: Vec<Tile> = board
            .rows()
            .iter()
            .map(|session| Tile {
                state: session.state(),
                handled: session.is_handled(),
                frozen: session.is_frozen(),
            })
            .collect();
        assert_eq!(map.tiles, expected);
        assert_eq!(map.tiles.len(), 5);
        assert_eq!(map.tiles[0].state, PaneState::NeedsYou);
        assert!(!map.tiles[0].handled);
        assert!(map.tiles[1].handled);
        assert!(map.tiles.iter().any(|tile| tile.frozen));
        assert!(map
            .tiles
            .iter()
            .take(2)
            .all(|tile| tile.state == PaneState::NeedsYou));
    }

    fn tile(state: PaneState, handled: bool, frozen: bool) -> Tile {
        Tile {
            state,
            handled,
            frozen,
        }
    }

    #[test]
    fn a_plain_tile_is_filled_in_the_color_of_its_state() {
        for state in PaneState::ALL {
            let plain = tile(state, false, false);
            assert_eq!(plain.color(), state.color(), "{state:?}");
            assert_eq!(plain.style(), TileStyle::Filled, "{state:?}");
        }
    }

    #[test]
    fn a_handled_tile_is_an_outline_in_the_color_of_its_state() {
        for state in PaneState::ALL {
            let handled = tile(state, true, false);
            assert_eq!(handled.color(), state.color(), "{state:?}");
            assert_eq!(handled.style(), TileStyle::Outline, "{state:?}");
        }
    }

    #[test]
    fn a_frozen_tile_is_always_filled_in_the_frozen_color() {
        for state in PaneState::ALL {
            for handled in [false, true] {
                let frozen = tile(state, handled, true);
                assert_eq!(frozen.color(), FROZEN_COLOR, "{state:?} {handled}");
                assert_eq!(frozen.style(), TileStyle::Filled, "{state:?} {handled}");
            }
        }
    }
}
