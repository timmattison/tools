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
        Self::solarized_dark()
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
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub ch: char,
    pub fg_color: (u8, u8, u8),
    pub bg_color: (u8, u8, u8),
    pub bold: bool,
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
        }
    }
    
    pub fn process_output(&mut self, data: &str) -> Result<()> {
        let bytes: Vec<u8> = data.bytes().collect();
        for byte in bytes {
            // We need to handle the borrow checker by using a temporary approach
            // The VTE parser calls back into our Perform implementation
            let mut temp_parser = std::mem::replace(&mut self.parser, Parser::new());
            temp_parser.advance(self, byte);
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
    
    pub fn get_width(&self) -> usize {
        self.width
    }
    
    pub fn get_height(&self) -> usize {
        self.height
    }
    
    fn put_char(&mut self, ch: char) {
        if self.cursor_y < self.height && self.cursor_x < self.width {
            self.grid[self.cursor_y][self.cursor_x] = Cell {
                ch,
                fg_color: self.current_fg,
                bg_color: self.current_bg,
                bold: self.bold,
                italic: self.italic,
                underline: self.underline,
            };
            self.cursor_x += 1;
            
            if self.cursor_x >= self.width {
                self.cursor_x = 0;
                self.cursor_y += 1;
                if self.cursor_y >= self.height {
                    self.scroll_up();
                    self.cursor_y = self.height - 1;
                }
            }
        }
    }
    
    fn scroll_up(&mut self) {
        for y in 1..self.height {
            for x in 0..self.width {
                self.grid[y - 1][x] = self.grid[y][x].clone();
            }
        }
        
        // Clear the last line
        for x in 0..self.width {
            self.grid[self.height - 1][x] = Cell {
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
}

impl Perform for TerminalState {
    fn print(&mut self, ch: char) {
        self.put_char(ch);
    }
    
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.cursor_y += 1;
                if self.cursor_y >= self.height {
                    self.scroll_up();
                    self.cursor_y = self.height - 1;
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
        // Handle hooks if needed
    }
    
    fn put(&mut self, _byte: u8) {
        // Handle put if needed
    }
    
    fn unhook(&mut self) {
        // Handle unhook if needed
    }
    
    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {
        // Handle OSC sequences if needed
    }
    
    fn csi_dispatch(&mut self, params: &vte::Params, _intermediates: &[u8], _ignore: bool, action: char) {
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
                            self.grid[self.cursor_y][x] = Cell::default();
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
                            self.grid[self.cursor_y][x] = Cell::default();
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
                            self.grid[self.cursor_y][x] = Cell::default();
                        }
                    }
                    1 => {
                        // Clear from start of line to cursor
                        for x in 0..=self.cursor_x {
                            self.grid[self.cursor_y][x] = Cell::default();
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
                for param in params.iter().flatten() {
                    match *param {
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
                        40..=47 => {
                            // Background colors
                            let color_index = (param - 40) as u8;
                            self.current_bg = self.theme.get_color(color_index);
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
                }
            }
            _ => {}
        }
    }
    
    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {
        // Handle escape sequences if needed
    }
}