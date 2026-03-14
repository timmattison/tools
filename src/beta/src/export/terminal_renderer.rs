use anyhow::Result;
use vte::{Parser, Perform};

#[derive(Debug, Clone)]
pub struct TerminalTheme {
    pub background: (u8, u8, u8),
    pub foreground: (u8, u8, u8),
    pub black: (u8, u8, u8),
    pub red: (u8, u8, u8),
    pub green: (u8, u8, u8),
    pub yellow: (u8, u8, u8),
    pub blue: (u8, u8, u8),
    pub magenta: (u8, u8, u8),
    pub cyan: (u8, u8, u8),
    pub white: (u8, u8, u8),
    pub bright_black: (u8, u8, u8),
    pub bright_red: (u8, u8, u8),
    pub bright_green: (u8, u8, u8),
    pub bright_yellow: (u8, u8, u8),
    pub bright_blue: (u8, u8, u8),
    pub bright_magenta: (u8, u8, u8),
    pub bright_cyan: (u8, u8, u8),
    pub bright_white: (u8, u8, u8),
}

impl TerminalTheme {
    pub fn auto() -> Self {
        Self::black()
    }
    
    pub fn black() -> Self {
        Self {
            background: (0, 0, 0),
            foreground: (255, 255, 255),
            black: (0, 0, 0),
            red: (255, 85, 85),
            green: (85, 255, 85),
            yellow: (255, 255, 85),
            blue: (85, 85, 255),
            magenta: (255, 85, 255),
            cyan: (85, 255, 255),
            white: (255, 255, 255),
            bright_black: (85, 85, 85),
            bright_red: (255, 85, 85),
            bright_green: (85, 255, 85),
            bright_yellow: (255, 255, 85),
            bright_blue: (85, 85, 255),
            bright_magenta: (255, 85, 255),
            bright_cyan: (85, 255, 255),
            bright_white: (255, 255, 255),
        }
    }
    
    pub fn dracula() -> Self {
        Self {
            background: (40, 42, 54),
            foreground: (248, 248, 242),
            black: (40, 42, 54),
            red: (255, 85, 85),
            green: (80, 250, 123),
            yellow: (241, 250, 140),
            blue: (98, 114, 164),
            magenta: (255, 121, 198),
            cyan: (139, 233, 253),
            white: (248, 248, 242),
            bright_black: (98, 114, 164),
            bright_red: (255, 85, 85),
            bright_green: (80, 250, 123),
            bright_yellow: (241, 250, 140),
            bright_blue: (98, 114, 164),
            bright_magenta: (255, 121, 198),
            bright_cyan: (139, 233, 253),
            bright_white: (255, 255, 255),
        }
    }
    
    pub fn monokai() -> Self {
        Self {
            background: (39, 40, 34),
            foreground: (248, 248, 242),
            black: (39, 40, 34),
            red: (249, 38, 114),
            green: (166, 226, 46),
            yellow: (244, 191, 117),
            blue: (102, 217, 239),
            magenta: (174, 129, 255),
            cyan: (161, 239, 228),
            white: (248, 248, 242),
            bright_black: (117, 113, 94),
            bright_red: (249, 38, 114),
            bright_green: (166, 226, 46),
            bright_yellow: (244, 191, 117),
            bright_blue: (102, 217, 239),
            bright_magenta: (174, 129, 255),
            bright_cyan: (161, 239, 228),
            bright_white: (248, 248, 242),
        }
    }
    
    pub fn solarized_dark() -> Self {
        Self {
            background: (0, 43, 54),
            foreground: (131, 148, 150),
            black: (7, 54, 66),
            red: (220, 50, 47),
            green: (133, 153, 0),
            yellow: (181, 137, 0),
            blue: (38, 139, 210),
            magenta: (211, 54, 130),
            cyan: (42, 161, 152),
            white: (238, 232, 213),
            bright_black: (0, 43, 54),
            bright_red: (203, 75, 22),
            bright_green: (88, 110, 117),
            bright_yellow: (101, 123, 131),
            bright_blue: (131, 148, 150),
            bright_magenta: (108, 113, 196),
            bright_cyan: (147, 161, 161),
            bright_white: (253, 246, 227),
        }
    }
    
    pub fn solarized_light() -> Self {
        Self {
            background: (253, 246, 227),
            foreground: (101, 123, 131),
            black: (7, 54, 66),
            red: (220, 50, 47),
            green: (133, 153, 0),
            yellow: (181, 137, 0),
            blue: (38, 139, 210),
            magenta: (211, 54, 130),
            cyan: (42, 161, 152),
            white: (238, 232, 213),
            bright_black: (0, 43, 54),
            bright_red: (203, 75, 22),
            bright_green: (88, 110, 117),
            bright_yellow: (101, 123, 131),
            bright_blue: (131, 148, 150),
            bright_magenta: (108, 113, 196),
            bright_cyan: (147, 161, 161),
            bright_white: (253, 246, 227),
        }
    }
    
