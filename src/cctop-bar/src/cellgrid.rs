//! A ratatui backend that keeps the cells. `cctop::ui::draw` draws into it,
//! and `paint` turns the cells into the operations that `AppKit` runs.

use cctop::state::Rgb;
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

/// The color of a 24-bit value `0xRRGGBB`.
const fn hex(value: u32) -> Rgb {
    let [_, red, green, blue] = value.to_be_bytes();
    Rgb { red, green, blue }
}

/// The 16 colors of the palette, in the order of the named colors of ratatui.
pub const DEFAULT_PALETTE: [Rgb; 16] = [
    hex(0x00_0000),
    hex(0xcd_3131),
    hex(0x0d_bc79),
    hex(0xe5_e510),
    hex(0x24_72c8),
    hex(0xbc_3fbc),
    hex(0x11_a8cd),
    hex(0xe5_e5e5),
    hex(0x66_6666),
    hex(0xf1_4c4c),
    hex(0x23_d18b),
    hex(0xf5_f543),
    hex(0x3b_8eea),
    hex(0xd6_70d6),
    hex(0x29_b8db),
    hex(0xff_ffff),
];

/// The foreground of the panel.
pub const DEFAULT_FOREGROUND: Rgb = hex(0xcc_cccc);

/// The background of the panel.
pub const DEFAULT_BACKGROUND: Rgb = hex(0x1e_1e1e);

/// The levels of one channel of the 6 by 6 by 6 color cube of xterm.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The first index of the color cube.
const CUBE_START: u8 = 16;

/// The first index of the gray ramp.
const GRAY_START: u8 = 232;

/// The level of the first gray of the ramp.
const GRAY_BASE: u8 = 8;

/// The step between two grays of the ramp.
const GRAY_STEP: u8 = 10;

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
            foreground: DEFAULT_FOREGROUND,
            background: DEFAULT_BACKGROUND,
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

/// The color `color` stands for. `Reset` gives `fallback`.
fn resolve(color: Color, fallback: Rgb, palette: &[Rgb; 16]) -> Rgb {
    match color {
        Color::Reset => fallback,
        Color::Rgb(red, green, blue) => Rgb { red, green, blue },
        Color::Indexed(index) => indexed(index, palette),
        Color::Black => palette[0],
        Color::Red => palette[1],
        Color::Green => palette[2],
        Color::Yellow => palette[3],
        Color::Blue => palette[4],
        Color::Magenta => palette[5],
        Color::Cyan => palette[6],
        Color::Gray => palette[7],
        Color::DarkGray => palette[8],
        Color::LightRed => palette[9],
        Color::LightGreen => palette[10],
        Color::LightYellow => palette[11],
        Color::LightBlue => palette[12],
        Color::LightMagenta => palette[13],
        Color::LightCyan => palette[14],
        Color::White => palette[15],
    }
}

