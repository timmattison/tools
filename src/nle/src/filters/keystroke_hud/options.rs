//! The options of the keystroke HUD filter.

use crate::types::FilterOptions;

/// The option that holds the position of the box.
pub const POSITION_OPTION: &str = "position";
/// The option that holds the time that the box stays, in seconds.
pub const DURATION_OPTION: &str = "duration";
/// The option that holds the time of the fade, in seconds.
pub const FADE_OPTION: &str = "fade";
/// The option that holds the name of the theme.
pub const THEME_OPTION: &str = "theme";
/// The option that holds the color of typed text.
pub const TEXT_OPTION: &str = "text";
/// The option that holds the color of the labels of special keys.
pub const ACCENT_OPTION: &str = "accent";
/// The option that holds the color of the inside of the box.
pub const BACKGROUND_OPTION: &str = "background";
/// The option that holds the color of the border of the box.
pub const BORDER_OPTION: &str = "border";

/// The theme names that the filter accepts.
pub(crate) const THEMES: [&str; 6] = [
    "auto",
    "black",
    "dracula",
    "monokai",
    "solarized-dark",
    "solarized-light",
];

/// Where the box of the HUD stands on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Position {
    /// The top left corner.
    TopLeft,
    /// The top edge, in the middle.
    TopCenter,
    /// The top right corner.
    #[default]
    TopRight,
    /// The bottom left corner.
    BottomLeft,
    /// The bottom edge, in the middle.
    BottomCenter,
    /// The bottom right corner.
    BottomRight,
}

impl Position {
    /// Every position, in the order above.
    pub const ALL: [Position; 6] = [
        Position::TopLeft,
        Position::TopCenter,
        Position::TopRight,
        Position::BottomLeft,
        Position::BottomCenter,
        Position::BottomRight,
    ];

    /// The name of the position, for example "top-left".
    #[must_use]
    pub fn as_str(self) -> &'static str {
        "top-left"
    }

    /// The position that has `name`, or `None`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let _ = name;
        None
    }
}

/// A 24-bit color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    /// The red channel.
    pub red: u8,
    /// The green channel.
    pub green: u8,
    /// The blue channel.
    pub blue: u8,
}

impl Rgb {
    /// Make a color from its channels.
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// The color of `text` in the form `#rrggbb` (hex digits in either case),
    /// or `None`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let _ = text;
        None
    }

    /// The color in the form `#rrggbb`, with lower-case hex digits.
    #[must_use]
    pub fn to_hex(self) -> String {
        String::new()
    }
}

/// The options of the keystroke HUD.
#[derive(Debug, Clone, PartialEq)]
pub struct HudOptions {
    /// Where the box stands.
    pub position: Position,
    /// How long the box stays after the last key of its group, in seconds. It
    /// is finite and above zero.
    pub duration: f64,
    /// How long the box takes to fade in and to fade out, in seconds. It is
    /// finite and not below zero. Zero turns the fade off.
    pub fade: f64,
    /// The name of the theme of the export (`beta export --theme`). The fade
    /// starts from the background of this theme.
    pub theme: String,
    /// The color of typed text.
    pub text: Rgb,
    /// The color of the labels of special keys, for example `Enter` or
    /// `Ctrl+C`.
    pub accent: Rgb,
    /// The color of the inside of the box.
    pub background: Rgb,
    /// The color of the border of the box.
    pub border: Rgb,
}

impl Default for HudOptions {
    fn default() -> Self {
        let black = Rgb::new(0, 0, 0);
        Self {
            position: Position::TopLeft,
            duration: 1.0,
            fade: 1.0,
            theme: String::new(),
            text: black,
            accent: black,
            background: black,
            border: black,
        }
    }
}

impl HudOptions {
    /// Read the options of a filter. A missing key gets its default. Unknown
    /// keys are ignored.
    ///
    /// # Errors
    /// Returns a message that names the key, when a value has the wrong type
    /// or is not valid.
    pub fn from_options(options: &FilterOptions) -> Result<Self, String> {
        let _ = options;
        Ok(Self::default())
    }

    /// Write each option into a set of filter options.
    #[must_use]
    pub fn to_options(&self) -> FilterOptions {
        FilterOptions::new()
    }

    /// The background of the theme, where the fade starts.
    #[must_use]
    pub fn fade_from(&self) -> Rgb {
        Rgb::new(1, 1, 1)
    }
}

#[cfg(test)]
mod tests;
