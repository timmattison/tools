//! A ratatui backend that keeps the cells. `cctop::ui::draw` draws into it,
//! and `paint` turns the cells into the operations that `AppKit` runs.

use cctop::state::Rgb;
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use ratatui::style::Color;
use unicode_width::UnicodeWidthStr;

/// The 16 colors of the palette, in the order of the named colors of ratatui.
pub const DEFAULT_PALETTE: [Rgb; 16] = [Rgb {
    red: 0,
    green: 0,
    blue: 0,
}; 16];

/// The colors that the panel uses for the cells that name none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelColors {
    /// The color of a glyph that names none.
    pub foreground: Rgb,
    /// The color of the panel and of a cell that names none.
    pub background: Rgb,
    /// The 16 named colors.
    pub palette: [Rgb; 16],
}

impl Default for PanelColors {
    fn default() -> Self {
        Self {
            foreground: Rgb {
                red: 0,
                green: 0,
                blue: 0,
            },
            background: Rgb {
                red: 0,
                green: 0,
                blue: 0,
            },
            palette: DEFAULT_PALETTE,
        }
    }
}

/// One run of cells in one row that share a background color other than the
/// panel background.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundRun {
    /// The row of the run.
    pub row: u16,
    /// The column of the first cell.
    pub column: u16,
    /// The number of cells.
    pub cells: u16,
    /// The color of the run.
    pub color: Rgb,
}

/// One glyph to draw at the origin of its own column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphOp {
    /// The row of the glyph.
    pub row: u16,
    /// The column of the glyph.
    pub column: u16,
    /// The text of the glyph.
    pub glyph: String,
    /// The number of cells that the glyph covers, 1 or 2.
    pub cells: u16,
    /// The color of the glyph.
    pub color: Rgb,
    /// Tells if the glyph is bold.
    pub bold: bool,
}

/// The paint operations of a grid.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Paint {
    /// The runs of background, row by row.
    pub backgrounds: Vec<BackgroundRun>,
    /// The glyphs, row by row.
    pub glyphs: Vec<GlyphOp>,
}

/// The widest glyph that one cell pair holds.
const MAX_GLYPH_CELLS: usize = 2;

/// The number of cells that `symbol` covers: its display width, at least 1
/// and at most 2.
fn glyph_cells(symbol: &str) -> u16 {
    let width = UnicodeWidthStr::width(symbol).clamp(1, MAX_GLYPH_CELLS);
    u16::try_from(width).unwrap_or(1)
}

/// The color `color` stands for. A color that is not 24-bit gives `fallback`.
fn resolve(color: Color, fallback: Rgb) -> Rgb {
    match color {
        Color::Rgb(red, green, blue) => Rgb { red, green, blue },
        _ => fallback,
    }
}

/// A ratatui backend of a fixed size that keeps each cell.
pub struct CellGrid {
    buffer: Buffer,
    cursor: Position,
}

impl CellGrid {
    /// A grid of `columns` by `rows` blank cells.
    #[must_use]
    pub fn new(columns: u16, rows: u16) -> Self {
        Self {
            buffer: Buffer::empty(Rect::new(0, 0, columns, rows)),
            cursor: Position::ORIGIN,
        }
    }

    /// Change the size. The next `Terminal::draw` sees the new size through
    /// `Backend::size` and redraws all cells.
    pub fn resize(&mut self, columns: u16, rows: u16) {
        self.buffer = Buffer::empty(Rect::new(0, 0, columns, rows));
    }

    /// The cells.
    #[must_use]
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Reset each cell of `region` that lies inside the grid.
    fn reset_area(&mut self, region: Rect) {
        let region = region.intersection(self.buffer.area);
        for y in region.top()..region.bottom() {
            for x in region.left()..region.right() {
                self.buffer[(x, y)].reset();
            }
        }
    }

    /// The paint operations of the current cells.
    #[must_use]
    pub fn paint(&self, colors: &PanelColors) -> Paint {
        let area = self.buffer.area;
        let mut paint = Paint::default();
        for row in 0..area.height {
            // The columns that the wide glyph to their left covers.
            let mut covered_until = 0;
            for column in 0..area.width {
                if column < covered_until {
                    continue;
                }
                let symbol = self.buffer[(column, row)].symbol();
                if symbol.is_empty() || symbol == " " {
                    continue;
                }
                let cells = glyph_cells(symbol);
                covered_until = column.saturating_add(cells);
                paint.glyphs.push(GlyphOp {
                    row,
                    column,
                    glyph: symbol.to_string(),
                    cells,
                    color: resolve(self.buffer[(column, row)].fg, colors.foreground),
                    bold: false,
                });
            }
        }
        paint
    }
}