    pub fn from_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "black" => Self::black(),
            "dracula" => Self::dracula(),
            "monokai" => Self::monokai(),
            "solarized-dark" => Self::solarized_dark(),
            "solarized-light" => Self::solarized_light(),
            _ => Self::auto(),
        }
    }
    
    pub fn get_color(&self, index: u8) -> (u8, u8, u8) {
        match index {
            0 => self.black,
            1 => self.red,
            2 => self.green,
            3 => self.yellow,
            4 => self.blue,
            5 => self.magenta,
            6 => self.cyan,
            7 => self.white,
            8 => self.bright_black,
            9 => self.bright_red,
            10 => self.bright_green,
            11 => self.bright_yellow,
            12 => self.bright_blue,
            13 => self.bright_magenta,
            14 => self.bright_cyan,
            15 => self.bright_white,
            _ => self.foreground,
        }
    }
    
    pub fn get_256_color(index: u8) -> (u8, u8, u8) {
        match index {
            // Standard 16 colors (0-15)
            0..=15 => {
                // Use standard ANSI colors
                match index {
                    0 => (0, 0, 0),         // black
                    1 => (128, 0, 0),       // red
                    2 => (0, 128, 0),       // green
                    3 => (128, 128, 0),     // yellow
                    4 => (0, 0, 128),       // blue
                    5 => (128, 0, 128),     // magenta
                    6 => (0, 128, 128),     // cyan
                    7 => (192, 192, 192),   // white
                    8 => (128, 128, 128),   // bright black
                    9 => (255, 0, 0),       // bright red
                    10 => (0, 255, 0),      // bright green
                    11 => (255, 255, 0),    // bright yellow
                    12 => (0, 0, 255),      // bright blue
                    13 => (255, 0, 255),    // bright magenta
                    14 => (0, 255, 255),    // bright cyan
                    15 => (255, 255, 255),  // bright white
                    _ => (255, 255, 255),
                }
            }
            // 6x6x6 RGB cube (16-231)
            16..=231 => {
                let index = index - 16;
                let r = index / 36;
                let g = (index % 36) / 6;
                let b = index % 6;
                
                // Convert 0-5 range to 0-255 range
                let r = if r == 0 { 0 } else { 55 + r * 40 };
                let g = if g == 0 { 0 } else { 55 + g * 40 };
                let b = if b == 0 { 0 } else { 55 + b * 40 };
                
                (r, g, b)
            }
            // Grayscale (232-255)
            232..=255 => {
                let gray = 8 + (index - 232) * 10;
                (gray, gray, gray)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub ch: char,
    pub fg_color: (u8, u8, u8),
    pub bg_color: (u8, u8, u8),
    pub bold: bool,
    #[allow(dead_code)] // Tracked by SGR processing; rendering support planned
    pub italic: bool,
    pub underline: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg_color: (255, 255, 255),
            bg_color: (0, 0, 0),
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

pub struct TerminalState {
    pub grid: Vec<Vec<Cell>>,
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub width: usize,
    pub height: usize,
    pub theme: TerminalTheme,
    pub current_fg: (u8, u8, u8),
    pub current_bg: (u8, u8, u8),
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    parser: Parser,
    // Saved cursor position for ESC 7/8
    saved_cursor_x: usize,
    saved_cursor_y: usize,
    // Scrolling region
    scroll_top: usize,
    scroll_bottom: usize,
    // Character set state
    use_acs: bool,
    // Cursor visibility
    cursor_visible: bool,
    // Dynamic color palette for OSC sequences
    dynamic_palette: Vec<(u8, u8, u8)>,
    // Selection colors
    selection_fg: Option<(u8, u8, u8)>,
    selection_bg: Option<(u8, u8, u8)>,
    // Override colors from OSC 10/11
    override_fg: Option<(u8, u8, u8)>,
    override_bg: Option<(u8, u8, u8)>,
    // Terminal modes (tracked for correctness but not used in rendering currently)
}

impl TerminalState {
    pub fn new(width: usize, height: usize, theme: TerminalTheme) -> Self {
        let mut grid = Vec::new();
        for _ in 0..height {
            let mut row = Vec::new();
            for _ in 0..width {
                row.push(Cell {
                    ch: ' ',
                    fg_color: theme.foreground,
                    bg_color: theme.background,
                    ..Default::default()
                });
            }
            grid.push(row);
        }
        
        // Initialize the standard 256-color palette
        let mut dynamic_palette = Vec::with_capacity(256);
        for i in 0..=255 {
            dynamic_palette.push(TerminalTheme::get_256_color(i));
        }
        
        Self {
            grid,
            cursor_x: 0,
            cursor_y: 0,
            width,
            height,
            current_fg: theme.foreground,
            current_bg: theme.background,
            bold: false,
            italic: false,
            underline: false,
            theme,
            parser: Parser::new(),
            saved_cursor_x: 0,
            saved_cursor_y: 0,
            scroll_top: 0,
            scroll_bottom: height - 1,
            use_acs: false,
            cursor_visible: true,
            dynamic_palette,
            selection_fg: Some((0, 0, 0)),    // Default selection: black on yellow
            selection_bg: Some((255, 255, 0)),
            override_fg: None,
            override_bg: None,
        }
    }
    
    pub fn process_output(&mut self, data: &str) -> Result<()> {
        let bytes: Vec<u8> = data.bytes().collect();
        // The VTE parser expects a slice of bytes
        for byte in bytes {
            // We need to handle the borrow checker by using a temporary approach
            // The VTE parser calls back into our Perform implementation
            let mut temp_parser = std::mem::replace(&mut self.parser, Parser::new());
            temp_parser.advance(self, &[byte]);
            self.parser = temp_parser;
        }
        Ok(())
    }
    
    pub fn get_grid(&self) -> &Vec<Vec<Cell>> {
        &self.grid
    }
    
    pub fn get_theme(&self) -> &TerminalTheme {
        &self.theme
    }
    
    #[cfg(test)]
    pub fn get_width(&self) -> usize {
        self.width
    }

    #[cfg(test)]
    pub fn get_height(&self) -> usize {
        self.height
    }
    
    pub fn get_cursor_position(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }
    
    pub fn is_cursor_visible(&self) -> bool {
        self.cursor_visible
    }
    
    
    pub fn resolve_cell_colors(&self, cell: &Cell) -> ((u8, u8, u8), (u8, u8, u8)) {
        let mut fg = cell.fg_color;
        let mut bg = cell.bg_color;

        // Only check palette entries that have been modified (not all 256)
        for (i, &dynamic_color) in self.dynamic_palette.iter().enumerate() {
            let standard_color = TerminalTheme::get_256_color(i as u8);
            if dynamic_color == standard_color {
                continue; // Skip unmodified entries
            }
            if fg == standard_color {
                fg = dynamic_color;
            }
            if bg == standard_color {
                bg = dynamic_color;
            }
        }
        
        // Apply global foreground/background color overrides from OSC 10/11
        if let Some(override_fg) = self.override_fg {
            // Replace theme foreground or any color that matches the theme foreground
            if fg == self.theme.foreground {
                fg = override_fg;
            }
        }
        if let Some(override_bg) = self.override_bg {
            // Replace theme background or any color that matches the theme background
            if bg == self.theme.background {
                bg = override_bg;
            }
        }
        
        // Handle common tmux color issues
        // tmux often uses specific color combinations that need adjustment
        self.apply_tmux_color_corrections(fg, bg)
    }
    
    fn apply_tmux_color_corrections(&self, fg: (u8, u8, u8), bg: (u8, u8, u8)) -> ((u8, u8, u8), (u8, u8, u8)) {
        // Fix common tmux color rendering issues
        
        // Handle tmux status bar colors - bright green should be normal green
        let corrected_bg = match bg {
            (0, 255, 0) => (0, 128, 0),    // Bright green -> normal green
            (85, 255, 85) => (0, 128, 0),  // Another bright green variant
            (80, 250, 123) => (0, 128, 0), // Dracula bright green -> normal green
            _ => bg
        };
        
        // Handle tmux status bar foreground text color
        let corrected_fg = if corrected_bg == (0, 128, 0) {
            // Status bar should have black text on green background for maximum visibility
            (0, 0, 0)
        } else if bg == (0, 255, 0) || bg == (85, 255, 85) || bg == (80, 250, 123) {
            // Even if we didn't correct the background, ensure text is visible on any green
            (0, 0, 0)
        } else if corrected_bg != (0, 0, 0) && fg == corrected_bg {
            // If foreground matches background, force contrasting color
            if is_light_color_terminal(corrected_bg) {
                (0, 0, 0)      // Black text on light background
            } else {
                (255, 255, 255) // White text on dark background
            }
        } else {
            fg
        };
        
        // Handle selection colors - ensure proper contrast for yellow selection background
        let (final_fg, final_bg) = if corrected_bg == (255, 255, 0) || corrected_bg == (255, 255, 85) {
            // Yellow background should have black foreground for selection
            ((0, 0, 0), corrected_bg)
        } else {
            (corrected_fg, corrected_bg)
        };
        
        (final_fg, final_bg)
    }
    
    pub fn is_status_bar_row(&self, row_index: usize) -> bool {
        // Check if this row index is the tmux status bar
        if let Some(layout) = self.detect_tmux_layout() {
            row_index == layout.status_bar_row
        } else {
            false
        }
    }
    
    pub fn detect_tmux_layout(&self) -> Option<TmuxLayout> {
        let grid = &self.grid;
        if grid.is_empty() {
            return None;
        }
        
        // Check for tmux status bar - usually at the bottom
        let last_row = grid.len() - 1;
        if last_row > 0 {
            let status_row = &grid[last_row];
            
            // Check if this row has characteristics of a tmux status bar.
            // Only match on specific known tmux status bar colors, not any non-default color.
            let has_tmux_green_bg = status_row.iter().any(|cell| {
                matches!(cell.bg_color,
                    (0, 255, 0) | (85, 255, 85) | (0, 128, 0) | (80, 250, 123)
                )
            });

            // Also check for full-width inverted row (white bg / black fg) with bracket chars
            let has_inverted_status = {
                let inverted_count = status_row.iter().filter(|cell| {
                    cell.bg_color == (255, 255, 255) && cell.fg_color == (0, 0, 0)
                }).count();
                let has_brackets = status_row.iter().any(|cell| matches!(cell.ch, '[' | ']'));
                inverted_count > status_row.len() / 2 && has_brackets
            };

            if has_tmux_green_bg || has_inverted_status {
                return Some(TmuxLayout {
                    status_bar_row: last_row,
                    content_height: last_row,
                });
            }
        }
        
        None
    }
    
    pub fn is_valid_cursor_position(&self, tmux_layout: Option<&TmuxLayout>) -> bool {
        // Check if cursor is within valid content area considering tmux layout
        let max_row = if let Some(layout) = tmux_layout {
            layout.content_height
        } else {
            self.height
        };
        
        self.cursor_y < max_row && self.cursor_x < self.width
    }
    
    pub fn adjust_coordinates_for_tmux(&self, x: usize, y: usize, tmux_layout: Option<&TmuxLayout>) -> (usize, usize) {
        // Adjust coordinates to account for tmux layout
        if let Some(layout) = tmux_layout {
            // Ensure we don't render outside the content area
            let adjusted_y = if y >= layout.content_height {
                layout.content_height - 1
            } else {
                y
            };
            (x, adjusted_y)
        } else {
            (x, y)
        }
    }
    
    
    pub fn protect_status_bar_area(&mut self) {
        // Ensure status bar area is protected from application content
        if let Some(layout) = self.detect_tmux_layout() {
            let status_row = layout.status_bar_row;
            
            // Clear any non-status content that might have been written to the status bar
            if status_row < self.grid.len() {
                let row = &mut self.grid[status_row];
                
                // Check if this looks like application content rather than status bar
                let non_status_chars = row.iter().filter(|cell| {
                    // Look for patterns that suggest this is application content
                    matches!(cell.ch, 'A'..='Z' | 'a'..='z' | '0'..='9') &&
                    cell.bg_color == (0, 0, 0) && // Default background
                    cell.fg_color == (255, 255, 255) // Default foreground
                }).count();
                
                // If more than 20% of the status bar looks like application content, clear it
                if non_status_chars > row.len() / 5 {
                    for cell in row {
                        if cell.bg_color == (0, 0, 0) && cell.fg_color == (255, 255, 255) {
                            // Only clear cells that look like application content
                            cell.ch = ' ';
                            cell.bg_color = (0, 128, 0); // Set to status bar green
                            cell.fg_color = (0, 0, 0);   // Black text
                        }
                    }
                }
            }
        }
    }
    
    pub fn debug_status_bar(&self) {
        // Debug function to analyze status bar content
        if let Some(layout) = self.detect_tmux_layout() {
            let status_row = layout.status_bar_row;
            if status_row < self.grid.len() {
                eprintln!("Status bar analysis (row {}):", status_row);
                let row = &self.grid[status_row];
                
                for (i, cell) in row.iter().enumerate().take(20) { // Show first 20 chars
                    if cell.ch != ' ' || cell.bg_color != (0, 0, 0) {
                        eprintln!("  [{}] char='{}' fg=({},{},{}) bg=({},{},{})", 
                            i, cell.ch, 
                            cell.fg_color.0, cell.fg_color.1, cell.fg_color.2,
                            cell.bg_color.0, cell.bg_color.1, cell.bg_color.2);
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TmuxLayout {
    pub status_bar_row: usize,
    pub content_height: usize,
}

// Helper function for color analysis in terminal renderer
fn is_light_color_terminal(color: (u8, u8, u8)) -> bool {
    let luminance = 0.299 * color.0 as f32 + 0.587 * color.1 as f32 + 0.114 * color.2 as f32;
    luminance > 127.0
}

impl TerminalState {
    fn put_char(&mut self, ch: char) {
        // Convert ACS characters to Unicode box drawing if needed
        let display_char = if self.use_acs {
            match ch {
                'j' => '┘', // Lower right corner
                'k' => '┐', // Upper right corner
                'l' => '┌', // Upper left corner
                'm' => '└', // Lower left corner
                'n' => '┼', // Cross
                'q' => '─', // Horizontal line
                't' => '├', // Left T
                'u' => '┤', // Right T
                'v' => '┴', // Bottom T
                'w' => '┬', // Top T
                'x' => '│', // Vertical line
                _ => ch,
            }
        } else {
            ch
        };
        
        
        if self.cursor_y < self.height && self.cursor_x < self.width {
            self.grid[self.cursor_y][self.cursor_x] = Cell {
                ch: display_char,
                fg_color: self.current_fg,
                bg_color: self.current_bg,
                bold: self.bold,
                italic: self.italic,
                underline: self.underline,
            };
            self.cursor_x += 1;
            
            // Don't auto-wrap at line end - just stop at the edge
            if self.cursor_x >= self.width {
                self.cursor_x = self.width - 1;
            }
        }
    }
    
    fn scroll_up(&mut self) {
        // Only scroll within the scrolling region
        for y in (self.scroll_top + 1)..=self.scroll_bottom {
            for x in 0..self.width {
                self.grid[y - 1][x] = self.grid[y][x].clone();
            }
        }
        
        // Clear the bottom line of the scrolling region
        for x in 0..self.width {
            self.grid[self.scroll_bottom][x] = Cell {
                ch: ' ',
                fg_color: self.theme.foreground,
                bg_color: self.theme.background,
                ..Default::default()
            };
        }
    }
    
    fn clear_line(&mut self, line: usize) {
        if line < self.height {
            for x in 0..self.width {
                self.grid[line][x] = Cell {
                    ch: ' ',
                    fg_color: self.theme.foreground,
                    bg_color: self.theme.background,
                    ..Default::default()
                };
            }
        }
    }
    
    fn clear_screen(&mut self) {
        for y in 0..self.height {
            for x in 0..self.width {
                self.grid[y][x] = Cell {
                    ch: ' ',
                    fg_color: self.theme.foreground,
                    bg_color: self.theme.background,
                    ..Default::default()
                };
            }
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }
    
    fn process_sgr_params(&mut self, params: &vte::Params) {
        let param_vec: Vec<u16> = params.iter().flatten().copied().collect();
        let mut i = 0;
        
        while i < param_vec.len() {
            let param = param_vec[i];
            
            match param {
                0 => {
                    // Reset
                    self.current_fg = self.theme.foreground;
                    self.current_bg = self.theme.background;
                    self.bold = false;
                    self.italic = false;
                    self.underline = false;
                }
                1 => self.bold = true,
                3 => self.italic = true,
                4 => self.underline = true,
                22 => self.bold = false,
                23 => self.italic = false,
                24 => self.underline = false,
                30..=37 => {
                    // Foreground colors
                    let color_index = (param - 30) as u8;
                    self.current_fg = self.theme.get_color(color_index);
                }
                38 => {
                    // Extended foreground color
                    if i + 1 < param_vec.len() {
                        match param_vec[i + 1] {
                            5 => {
                                // 256-color mode: ESC[38;5;n
                                if i + 2 < param_vec.len() {
                                    let color_index = param_vec[i + 2] as u8;
                                    if (color_index as usize) < self.dynamic_palette.len() {
                                        self.current_fg = self.dynamic_palette[color_index as usize];
                                    } else {
                                        self.current_fg = TerminalTheme::get_256_color(color_index);
                                    }
                                    i += 2; // Skip the 5 and color index
                                }
                            }
                            2 => {
                                // 24-bit RGB mode: ESC[38;2;r;g;b
                                if i + 4 < param_vec.len() {
                                    let r = param_vec[i + 2] as u8;
                                    let g = param_vec[i + 3] as u8;
                                    let b = param_vec[i + 4] as u8;
                                    self.current_fg = (r, g, b);
                                    i += 4; // Skip the 2, r, g, b
                                }
                            }
                            _ => {}
                        }
                    }
                }
                40..=47 => {
                    // Background colors
                    let color_index = (param - 40) as u8;
                    self.current_bg = self.theme.get_color(color_index);
                }
                48 => {
                    // Extended background color
                    if i + 1 < param_vec.len() {
                        match param_vec[i + 1] {
                            5 => {
                                // 256-color mode: ESC[48;5;n
                                if i + 2 < param_vec.len() {
                                    let color_index = param_vec[i + 2] as u8;
                                    if (color_index as usize) < self.dynamic_palette.len() {
                                        self.current_bg = self.dynamic_palette[color_index as usize];
                                    } else {
                                        self.current_bg = TerminalTheme::get_256_color(color_index);
                                    }
                                    i += 2; // Skip the 5 and color index
                                }
                            }
                            2 => {
                                // 24-bit RGB mode: ESC[48;2;r;g;b
                                if i + 4 < param_vec.len() {
                                    let r = param_vec[i + 2] as u8;
                                    let g = param_vec[i + 3] as u8;
                                    let b = param_vec[i + 4] as u8;
                                    self.current_bg = (r, g, b);
                                    i += 4; // Skip the 2, r, g, b
                                }
                            }
                            _ => {}
                        }
                    }
                }
                90..=97 => {
                    // Bright foreground colors
                    let color_index = (param - 90 + 8) as u8;
                    self.current_fg = self.theme.get_color(color_index);
                }
                100..=107 => {
                    // Bright background colors
                    let color_index = (param - 100 + 8) as u8;
                    self.current_bg = self.theme.get_color(color_index);
                }
                _ => {}
            }
            
            i += 1;
        }
    }
    
    /// Public wrapper for testing parse_color_spec.
    #[cfg(test)]
    pub fn parse_color_spec_pub(&self, color_spec: &str) -> Option<(u8, u8, u8)> {
        self.parse_color_spec(color_spec)
    }

    // All byte-level indexing below is safe because we reject non-ASCII input
    // via `is_ascii()` checks before any indexing occurs.
    #[allow(clippy::string_slice)]
    fn parse_color_spec(&self, color_spec: &str) -> Option<(u8, u8, u8)> {
        let spec = color_spec.trim();
        if spec.starts_with('#') && spec.len() == 7 && spec.is_ascii() {
            // Hex format: #RRGGBB
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&spec[1..3], 16),
                u8::from_str_radix(&spec[3..5], 16),
                u8::from_str_radix(&spec[5..7], 16),
            ) {
                return Some((r, g, b));
            }
        } else if spec.starts_with("rgb:") && spec.is_ascii() {
            // X11 RGB format: rgb:RRRR/GGGG/BBBB or rgb:RR/GG/BB
            let rgb_part = &spec[4..];
            let parts: Vec<&str> = rgb_part.split('/').collect();
            if parts.len() == 3 {
                let parse_component = |s: &str| -> Option<u8> {
                    match s.len() {
                        2 => u8::from_str_radix(s, 16).ok(),
                        4 => u8::from_str_radix(&s[..2], 16).ok(),
                        _ => None,
                    }
                };

                if let (Some(r), Some(g), Some(b)) = (
                    parse_component(parts[0]),
                    parse_component(parts[1]),
                    parse_component(parts[2]),
                ) {
                    return Some((r, g, b));
                }
            }
        } else {
            // Named colors - basic support
            match spec.to_lowercase().as_str() {
                "black" => return Some((0, 0, 0)),
                "red" => return Some((255, 0, 0)),
                "green" => return Some((0, 255, 0)),
                "yellow" => return Some((255, 255, 0)),
                "blue" => return Some((0, 0, 255)),
                "magenta" => return Some((255, 0, 255)),
                "cyan" => return Some((0, 255, 255)),
                "white" => return Some((255, 255, 255)),
                _ => {}
            }
        }
        
        None
    }
}

impl Perform for TerminalState {
    fn print(&mut self, ch: char) {
        self.put_char(ch);
    }
    
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.cursor_y += 1;
                // Check if we need to scroll within the scrolling region
                if self.cursor_y > self.scroll_bottom {
                    self.scroll_up();
                    self.cursor_y = self.scroll_bottom;
                }
            }
            b'\r' => {
                self.cursor_x = 0;
            }
            b'\t' => {
                let spaces = 8 - (self.cursor_x % 8);
                for _ in 0..spaces {
                    self.put_char(' ');
                }
            }
            b'\x08' => {
                // Backspace
                if self.cursor_x > 0 {
                    self.cursor_x -= 1;
                }
            }
            _ => {}
        }
    }
    
    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {
        // DCS sequence start — consumed without action
    }

    fn put(&mut self, _byte: u8) {
        // Bytes within DCS/OSC sequences — consumed without displaying
    }

    fn unhook(&mut self) {
        // DCS sequence end — consumed without action
    }
    
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // Handle OSC sequences - these are operating system commands
        if params.is_empty() {
            return;
        }
        
        let first_param = std::str::from_utf8(params[0]).unwrap_or("");
        
        match first_param {
            "4" => {
                // OSC 4 ; color_index ; color_spec ST
                // Set/modify color palette entry
                if params.len() >= 3 {
                    if let Ok(index) = std::str::from_utf8(params[1]).unwrap_or("").parse::<u8>() {
                        let color_spec = std::str::from_utf8(params[2]).unwrap_or("");
                        if let Some(color) = self.parse_color_spec(color_spec) {
                            if (index as usize) < self.dynamic_palette.len() {
                                self.dynamic_palette[index as usize] = color;
                            }
                        }
                    }
                }
            }
            "10" => {
                // OSC 10 ; color_spec ST - Set text foreground color
                if params.len() >= 2 {
                    let color_spec = std::str::from_utf8(params[1]).unwrap_or("");
                    if let Some(color) = self.parse_color_spec(color_spec) {
                        self.override_fg = Some(color);
                    }
                }
            }
            "11" => {
                // OSC 11 ; color_spec ST - Set text background color  
                if params.len() >= 2 {
                    let color_spec = std::str::from_utf8(params[1]).unwrap_or("");
                    if let Some(color) = self.parse_color_spec(color_spec) {
                        self.override_bg = Some(color);
                    }
                }
            }
            "17" => {
                // OSC 17 ; color_spec ST - Set selection background color
                if params.len() >= 2 {
                    let color_spec = std::str::from_utf8(params[1]).unwrap_or("");
                    if let Some(color) = self.parse_color_spec(color_spec) {
                        self.selection_bg = Some(color);
                    }
                }
            }
            "19" => {
                // OSC 19 ; color_spec ST - Set selection foreground color
                if params.len() >= 2 {
                    let color_spec = std::str::from_utf8(params[1]).unwrap_or("");
                    if let Some(color) = self.parse_color_spec(color_spec) {
                        self.selection_fg = Some(color);
                    }
                }
            }
            "52" => {
                // OSC 52 - Clipboard operations - ignore for security
            }
            "104" => {
                // OSC 104 ; index_list ST - Reset color palette entries
                if params.len() >= 2 {
                    let indices = std::str::from_utf8(params[1]).unwrap_or("");
                    if indices.is_empty() {
                        // Reset all colors
                        for i in 0..=255 {
                            self.dynamic_palette[i] = TerminalTheme::get_256_color(i as u8);
                        }
                    } else {
                        // Reset specific indices
                        for index_str in indices.split(';') {
                            if let Ok(index) = index_str.parse::<u8>() {
                                if (index as usize) < self.dynamic_palette.len() {
                                    self.dynamic_palette[index as usize] = TerminalTheme::get_256_color(index);
                                }
                            }
                        }
                    }
                }
            }
            "110" => {
                // OSC 110 ST - Reset text foreground color
                self.override_fg = None;
            }
            "111" => {
                // OSC 111 ST - Reset text background color
                self.override_bg = None;
            }
            "117" => {
                // OSC 117 ST - Reset selection background color
                self.selection_bg = Some((255, 255, 0)); // Reset to default yellow
            }
            "119" => {
                // OSC 119 ST - Reset selection foreground color
                self.selection_fg = Some((0, 0, 0)); // Reset to default black
            }
            _ => {
                // Ignore other OSC sequences
            }
        }
    }
    
    fn csi_dispatch(&mut self, params: &vte::Params, intermediates: &[u8], _ignore: bool, action: char) {
        // Check for private mode sequences (CSI ? ...)
        if intermediates.contains(&b'?') {
            match action {
                'h' => {
                    // DECSET - Set Mode
                    for param in params.iter().flatten() {
                        match *param {
                            25 => self.cursor_visible = true,
                            1049 => {
                                self.saved_cursor_x = self.cursor_x;
                                self.saved_cursor_y = self.cursor_y;
                            }
                            // 47, 1047, 1000, 1002, 1006, 2004: acknowledged but not rendered
                            _ => {}
                        }
                    }
                }
                'l' => {
                    // DECRST - Reset Mode
                    for param in params.iter().flatten() {
                        match *param {
                            25 => self.cursor_visible = false,
                            1049 => {
                                self.cursor_x = self.saved_cursor_x;
                                self.cursor_y = self.saved_cursor_y;
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        
        match action {
            'H' | 'f' => {
                // Cursor position
                let row = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1);
                let col = params.iter().nth(1).and_then(|p| p.first().copied()).unwrap_or(1);
                self.cursor_y = ((row as usize).saturating_sub(1)).min(self.height - 1);
                self.cursor_x = ((col as usize).saturating_sub(1)).min(self.width - 1);
            }
            'A' => {
                // Cursor up
                let count = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1) as usize;
                self.cursor_y = self.cursor_y.saturating_sub(count);
            }
            'B' => {
                // Cursor down
                let count = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1) as usize;
                self.cursor_y = (self.cursor_y + count).min(self.height - 1);
            }
            'C' => {
                // Cursor right
                let count = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1) as usize;
                self.cursor_x = (self.cursor_x + count).min(self.width - 1);
            }
            'D' => {
                // Cursor left
                let count = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1) as usize;
                self.cursor_x = self.cursor_x.saturating_sub(count);
            }
            'J' => {
                // Erase in display
                let mode = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(0);
                match mode {
                    0 => {
                        // Clear from cursor to end of screen
                        for x in self.cursor_x..self.width {
                            self.grid[self.cursor_y][x] = Cell {
                                ch: ' ',
                                fg_color: self.theme.foreground,
                                bg_color: self.theme.background,
                                ..Default::default()
                            };
                        }
                        for y in (self.cursor_y + 1)..self.height {
                            self.clear_line(y);
                        }
                    }
                    1 => {
                        // Clear from start of screen to cursor
                        for y in 0..self.cursor_y {
                            self.clear_line(y);
                        }
                        for x in 0..=self.cursor_x {
                            self.grid[self.cursor_y][x] = Cell {
                                ch: ' ',
                                fg_color: self.theme.foreground,
                                bg_color: self.theme.background,
                                ..Default::default()
                            };
                        }
                    }
                    2 => {
                        // Clear entire screen
                        self.clear_screen();
                    }
                    _ => {}
                }
            }
            'K' => {
                // Erase in line
                let mode = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(0);
                match mode {
                    0 => {
                        // Clear from cursor to end of line
                        for x in self.cursor_x..self.width {
                            self.grid[self.cursor_y][x] = Cell {
                                ch: ' ',
                                fg_color: self.theme.foreground,
                                bg_color: self.theme.background,
                                ..Default::default()
                            };
                        }
                    }
                    1 => {
                        // Clear from start of line to cursor
                        for x in 0..=self.cursor_x {
                            self.grid[self.cursor_y][x] = Cell {
                                ch: ' ',
                                fg_color: self.theme.foreground,
                                bg_color: self.theme.background,
                                ..Default::default()
                            };
                        }
                    }
                    2 => {
                        // Clear entire line
                        self.clear_line(self.cursor_y);
                    }
                    _ => {}
                }
            }
            'm' => {
                // Set graphics rendition (colors, bold, etc.)
                self.process_sgr_params(params);
            }
            'r' => {
                // Set scrolling region (DECSTBM)
                let top = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1) as usize;
                let bottom = params.iter().nth(1).and_then(|p| p.first().copied()).unwrap_or(self.height as u16) as usize;
                
                // Validate and set scrolling region
                if top > 0 && bottom <= self.height && top < bottom {
                    self.scroll_top = top - 1;  // Convert to 0-based
                    self.scroll_bottom = bottom - 1;
                    
                    // DECSTBM also moves cursor to home position
                    self.cursor_x = 0;
                    self.cursor_y = 0;
                }
            }
            'c' => {
                // Device Attributes (DA) request - ignore for now
                // tmux sends this to query terminal capabilities
                // We're not responding, just consuming it
            }
            's' => {
                // Save cursor position (ANSI.SYS style) - similar to ESC 7
                self.saved_cursor_x = self.cursor_x;
                self.saved_cursor_y = self.cursor_y;
            }
            'u' => {
                // Restore cursor position (ANSI.SYS style) - similar to ESC 8
                self.cursor_x = self.saved_cursor_x;
                self.cursor_y = self.saved_cursor_y;
            }
            'S' => {
                // Scroll up
                let count = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1) as usize;
                for _ in 0..count {
                    self.scroll_up();
                }
            }
            'T' => {
                // Scroll down: shift content DOWN within scroll region, clear top
                let count = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1) as usize;
                for _ in 0..count {
                    for y in (self.scroll_top + 1..=self.scroll_bottom).rev() {
                        for x in 0..self.width {
                            self.grid[y][x] = self.grid[y - 1][x].clone();
                        }
                    }
                    self.clear_line(self.scroll_top);
                }
            }
            'L' => {
                // Insert lines
                let count = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1) as usize;
                for _ in 0..count {
                    // Move lines down from current cursor position
                    for y in (self.cursor_y + 1..self.height).rev() {
                        for x in 0..self.width {
                            if y > 0 {
                                self.grid[y][x] = self.grid[y - 1][x].clone();
                            }
                        }
                    }
                    self.clear_line(self.cursor_y);
                }
            }
            'M' => {
                // Delete lines
                let count = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1) as usize;
                for _ in 0..count {
                    // Move lines up from current cursor position
                    for y in self.cursor_y..(self.height - 1) {
                        for x in 0..self.width {
                            self.grid[y][x] = self.grid[y + 1][x].clone();
                        }
                    }
                    self.clear_line(self.height - 1);
                }
            }
            'P' => {
                // Delete characters (clamped to avoid usize underflow)
                let raw_count = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1) as usize;
                let count = raw_count.min(self.width.saturating_sub(self.cursor_x));
                let start_x = self.cursor_x;
                for x in start_x..self.width.saturating_sub(count) {
                    self.grid[self.cursor_y][x] = self.grid[self.cursor_y][x + count].clone();
                }
                for x in self.width.saturating_sub(count)..self.width {
                    self.grid[self.cursor_y][x] = Cell {
                        ch: ' ',
                        fg_color: self.theme.foreground,
                        bg_color: self.theme.background,
                        ..Default::default()
                    };
                }
            }
            '@' => {
                // Insert characters (clamped to avoid usize underflow)
                let raw_count = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1) as usize;
                let count = raw_count.min(self.width.saturating_sub(self.cursor_x));
                let start_x = self.cursor_x;
                for x in (start_x..self.width.saturating_sub(count)).rev() {
                    self.grid[self.cursor_y][x + count] = self.grid[self.cursor_y][x].clone();
                }
                for x in start_x..(start_x + count).min(self.width) {
                    self.grid[self.cursor_y][x] = Cell {
                        ch: ' ',
                        fg_color: self.theme.foreground,
                        bg_color: self.theme.background,
                        ..Default::default()
                    };
                }
            }
            'X' => {
                // Erase characters
                let count = params.iter().nth(0).and_then(|p| p.first().copied()).unwrap_or(1) as usize;
                for i in 0..count {
                    if self.cursor_x + i < self.width {
                        self.grid[self.cursor_y][self.cursor_x + i] = Cell {
                            ch: ' ',
                            fg_color: self.theme.foreground,
                            bg_color: self.theme.background,
                            ..Default::default()
                        };
                    }
                }
            }
            _ => {}
        }
    }
    
    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => {
                // Save cursor position (DECSC)
                self.saved_cursor_x = self.cursor_x;
                self.saved_cursor_y = self.cursor_y;
            }
            b'8' => {
                // Restore cursor position (DECRC)
                self.cursor_x = self.saved_cursor_x;
                self.cursor_y = self.saved_cursor_y;
            }
            b'D' => {
                // Index (IND) - move cursor down, scroll if at bottom
                self.cursor_y += 1;
                if self.cursor_y > self.scroll_bottom {
                    self.scroll_up();
                    self.cursor_y = self.scroll_bottom;
                }
            }
            b'E' => {
                // Next Line (NEL) - move to first column of next line
                self.cursor_x = 0;
                self.cursor_y += 1;
                if self.cursor_y > self.scroll_bottom {
                    self.scroll_up();
                    self.cursor_y = self.scroll_bottom;
                }
            }
            b'M' => {
                // Reverse Index (RI) - move cursor up, scroll if at top
                if self.cursor_y == self.scroll_top {
                    // Scroll down by moving lines down and clearing the top
                    for y in (self.scroll_top + 1..=self.scroll_bottom).rev() {
                        for x in 0..self.width {
                            self.grid[y][x] = self.grid[y - 1][x].clone();
                        }
                    }
                    self.clear_line(self.scroll_top);
                } else if self.cursor_y > 0 {
                    self.cursor_y -= 1;
                }
            }
            b'H' => {
                // Tab Set (HTS) - set tab stop at current column
                // We don't implement tab stops, just consume
            }
            b'=' => {
                // Application Keypad Mode (DECKPAM)
                // Just consume, we don't differentiate keypad modes
            }
            b'>' => {
                // Normal Keypad Mode (DECKPNM)
                // Just consume, we don't differentiate keypad modes
            }
            b'c' => {
                // Reset terminal (RIS) - full reset
                self.clear_screen();
                self.current_fg = self.theme.foreground;
                self.current_bg = self.theme.background;
                self.bold = false;
                self.italic = false;
                self.underline = false;
                self.cursor_visible = true;
                self.use_acs = false;
            }
            _ => {
                // Ignore unhandled escape sequences
            }
        }
        
        // Handle character set designation sequences
        if !intermediates.is_empty() {
            match intermediates[0] {
                b'(' | b')' | b'*' | b'+' => {
                    // Character set designation
                    match byte {
                        b'0' => self.use_acs = true,  // DEC Special Character and Line Drawing Set
                        b'B' => self.use_acs = false, // ASCII character set
                        b'A' => self.use_acs = false, // UK character set
                        _ => {} // Other character sets - just use ASCII
                    }
                }
                _ => {}
            }
        }
    }
}