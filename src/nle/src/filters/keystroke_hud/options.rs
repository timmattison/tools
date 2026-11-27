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

/// The number of hex digits in a color.
const HEX_LENGTH: usize = 6;
/// The default time that the box stays, in seconds.
const DEFAULT_DURATION: f64 = 1.5;
/// The default time of the fade, in seconds.
const DEFAULT_FADE: f64 = 0.2;
/// The default theme.
const DEFAULT_THEME: &str = "auto";
/// The default color of typed text.
const DEFAULT_TEXT: Rgb = Rgb::new(255, 255, 255);
/// The default color of the labels of special keys.
const DEFAULT_ACCENT: Rgb = Rgb::new(255, 200, 60);
/// The default color of the inside of the box.
const DEFAULT_BACKGROUND: Rgb = Rgb::new(48, 48, 48);
/// The default color of the border of the box.
const DEFAULT_BORDER: Rgb = Rgb::new(128, 128, 128);

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
        match self {
            Position::TopLeft => "top-left",
            Position::TopCenter => "top-center",
            Position::TopRight => "top-right",
            Position::BottomLeft => "bottom-left",
            Position::BottomCenter => "bottom-center",
            Position::BottomRight => "bottom-right",
        }
    }

    /// The position that has `name`, or `None`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|position| position.as_str() == name)
    }

    /// The names of all positions, joined with a comma and a space.
    fn names() -> String {
        Self::ALL.map(Position::as_str).join(", ")
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
        let digits = text.strip_prefix('#')?;
        if digits.len() != HEX_LENGTH || !digits.is_ascii() {
            return None;
        }
        let channel = |index: usize| {
            let start = index * 2;
            digits
                .get(start..start + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        };
        // `from_str_radix` accepts a leading "+", so check each digit first.
        if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        Some(Self::new(channel(0)?, channel(1)?, channel(2)?))
    }

    /// The color in the form `#rrggbb`, with lower-case hex digits.
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.red, self.green, self.blue)
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
        Self {
            position: Position::default(),
            duration: DEFAULT_DURATION,
            fade: DEFAULT_FADE,
            theme: DEFAULT_THEME.to_string(),
            text: DEFAULT_TEXT,
            accent: DEFAULT_ACCENT,
            background: DEFAULT_BACKGROUND,
            border: DEFAULT_BORDER,
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
        let default = Self::default();
        let position = match options.get(POSITION_OPTION) {
            None => default.position,
            Some(value) => value
                .as_str()
                .and_then(Position::from_name)
                .ok_or_else(|| {
                    format!(
                        "the option \"{POSITION_OPTION}\" must be one of {}",
                        Position::names()
                    )
                })?,
        };
        let duration = match options.get(DURATION_OPTION) {
            None => default.duration,
            Some(value) => value
                .as_f64()
                .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
                .ok_or_else(|| {
                    format!(
                        "the option \"{DURATION_OPTION}\" must be a number of seconds above zero"
                    )
                })?,
        };
        let fade = match options.get(FADE_OPTION) {
            None => default.fade,
            Some(value) => value
                .as_f64()
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                .ok_or_else(|| {
                    format!(
                        "the option \"{FADE_OPTION}\" must be a number of seconds, zero or more"
                    )
                })?,
        };
        let theme = match options.get(THEME_OPTION) {
            None => default.theme,
            Some(value) => value
                .as_str()
                .map(str::to_lowercase)
                .filter(|name| THEMES.contains(&name.as_str()))
                .ok_or_else(|| {
                    format!(
                        "the option \"{THEME_OPTION}\" must be one of {}",
                        THEMES.join(", ")
                    )
                })?,
        };
        Ok(Self {
            position,
            duration,
            fade,
            theme,
            text: color(options, TEXT_OPTION, default.text)?,
            accent: color(options, ACCENT_OPTION, default.accent)?,
            background: color(options, BACKGROUND_OPTION, default.background)?,
            border: color(options, BORDER_OPTION, default.border)?,
        })
    }

    /// Write each option into a set of filter options.
    #[must_use]
    pub fn to_options(&self) -> FilterOptions {
        FilterOptions::new()
            .with(POSITION_OPTION, self.position.as_str())
            .with(DURATION_OPTION, self.duration)
            .with(FADE_OPTION, self.fade)
            .with(THEME_OPTION, self.theme.as_str())
            .with(TEXT_OPTION, self.text.to_hex())
            .with(ACCENT_OPTION, self.accent.to_hex())
            .with(BACKGROUND_OPTION, self.background.to_hex())
            .with(BORDER_OPTION, self.border.to_hex())
    }

    /// The background of the theme, where the fade starts.
    #[must_use]
    pub fn fade_from(&self) -> Rgb {
        let (red, green, blue) = termstate::TerminalTheme::from_name(&self.theme).background;
        Rgb::new(red, green, blue)
    }
}

/// Read the color option `key`, or give `default` when the option is missing.
fn color(options: &FilterOptions, key: &str, default: Rgb) -> Result<Rgb, String> {
    match options.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_str()
            .and_then(Rgb::parse)
            .ok_or_else(|| format!("the option \"{key}\" must be a color in the form \"#rrggbb\"")),
    }
}

#[cfg(test)]
mod tests;