impl Backend for CellGrid {
    type Error = std::io::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (x, y, cell) in content {
            if let Some(target) = self.buffer.cell_mut((x, y)) {
                *target = cell.clone();
            }
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.cursor = position.into();
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.buffer.reset();
        Ok(())
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        let area = self.buffer.area;
        let cursor = self.cursor;
        let rest_of_line = Rect::new(cursor.x, cursor.y, area.width.saturating_sub(cursor.x), 1);
        match clear_type {
            ClearType::All => self.reset_area(area),
            ClearType::CurrentLine => self.reset_area(Rect::new(0, cursor.y, area.width, 1)),
            ClearType::UntilNewLine => self.reset_area(rest_of_line),
            ClearType::AfterCursor => {
                self.reset_area(rest_of_line);
                self.reset_area(Rect::new(
                    0,
                    cursor.y.saturating_add(1),
                    area.width,
                    area.height,
                ));
            }
            ClearType::BeforeCursor => {
                self.reset_area(Rect::new(0, 0, area.width, cursor.y));
                self.reset_area(Rect::new(0, cursor.y, cursor.x.saturating_add(1), 1));
            }
        }
        Ok(())
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(self.buffer.area.as_size())
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        Ok(WindowSize {
            columns_rows: self.buffer.area.as_size(),
            pixels: Size::default(),
        })
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctop::app::App;
    use cctop::state::{parse_state_file, EpochMillis, PaneState};
    use ratatui::Terminal;

    /// The time of the fixtures.
    const NOW: u64 = 1_794_824_300_000;

    /// A state file with the given pid and state.
    fn state_json(pid: u32, state: &str) -> String {
        format!(
            r#"{{"version":1,"pid":{pid},"sessionId":"s-{pid}","startedAt":1794823262488,"state":"{state}","stamp":1794824265058,"cwd":"/path/to/worktree","zellijSession":"tools","zellijPane":"310"}}"#
        )
    }

    /// An app that holds one session for each pair of pid and state, named.
    fn app_with(sessions: &[(u32, &str, &str)]) -> App {
        let mut app = App::new();
        for (pid, state, name) in sessions {
            let file = parse_state_file(&state_json(*pid, state)).unwrap();
            app.board_mut().load(file, Some((*name).to_string()));
        }
        app
    }

    /// Draw `app` into a fresh grid of `columns` by `rows`.
    fn draw_app(app: &App, columns: u16, rows: u16) -> Terminal<CellGrid> {
        let mut terminal = Terminal::new(CellGrid::new(columns, rows)).unwrap();
        terminal
            .draw(|frame| cctop::ui::draw(frame, app, EpochMillis::new(NOW)))
            .unwrap();
        terminal
    }

    /// The symbols of row `row`, joined.
    fn row_text(grid: &CellGrid, row: u16) -> String {
        let area = grid.buffer().area;
        (0..area.width)
            .map(|x| grid.buffer()[(x, row)].symbol().to_string())
            .collect()
    }

    #[test]
    fn the_backend_keeps_what_draw_drew() {
        let app = app_with(&[
            (1, "needs-you", "日本語"),
            (2, "working", "café"),
            (3, "idle", "🎉"),
        ]);
        let terminal = draw_app(&app, 100, 30);
        let title = row_text(terminal.backend(), 0);
        assert!(title.contains("cctop"), "{title}");
        assert!(title.contains("3 sessions"), "{title}");
    }

    #[test]
    fn a_wide_glyph_covers_two_cells_and_keeps_the_next_glyph_in_place() {
        let app = app_with(&[(1, "needs-you", "日本語")]);
        let terminal = draw_app(&app, 100, 30);
        let paint = terminal.backend().paint(&PanelColors::default());
        let title: Vec<&GlyphOp> = paint.glyphs.iter().filter(|op| op.row == 0).collect();
        let wide = title
            .iter()
            .find(|op| op.glyph == "❗")
            .expect("the marker");
        assert_eq!(wide.cells, 2);
        // The cell to the right of the marker is covered, the one after it is a space.
        assert!(title.iter().all(|op| op.column != wide.column + 1));
        assert!(title.iter().all(|op| op.column != wide.column + 2));
        let next = title
            .iter()
            .find(|op| op.column > wide.column)
            .expect("a glyph after the marker");
        assert_eq!(next.column, wide.column + 3);
        assert_eq!(next.cells, 1);
    }

    #[test]
    fn the_state_blocks_have_the_color_of_the_state() {
        let app = app_with(&[(1, "needs-you", "日本語")]);
        let terminal = draw_app(&app, 100, 30);
        let paint = terminal.backend().paint(&PanelColors::default());
        let blocks: Vec<&GlyphOp> = paint.glyphs.iter().filter(|op| op.glyph == "█").collect();
        assert_eq!(blocks.len(), 2);
        let expected = PaneState::NeedsYou.color();
        assert!(blocks.iter().all(|op| op.color == expected));
    }

    /// Colors whose palette entries are all different, so a wrong index shows.
    fn distinct_colors() -> PanelColors {
        let mut palette = [Rgb {
            red: 0,
            green: 0,
            blue: 0,
        }; 16];
        for (index, entry) in palette.iter_mut().enumerate() {
            let level = u8::try_from(index).unwrap() * 10 + 1;
            *entry = Rgb {
                red: level,
                green: level + 1,
                blue: level + 2,
            };
        }
        PanelColors {
            foreground: Rgb {
                red: 200,
                green: 201,
                blue: 202,
            },
            background: Rgb {
                red: 30,
                green: 31,
                blue: 32,
            },
            palette,
        }
    }

    /// A grid of `width` by 2 cells where the cells of row 0 are `cells`.
    fn grid_of(width: u16, cells: &[Cell]) -> CellGrid {
        let mut grid = CellGrid::new(width, 2);
        let content: Vec<(u16, u16, &Cell)> = cells
            .iter()
            .enumerate()
            .map(|(x, cell)| (u16::try_from(x).unwrap(), 0, cell))
            .collect();
        grid.draw(content.into_iter()).unwrap();
        grid
    }

    /// The foreground colors that `paint` gives to cells with the foregrounds `colors`.
    fn glyph_colors(panel: &PanelColors, foregrounds: &[Color]) -> Vec<Rgb> {
        let cells: Vec<Cell> = foregrounds
            .iter()
            .map(|color| {
                let mut cell = Cell::new("x");
                cell.set_fg(*color);
                cell
            })
            .collect();
        let width = u16::try_from(cells.len()).unwrap();
        grid_of(width, &cells)
            .paint(panel)
            .glyphs
            .iter()
            .map(|op| op.color)
            .collect()
    }

    #[test]
    fn the_reset_color_maps_to_the_panel_colors() {
        let panel = distinct_colors();
        assert_eq!(
            glyph_colors(&panel, &[Color::Reset]),
            vec![panel.foreground]
        );
    }

    #[test]
    fn a_24_bit_color_stays_as_it_is() {
        let panel = distinct_colors();
        assert_eq!(
            glyph_colors(&panel, &[Color::Rgb(1, 2, 3)]),
            vec![Rgb {
                red: 1,
                green: 2,
                blue: 3
            }]
        );
    }

    #[test]
    fn the_named_colors_map_to_the_palette_in_order() {
        let panel = distinct_colors();
        let named = [
            Color::Black,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::Gray,
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::White,
        ];
        assert_eq!(glyph_colors(&panel, &named), panel.palette.to_vec());
    }

    #[test]
    fn the_indexed_colors_map_to_the_palette_the_cube_and_the_gray_ramp() {
        let panel = distinct_colors();
        let colors = glyph_colors(
            &panel,
            &[
                Color::Indexed(0),
                Color::Indexed(15),
                Color::Indexed(16),
                Color::Indexed(21),
                Color::Indexed(196),
                Color::Indexed(231),
                Color::Indexed(59),
                Color::Indexed(232),
                Color::Indexed(255),
            ],
        );
        let rgb = |red, green, blue| Rgb { red, green, blue };
        assert_eq!(
            colors,
            vec![
                panel.palette[0],
                panel.palette[15],
                rgb(0, 0, 0),
                rgb(0, 0, 255),
                rgb(255, 0, 0),
                rgb(255, 255, 255),
                rgb(95, 95, 95),
                rgb(8, 8, 8),
                rgb(238, 238, 238),
            ]
        );
    }

    #[test]
    fn the_default_colors_are_the_dark_panel() {
        let panel = PanelColors::default();
        let rgb = |value: u32| Rgb {
            red: u8::try_from(value >> 16).unwrap(),
            green: u8::try_from((value >> 8) & 0xff).unwrap(),
            blue: u8::try_from(value & 0xff).unwrap(),
        };
        assert_eq!(panel.foreground, rgb(0xcccccc));
        assert_eq!(panel.background, rgb(0x1e1e1e));
        assert_eq!(panel.palette[1], rgb(0xcd3131));
        assert_eq!(panel.palette[15], rgb(0xffffff));
    }
}
