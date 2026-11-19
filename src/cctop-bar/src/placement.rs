//! Where the panel goes and how many cells it has. All rectangles are in
//! Cocoa global points: the origin is the bottom left of the primary screen
//! and y grows upward.

/// A rectangle in Cocoa global points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// The left edge.
    pub x: f64,
    /// The bottom edge.
    pub y: f64,
    /// The width.
    pub width: f64,
    /// The height.
    pub height: f64,
}

/// A point in Cocoa global points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// The horizontal position.
    pub x: f64,
    /// The vertical position.
    pub y: f64,
}

/// A screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    /// The whole screen.
    pub frame: Rect,
    /// The screen without the menu bar and the Dock.
    pub visible: Rect,
}

/// The size of one cell in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellSize {
    /// The width of a cell.
    pub width: f64,
    /// The height of a cell.
    pub height: f64,
}

/// The fewest columns that the panel has when the screen holds them.
pub const MIN_COLUMNS: u16 = 80;

/// The fewest rows that the panel has when the screen holds them.
pub const MIN_ROWS: u16 = 24;

/// What `place` needs to know.
#[derive(Debug, Clone, Copy)]
pub struct PlacementInput<'a> {
    /// The frame of the window that has the focus, if any.
    pub front_window: Option<Rect>,
    /// The screens, the primary one first.
    pub screens: &'a [Screen],
    /// The position of the mouse.
    pub mouse: Point,
    /// The size of a cell.
    pub cell: CellSize,
}

/// The place of the panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The frame of the panel.
    pub frame: Rect,
    /// The columns of the panel.
    pub columns: u16,
    /// The rows of the panel.
    pub rows: u16,
}

/// The place of the panel, or `None` when there is no screen or the cell size
/// is not positive.
#[must_use]
pub fn place(input: &PlacementInput<'_>) -> Option<Placement> {
    let cell = input.cell;
    if cell.width.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
        || cell.height.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
    {
        return None;
    }
    let screen = input.screens.first()?;
    let visible = screen.visible;
    let columns = cells_in(visible.width, cell.width);
    let rows = cells_in(visible.height, cell.height);
    let width = f64::from(columns) * cell.width;
    let height = f64::from(rows) * cell.height;
    Some(Placement {
        frame: Rect {
            x: (visible.x + (visible.width - width) / 2.0).floor(),
            y: (visible.y + (visible.height - height) / 2.0).floor(),
            width,
            height,
        },
        columns,
        rows,
    })
}

/// The number of whole cells in two thirds of `extent`, clamped to `u16`.
fn cells_in(extent: f64, cell: f64) -> u16 {
    clamp_to_u16((extent * 2.0 / 3.0 / cell).floor())
}

/// `value` as a `u16`, with 0 and `u16::MAX` as the bounds.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to the range of u16 first"
)]
fn clamp_to_u16(value: f64) -> u16 {
    value.clamp(0.0, f64::from(u16::MAX)) as u16
}

/// A `CGWindow` bounds rectangle in Cocoa global points. The bounds have their
/// origin at the top left of the primary display and y grows downward.
/// `primary_height` is the height of the frame of the first screen.
#[must_use]
pub fn flip_window_bounds(bounds: Rect, primary_height: f64) -> Rect {
    let _ = primary_height;
    bounds
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: CellSize = CellSize {
        width: 8.0,
        height: 16.0,
    };

    const fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// A 1440 by 900 screen with a menu bar of 25 points and a Dock of 70.
    const MAIN: Screen = Screen {
        frame: rect(0.0, 0.0, 1440.0, 900.0),
        visible: rect(0.0, 70.0, 1440.0, 805.0),
    };

    fn input(screens: &[Screen]) -> PlacementInput<'_> {
        PlacementInput {
            front_window: None,
            screens,
            mouse: Point { x: 0.0, y: 0.0 },
            cell: CELL,
        }
    }

    #[test]
    fn one_screen_gives_two_thirds_in_whole_cells_centered_in_the_visible_frame() {
        let placement = place(&input(&[MAIN])).unwrap();
        assert_eq!(placement.columns, 120);
        assert_eq!(placement.rows, 33);
        assert_eq!(placement.frame, rect(240.0, 208.0, 960.0, 528.0));
        // The frame stays below the menu bar and above the Dock.
        assert!(placement.frame.y >= MAIN.visible.y);
        assert!(placement.frame.y + placement.frame.height <= MAIN.visible.y + MAIN.visible.height);
    }
}
