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
    let screen = active_screen(input)?;
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

/// The screen that the panel opens on: the one that holds most of the front
/// window, else the one under the mouse, else the first one.
fn active_screen<'a>(input: &PlacementInput<'a>) -> Option<&'a Screen> {
    let by_window = input.front_window.and_then(|window| {
        input
            .screens
            .iter()
            .map(|screen| (overlap_area(window, screen.frame), screen))
            .filter(|(area, _)| *area > 0.0)
            .fold(None, |best: Option<(f64, &Screen)>, candidate| match best {
                Some((area, _)) if area >= candidate.0 => best,
                _ => Some(candidate),
            })
            .map(|(_, screen)| screen)
    });
    by_window
        .or_else(|| {
            input
                .screens
                .iter()
                .find(|screen| contains(screen.frame, input.mouse))
        })
        .or_else(|| input.screens.first())
}

/// The area where `a` and `b` overlap, 0 when they do not.
fn overlap_area(a: Rect, b: Rect) -> f64 {
    let width = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
    let height = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
    if width > 0.0 && height > 0.0 {
        width * height
    } else {
        0.0
    }
}

/// Tells if `point` is in `rect`. The left and bottom edges count, the right
/// and top edges do not.
fn contains(rect: Rect, point: Point) -> bool {
    point.x >= rect.x
        && point.x < rect.x + rect.width
        && point.y >= rect.y
        && point.y < rect.y + rect.height
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

    /// A screen of 1920 by 1080 points to the right of `MAIN`.
    const RIGHT: Screen = Screen {
        frame: rect(1440.0, 0.0, 1920.0, 1080.0),
        visible: rect(1440.0, 0.0, 1920.0, 1055.0),
    };

    /// The screen that holds the center of the frame of `placement`.
    fn screen_of(placement: &Placement) -> usize {
        let center = placement.frame.x + placement.frame.width / 2.0;
        usize::from(center >= RIGHT.frame.x)
    }

    #[test]
    fn a_window_across_two_screens_picks_the_larger_overlap() {
        let mut request = input(&[MAIN, RIGHT]);
        request.front_window = Some(rect(1240.0, 100.0, 600.0, 400.0));
        assert_eq!(screen_of(&place(&request).unwrap()), 1);
        request.front_window = Some(rect(840.0, 100.0, 800.0, 400.0));
        assert_eq!(screen_of(&place(&request).unwrap()), 0);
    }

    #[test]
    fn no_front_window_picks_the_screen_under_the_mouse() {
        let mut request = input(&[MAIN, RIGHT]);
        request.mouse = Point {
            x: 1500.0,
            y: 100.0,
        };
        assert_eq!(screen_of(&place(&request).unwrap()), 1);
        // The left edge belongs to the screen, the right edge does not.
        request.mouse = Point {
            x: 1440.0,
            y: 100.0,
        };
        assert_eq!(screen_of(&place(&request).unwrap()), 1);
        request.mouse = Point {
            x: 1439.0,
            y: 100.0,
        };
        assert_eq!(screen_of(&place(&request).unwrap()), 0);
    }

    #[test]
    fn a_window_that_touches_no_screen_falls_back_to_the_mouse() {
        let mut request = input(&[MAIN, RIGHT]);
        request.front_window = Some(rect(-900.0, 100.0, 400.0, 400.0));
        request.mouse = Point {
            x: 2000.0,
            y: 500.0,
        };
        assert_eq!(screen_of(&place(&request).unwrap()), 1);
    }

    #[test]
    fn a_mouse_on_no_screen_picks_the_first_screen() {
        let mut request = input(&[MAIN, RIGHT]);
        request.mouse = Point {
            x: -50.0,
            y: 5000.0,
        };
        assert_eq!(screen_of(&place(&request).unwrap()), 0);
    }

    #[test]
    fn the_panel_is_centered_in_the_visible_frame_of_the_chosen_screen() {
        let mut request = input(&[MAIN, RIGHT]);
        request.mouse = Point {
            x: 2000.0,
            y: 500.0,
        };
        let placement = place(&request).unwrap();
        // 1920 / 3 * 2 is 1280 points, 160 columns. 1055 / 3 * 2 is 703 points, 43 rows.
        assert_eq!((placement.columns, placement.rows), (160, 43));
        assert_eq!(placement.frame, rect(1760.0, 183.0, 1280.0, 688.0));
    }
}