/// The color of the xterm index `index`.
fn indexed(index: u8, palette: &[Rgb; 16]) -> Rgb {
    if index < CUBE_START {
        palette[usize::from(index)]
    } else if index < GRAY_START {
        let offset = usize::from(index - CUBE_START);
        Rgb {
            red: CUBE_LEVELS[offset / 36],
            green: CUBE_LEVELS[offset / 6 % 6],
            blue: CUBE_LEVELS[offset % 6],
        }
    } else {
        let level = GRAY_BASE + GRAY_STEP * (index - GRAY_START);
        Rgb {
            red: level,
            green: level,
            blue: level,
        }
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

    /// The foreground and the background of `cell`, with the reverse applied.
    fn cell_colors(cell: &Cell, colors: &PanelColors) -> (Rgb, Rgb) {
        let foreground = resolve(cell.fg, colors.foreground, &colors.palette);
        let background = resolve(cell.bg, colors.background, &colors.palette);
        if cell.modifier.contains(Modifier::REVERSED) {
            (background, foreground)
        } else {
            (foreground, background)
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
            let mut run: Option<BackgroundRun> = None;
            for column in 0..area.width {
                let cell = &self.buffer[(column, row)];
                let (foreground, background) = Self::cell_colors(cell, colors);
                match run.as_mut() {
                    Some(open) if open.color == background => open.cells += 1,
                    _ => {
                        paint.backgrounds.extend(run.take());
                        if background != colors.background {
                            run = Some(BackgroundRun {
                                row,
                                column,
                                cells: 1,
                                color: background,
                            });
                        }
                    }
                }
                let symbol = cell.symbol();
                if column < covered_until || symbol.is_empty() || symbol == " " {
                    continue;
                }
                let cells = glyph_cells(symbol);
                covered_until = column.saturating_add(cells);
                paint.glyphs.push(GlyphOp {
                    row,
                    column,
                    glyph: symbol.to_string(),
                    cells,
                    color: foreground,
                    bold: cell.modifier.contains(Modifier::BOLD),
                });
            }
            paint.backgrounds.extend(run);
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
    use ratatui::style::Modifier;
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
        // The first row is selected, thus the highlight reverses it. The second row shows the plain colors.
        let app = app_with(&[(1, "needs-you", "日本語"), (2, "needs-you", "café")]);
        let terminal = draw_app(&app, 100, 30);
        let paint = terminal.backend().paint(&PanelColors::default());
        let expected = PaneState::NeedsYou.color();
        let blocks: Vec<&GlyphOp> = paint.glyphs.iter().filter(|op| op.glyph == "█").collect();
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks.iter().filter(|op| op.color == expected).count(), 2);
        // The reversed row has the color as its background, and the block takes the panel background.
        assert!(paint.backgrounds.iter().any(|run| run.color == expected));
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

    /// A cell with the symbol `symbol` and the colors `fg` and `bg`.
    fn cell(symbol: &'static str, fg: Color, bg: Color, modifier: Modifier) -> Cell {
        let mut cell = Cell::new(symbol);
        cell.set_fg(fg).set_bg(bg).modifier = modifier;
        cell
    }

    #[test]
    fn a_reversed_cell_swaps_its_colors() {
        let panel = distinct_colors();
        let paint = grid_of(
            1,
            &[cell(
                "x",
                Color::Rgb(10, 20, 30),
                Color::Reset,
                Modifier::REVERSED,
            )],
        )
        .paint(&panel);
        assert_eq!(
            paint.backgrounds,
            vec![BackgroundRun {
                row: 0,
                column: 0,
                cells: 1,
                color: Rgb {
                    red: 10,
                    green: 20,
                    blue: 30
                },
            }]
        );
        assert_eq!(paint.glyphs[0].color, panel.background);
    }

    #[test]
    fn the_bold_modifier_makes_a_bold_glyph_and_other_modifiers_do_nothing() {
        let panel = distinct_colors();
        let paint = grid_of(
            2,
            &[
                cell("a", Color::Reset, Color::Reset, Modifier::BOLD),
                cell(
                    "b",
                    Color::Reset,
                    Color::Reset,
                    Modifier::ITALIC | Modifier::DIM,
                ),
            ],
        )
        .paint(&panel);
        assert_eq!(
            paint.glyphs.iter().map(|op| op.bold).collect::<Vec<_>>(),
            vec![true, false]
        );
        assert!(paint.backgrounds.is_empty());
    }

    #[test]
    fn the_header_of_the_table_is_bold() {
        let app = app_with(&[(1, "needs-you", "日本語")]);
        let terminal = draw_app(&app, 100, 30);
        let paint = terminal.backend().paint(&PanelColors::default());
        let header: Vec<&GlyphOp> = paint
            .glyphs
            .iter()
            .filter(|op| op.row == 1 && op.glyph != "│")
            .collect();
        assert!(!header.is_empty());
        assert!(header.iter().all(|op| op.bold));
    }

    #[test]
    fn neighbours_of_one_background_merge_and_the_panel_background_is_left_out() {
        let panel = distinct_colors();
        let red = Color::Rgb(200, 0, 0);
        let blue = Color::Rgb(0, 0, 200);
        let plain = |fg| cell(" ", fg, Color::Reset, Modifier::empty());
        let on = |bg| cell("x", Color::Reset, bg, Modifier::empty());
        let paint = grid_of(
            7,
            &[
                on(red),
                on(red),
                on(Color::Rgb(30, 31, 32)),
                plain(Color::Reset),
                on(red),
                on(blue),
                on(blue),
            ],
        )
        .paint(&panel);
        let run = |column, cells, color| BackgroundRun {
            row: 0,
            column,
            cells,
            color,
        };
        let rgb = |red, green, blue| Rgb { red, green, blue };
        assert_eq!(
            paint.backgrounds,
            vec![
                run(0, 2, rgb(200, 0, 0)),
                run(4, 1, rgb(200, 0, 0)),
                run(5, 2, rgb(0, 0, 200)),
            ]
        );
    }

    #[test]
    fn a_cell_under_a_wide_glyph_joins_the_background_run() {
        let panel = distinct_colors();
        let red = Color::Rgb(200, 0, 0);
        let paint = grid_of(
            2,
            &[
                cell("❗", Color::Reset, red, Modifier::empty()),
                cell(" ", Color::Reset, red, Modifier::empty()),
            ],
        )
        .paint(&panel);
        assert_eq!(paint.backgrounds.len(), 1);
        assert_eq!(paint.backgrounds[0].cells, 2);
        assert_eq!(paint.glyphs.len(), 1);
    }
}
