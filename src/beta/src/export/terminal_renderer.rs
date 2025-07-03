use anyhow::Result;

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
}

#[derive(Debug, Clone)]
pub struct TerminalState {
    pub content: String,
    pub theme: TerminalTheme,
    pub width: usize,
    pub height: usize,
}

impl TerminalState {
    pub fn new(width: usize, height: usize, theme: TerminalTheme) -> Self {
        Self {
            content: String::new(),
            theme,
            width,
            height,
        }
    }
    
    pub fn process_output(&mut self, data: &str) -> Result<()> {
        self.content.push_str(data);
        Ok(())
    }
    
    pub fn get_content(&self) -> &str {
        &self.content
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
}