//! The filter that converts the colors of a recording to the colors of a theme.
//!
//! The filter works at the level of events. It rewrites the SGR color
//! parameters in the `data` of the output events. Thus a recording made in one
//! terminal theme plays in the colors of another theme, in each renderer. Input
//! events stay as they are. The options, with their keys in the project file:
//!
//! - `theme`: necessary. It is one of `auto`, `dracula`, `monokai`,
//!   `solarized-dark`, and `solarized-light`. The filter takes the 16 colors,
//!   the default foreground, and the default background from this theme.
//! - `swap`: `true` or `false`. The default is `false`. When it is `true`, the
//!   default foreground and the default background of the theme change places.
//! - `cube`: `keep` (the default) or `nearest`. It sets what the filter does
//!   with a color of the 6x6x6 cube, the palette indexes 16 to 231. `nearest`
//!   gives the color of the 16 colors of the theme that is the nearest to it.
//! - `grayscale`: `keep` (the default) or `invert`. It sets what the filter
//!   does with a color of the grayscale ramp, the palette indexes 232 to 255.
//!   `invert` swaps the ends of the ramp, so 232 becomes 255.
//! - `truecolor`: `keep` (the default) or `invert-lightness`. It sets what the
//!   filter does with a 24-bit color. `invert-lightness` changes the lightness
//!   L of the color to 1 - L, and it keeps the hue and the saturation.
//!
//! The color rules:
//!
//! - A color of the 16 base colors, from `30` to `37`, `40` to `47`, `90` to
//!   `97`, `100` to `107`, and `38;5;n`, `48;5;n`, `58;5;n` with n below 16,
//!   becomes the 24-bit color of the same index in the theme.
//! - The cube, the grayscale ramp, and the 24-bit colors follow the three rules
//!   above.
//! - The filter writes each new color in the form `38;2;r;g;b` or `38;5;n`,
//!   with the codes 48 and 58 for the background and the underline. It always
//!   uses the semicolon form, because the emulator of `termstate` reads only
//!   that form.
//! - The filter reads the colon form (`38:5:n`, `38:2::r:g:b`) and rewrites it
//!   in the semicolon form.
//! - A parameter that is not a color stays as written. An incomplete color
//!   stays as written. A color with an operand that is not a number from 0 to
//!   255 stays as written.
//!
//! The filter does not read OSC 4, thus a palette that the recording sets with
//! OSC 4 is not converted. A terminal that shows bold text in the bright colors
//! does not do so for a base color that the filter converted, because the
//! converted color is a 24-bit color.

use serde_json::Value;
use termstate::TerminalTheme;

use super::splice::{SequenceEditor, Splicer, ST};
use crate::ansi::Sequence;
use crate::pipeline::{FilterRegistry, RegionView};
use crate::preview::screen::{hex, Rgb};
use crate::preview::PreviewTheme;
use crate::types::{AppliedFilter, Event, EventType, FilterKind, FilterOptions, FilterRegion};

/// The kind of the color scheme filter.
pub const KIND: &str = "color-scheme";

/// The option that holds the theme.
pub const THEME_OPTION: &str = "theme";

/// The option that swaps the default foreground and the default background.
pub const SWAP_OPTION: &str = "swap";

/// The option that holds the rule for the colors of the 6x6x6 cube.
pub const CUBE_OPTION: &str = "cube";

/// The option that holds the rule for the colors of the grayscale ramp.
pub const GRAYSCALE_OPTION: &str = "grayscale";

/// The option that holds the rule for the 24-bit colors.
pub const TRUECOLOR_OPTION: &str = "truecolor";

/// The name of the rule that leaves a color as it is.
const RULE_KEEP: &str = "keep";
/// The name of the cube rule that gives the nearest color of the theme.
const RULE_NEAREST: &str = "nearest";
/// The name of the grayscale rule that swaps the ends of the ramp.
const RULE_INVERT: &str = "invert";
/// The name of the truecolor rule that inverts the lightness.
const RULE_INVERT_LIGHTNESS: &str = "invert-lightness";

/// The first palette index of the 6x6x6 cube.
const CUBE_START: u8 = 16;
/// The first palette index of the grayscale ramp.
const GRAYSCALE_START: u8 = 232;
/// The sum of the first and the last index of the grayscale ramp.
const GRAYSCALE_MIRROR: u16 = 232 + 255;
/// The number of colors in a theme.
const THEME_COLORS: usize = 16;

/// What the filter does with a color of the 6x6x6 cube (palette indexes 16 to
/// 231).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CubeRule {
    /// Leave the color as it is.
    #[default]
    Keep,
    /// Give the nearest color of the 16 colors of the theme.
    Nearest,
}

impl CubeRule {
    /// Each rule.
    pub const ALL: [Self; 2] = [Self::Keep, Self::Nearest];

    /// The name of the rule in the options.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Keep => RULE_KEEP,
            Self::Nearest => RULE_NEAREST,
        }
    }

    /// The rule with this exact `name`, or `None`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|rule| rule.name() == name)
    }
}

/// What the filter does with a color of the grayscale ramp (palette indexes 232
/// to 255).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrayscaleRule {
    /// Leave the color as it is.
    #[default]
    Keep,
    /// Swap the ends of the ramp.
    Invert,
}

impl GrayscaleRule {
    /// Each rule.
    pub const ALL: [Self; 2] = [Self::Keep, Self::Invert];

    /// The name of the rule in the options.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Keep => RULE_KEEP,
            Self::Invert => RULE_INVERT,
        }
    }

    /// The rule with this exact `name`, or `None`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|rule| rule.name() == name)
    }
}

/// What the filter does with a 24-bit color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TruecolorRule {
    /// Leave the color as it is.
    #[default]
    Keep,
    /// Change the lightness L of the color to 1 - L.
    InvertLightness,
}

impl TruecolorRule {
    /// Each rule.
    pub const ALL: [Self; 2] = [Self::Keep, Self::InvertLightness];

    /// The name of the rule in the options.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Keep => RULE_KEEP,
            Self::InvertLightness => RULE_INVERT_LIGHTNESS,
        }
    }

    /// The rule with this exact `name`, or `None`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|rule| rule.name() == name)
    }
}

/// The options of the filter, after their checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorScheme {
    /// The theme that gives the 16 colors and the default colors.
    pub theme: PreviewTheme,
    /// True to swap the default foreground and the default background of the
    /// theme.
    pub swap: bool,
    /// The rule for the colors of the 6x6x6 cube.
    pub cube: CubeRule,
    /// The rule for the colors of the grayscale ramp.
    pub grayscale: GrayscaleRule,
    /// The rule for the 24-bit colors.
    pub truecolor: TruecolorRule,
}

impl ColorScheme {
    /// The scheme of `theme`, with no swap and each rule `Keep`.
    #[must_use]
    pub fn new(theme: PreviewTheme) -> Self {
        Self {
            theme,
            swap: false,
            cube: CubeRule::Keep,
            grayscale: GrayscaleRule::Keep,
            truecolor: TruecolorRule::Keep,
        }
    }

    /// Read and check the options of a filter.
    ///
    /// # Errors
    /// Returns a message when the `theme` is missing or unknown, when a key is
    /// unknown, or when a value has the wrong type or is not one of the values
    /// of its option.
    pub fn parse(options: &FilterOptions) -> Result<Self, String> {
        let value = serde_json::to_value(options).map_err(|error| error.to_string())?;
        let Value::Object(map) = value else {
            return Err("the options are not an object".to_string());
        };
        const KEYS: [&str; 5] = [
            THEME_OPTION,
            SWAP_OPTION,
            CUBE_OPTION,
            GRAYSCALE_OPTION,
            TRUECOLOR_OPTION,
        ];
        if let Some(key) = map.keys().find(|key| !KEYS.contains(&key.as_str())) {
            return Err(format!("the option \"{key}\" is not known"));
        }
        let theme_name = match map.get(THEME_OPTION) {
            None => return Err(format!("the option \"{THEME_OPTION}\" is missing")),
            Some(Value::String(name)) => name,
            Some(_) => return Err(format!("the option \"{THEME_OPTION}\" is not a string")),
        };
        let theme = PreviewTheme::from_name(theme_name).ok_or_else(|| {
            let names: Vec<&str> = PreviewTheme::ALL.iter().map(|theme| theme.name()).collect();
            format!(
                "the theme \"{theme_name}\" is not one of {}",
                names.join(", ")
            )
        })?;
        let swap = match map.get(SWAP_OPTION) {
            None => false,
            Some(Value::Bool(swap)) => *swap,
            Some(_) => return Err(format!("the option \"{SWAP_OPTION}\" is not a boolean")),
        };
        Ok(Self {
            theme,
            swap,
            cube: read_rule(
                &map,
                CUBE_OPTION,
                CubeRule::from_name,
                &CubeRule::ALL.map(CubeRule::name),
            )?,
            grayscale: read_rule(
                &map,
                GRAYSCALE_OPTION,
                GrayscaleRule::from_name,
                &GrayscaleRule::ALL.map(GrayscaleRule::name),
            )?,
            truecolor: read_rule(
                &map,
                TRUECOLOR_OPTION,
                TruecolorRule::from_name,
                &TruecolorRule::ALL.map(TruecolorRule::name),
            )?,
        })
    }

    /// The options of the filter: all five keys, always.
    #[must_use]
    pub fn options(&self) -> FilterOptions {
        FilterOptions::new()
            .with(THEME_OPTION, self.theme.name())
            .with(SWAP_OPTION, self.swap)
            .with(CUBE_OPTION, self.cube.name())
            .with(GRAYSCALE_OPTION, self.grayscale.name())
            .with(TRUECOLOR_OPTION, self.truecolor.name())
    }

    /// The default foreground that the filter sets: the foreground of the
    /// theme, or its background when `swap` is true.
    #[must_use]
    pub fn foreground(&self) -> Rgb {
        let theme = self.theme.palette();
        if self.swap {
            theme.background
        } else {
            theme.foreground
        }
    }

    /// The default background that the filter sets: the background of the
    /// theme, or its foreground when `swap` is true.
    #[must_use]
    pub fn background(&self) -> Rgb {
        let theme = self.theme.palette();
        if self.swap {
            theme.foreground
        } else {
            theme.background
        }
    }
}

/// The value of the rule option `key`, or the default rule when the option is
/// not set.
fn read_rule<T: Default>(
    map: &serde_json::Map<String, Value>,
    key: &str,
    from_name: fn(&str) -> Option<T>,
    names: &[&str],
) -> Result<T, String> {
    match map.get(key) {
        None => Ok(T::default()),
        Some(Value::String(name)) => from_name(name)
            .ok_or_else(|| format!("the {key} \"{name}\" is not one of {}", names.join(", "))),
        Some(_) => Err(format!("the option \"{key}\" is not a string")),
    }
}

/// The kind of the color scheme filter, as a `FilterKind`.
#[must_use]
pub fn kind() -> FilterKind {
    FilterKind::new(KIND)
}

/// A color scheme filter for `region` with `scheme`.
#[must_use]
pub fn applied(region: FilterRegion, scheme: &ColorScheme) -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region,
        options: scheme.options(),
    }
}

/// The scheme of a color scheme filter. It is `None` when the filter is of
/// another kind, or when its options do not parse.
#[must_use]
pub fn scheme_of(filter: &AppliedFilter) -> Option<ColorScheme> {
    if filter.kind != kind() {
        return None;
    }
    ColorScheme::parse(&filter.options).ok()
}

/// The target of an SGR color: the codes 38, 48, and 58 set it, and 39, 49,
/// and 59 set it back to its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Foreground,
    Background,
    Underline,
}

impl Slot {
    /// The code that sets a color of this slot.
    fn code(self) -> u8 {
        match self {
            Self::Foreground => 38,
            Self::Background => 48,
            Self::Underline => 58,
        }
    }
}

/// A color that an SGR parameter names: a palette index 0 to 255, or a 24-bit
/// color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SgrColor {
    Index(u8),
    Rgb(Rgb),
}

/// One item of an SGR parameter list, with its text as written.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SgrItem {
    /// An empty parameter or 0: the reset of all attributes. It holds its text.
    Reset(String),
    /// 39, 49, or 59: a slot back to its default. It holds its text.
    Default(Slot, String),
    /// A color for a slot. `text` is its parameters as written, for example
    /// `31`, `38;5;196`, or `38:2::1:2:3`.
    Color {
        slot: Slot,
        text: String,
        color: SgrColor,
    },
    /// Any other parameter, and a color form that is not complete or not
    /// valid, as written.
    Other(String),
}

impl SgrItem {
    /// The parameters of the item, as written.
    fn text(&self) -> &str {
        match self {
            Self::Reset(text) | Self::Default(_, text) | Self::Other(text) => text,
            Self::Color { text, .. } => text,
        }
    }
}

/// The color of `palette` that has the smallest squared distance to `color` in
/// RGB. A tie goes to the lower index.
fn nearest(palette: &[Rgb; THEME_COLORS], color: Rgb) -> Rgb {
    let distance = |other: Rgb| -> i32 {
        let channel = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        channel(other.0, color.0) + channel(other.1, color.1) + channel(other.2, color.2)
    };
    let mut best = color;
    let mut best_distance = i32::MAX;
    for candidate in palette {
        let candidate_distance = distance(*candidate);
        if candidate_distance < best_distance {
            best = *candidate;
            best_distance = candidate_distance;
        }
    }
    best
}

/// The color with the same hue and saturation as `color`, and the lightness
/// 1 - L. Each channel c becomes `c + 255 - (max + min)`.
fn invert_lightness(color: Rgb) -> Rgb {
    let channels = [color.0, color.1, color.2];
    let max = channels.into_iter().max().unwrap_or(0);
    let min = channels.into_iter().min().unwrap_or(0);
    let mirror = u16::from(max) + u16::from(min);
    let invert = |channel: u8| {
        let value = (u16::from(channel) + u16::from(u8::MAX)).saturating_sub(mirror);
        u8::try_from(value).unwrap_or(u8::MAX)
    };
    (invert(color.0), invert(color.1), invert(color.2))
}

/// The index of the grayscale ramp that mirrors `index`: 232 and 255 swap.
fn invert_gray(index: u8) -> u8 {
    u8::try_from(GRAYSCALE_MIRROR.saturating_sub(u16::from(index))).unwrap_or(index)
}

/// The value of a parameter that is a plain number: only ASCII digits, or
/// nothing, which is 0. It is `None` for any other text, and for a number that
/// does not fit in `u32`.
fn plain_number(text: &str) -> Option<u32> {
    if text.is_empty() {
        Some(0)
    } else if text.bytes().all(|byte| byte.is_ascii_digit()) {
        text.parse().ok()
    } else {
        None
    }
}

/// The value of an operand of a color: a plain number from 0 to 255.
fn operand(text: &str) -> Option<u8> {
    plain_number(text).and_then(|number| u8::try_from(number).ok())
}

/// The slot of an extended color code: 38, 48, or 58.
fn extended_slot(code: u32) -> Option<Slot> {
    match code {
        38 => Some(Slot::Foreground),
        48 => Some(Slot::Background),
        58 => Some(Slot::Underline),
        _ => None,
    }
}

/// The color of the operands of an extended color, after its mode: one index
/// for the mode 5, or three channels for the mode 2.
fn color_of_operands(mode: &str, operands: &[&str]) -> Option<SgrColor> {
    let values: Option<Vec<u8>> = operands.iter().map(|text| operand(text)).collect();
    match (mode, values?.as_slice()) {
        ("5", [index]) => Some(SgrColor::Index(*index)),
        ("2", [red, green, blue]) => Some(SgrColor::Rgb((*red, *green, *blue))),
        _ => None,
    }
}

/// The item of a parameter that holds a `:`.
fn colon_item(parameter: &str) -> SgrItem {
    let parts: Vec<&str> = parameter.split(':').collect();
    let color = match parts.as_slice() {
        [head, rest @ ..] => plain_number(head).and_then(extended_slot).and_then(|slot| {
            let color = match rest {
                ["5", index] => color_of_operands("5", &[index]),
                ["2", red, green, blue] | ["2", _, red, green, blue] => {
                    color_of_operands("2", &[red, green, blue])
                }
                _ => None,
            };
            color.map(|color| (slot, color))
        }),
        [] => None,
    };
    match color {
        Some((slot, color)) => SgrItem::Color {
            slot,
            text: parameter.to_string(),
            color,
        },
        None => SgrItem::Other(parameter.to_string()),
    }
}

/// The color of a parameter with no `:` and a value of the 16 base colors.
fn base_color(value: u32) -> Option<(Slot, u8)> {
    let (slot, offset) = match value {
        30..=37 => (Slot::Foreground, 30),
        40..=47 => (Slot::Background, 40),
        90..=97 => (Slot::Foreground, 82),
        100..=107 => (Slot::Background, 92),
        _ => return None,
    };
    u8::try_from(value - offset).ok().map(|index| (slot, index))
}

/// The items of a list of SGR parameters, read from left to right. The texts of
/// the items, joined with `;`, give back `params`.
fn sgr_items(params: &str) -> Vec<SgrItem> {
    let parts: Vec<&str> = params.split(';').collect();
    let mut items = Vec::new();
    let mut at = 0;
    while let Some(&parameter) = parts.get(at) {
        at += 1;
        if parameter.contains(':') {
            items.push(colon_item(parameter));
            continue;
        }
        let text = parameter.to_string();
        let Some(value) = plain_number(parameter) else {
            items.push(SgrItem::Other(text));
            continue;
        };
        if let Some((slot, index)) = base_color(value) {
            items.push(SgrItem::Color {
                slot,
                text,
                color: SgrColor::Index(index),
            });
            continue;
        }
        match value {
            0 => items.push(SgrItem::Reset(text)),
            39 => items.push(SgrItem::Default(Slot::Foreground, text)),
            49 => items.push(SgrItem::Default(Slot::Background, text)),
            59 => items.push(SgrItem::Default(Slot::Underline, text)),
            _ => {
                let Some(slot) = extended_slot(value) else {
                    items.push(SgrItem::Other(text));
                    continue;
                };
                let mode = parts.get(at).copied();
                let count = match mode {
                    Some("5") => 1,
                    Some("2") => 3,
                    _ => {
                        items.push(SgrItem::Other(text));
                        continue;
                    }
                };
                let Some(group) = parts.get(at - 1..at + 1 + count) else {
                    // The list ends before the operands end.
                    items.push(SgrItem::Other(
                        parts.get(at - 1..).unwrap_or_default().join(";"),
                    ));
                    at = parts.len();
                    continue;
                };
                at += 1 + count;
                let color = group
                    .get(1)
                    .zip(group.get(2..))
                    .and_then(|(mode, operands)| color_of_operands(mode, operands));
                let text = group.join(";");
                items.push(match color {
                    Some(color) => SgrItem::Color { slot, text, color },
                    None => SgrItem::Other(text),
                });
            }
        }
    }
    items
}

/// The colors of the filter: the 16 colors of the target theme and the three
/// rules.
struct Converter {
    palette: [Rgb; THEME_COLORS],
    cube: CubeRule,
    grayscale: GrayscaleRule,
    truecolor: TruecolorRule,
}

impl Converter {
    /// The converter of `scheme`.
    fn new(scheme: &ColorScheme) -> Self {
        let theme = scheme.theme.palette();
        Self {
            palette: std::array::from_fn(|index| theme.get_color(u8::try_from(index).unwrap_or(0))),
            cube: scheme.cube,
            grayscale: scheme.grayscale,
            truecolor: scheme.truecolor,
        }
    }

    /// The color after the rules.
    fn map(&self, color: SgrColor) -> SgrColor {
        match color {
            SgrColor::Index(index) if usize::from(index) < THEME_COLORS => SgrColor::Rgb(
                self.palette
                    .get(usize::from(index))
                    .copied()
                    .unwrap_or_default(),
            ),
            SgrColor::Index(index) if index < GRAYSCALE_START => match self.cube {
                CubeRule::Keep => color,
                CubeRule::Nearest => {
                    SgrColor::Rgb(nearest(&self.palette, TerminalTheme::get_256_color(index)))
                }
            },
            SgrColor::Index(index) => match self.grayscale {
                GrayscaleRule::Keep => color,
                GrayscaleRule::Invert => SgrColor::Index(invert_gray(index)),
            },
            SgrColor::Rgb(rgb) => match self.truecolor {
                TruecolorRule::Keep => color,
                TruecolorRule::InvertLightness => SgrColor::Rgb(invert_lightness(rgb)),
            },
        }
    }

    /// The replacement text of one color item, or `None` when the rules leave
    /// its color as it is.
    fn convert(&self, slot: Slot, color: SgrColor) -> Option<String> {
        let code = slot.code();
        match self.map(color) {
            converted if converted == color => None,
            SgrColor::Index(index) => Some(format!("{code};5;{index}")),
            SgrColor::Rgb((red, green, blue)) => Some(format!("{code};2;{red};{green};{blue}")),
        }
    }

    /// The parameters with each color converted, or `None` when no item
    /// changes.
    fn rewrite(&self, params: &str) -> Option<String> {
        let mut changed = false;
        let texts: Vec<String> = sgr_items(params)
            .into_iter()
            .map(|item| match &item {
                SgrItem::Color { slot, color, .. } => match self.convert(*slot, *color) {
                    Some(text) => {
                        changed = true;
                        text
                    }
                    None => item.text().to_string(),
                },
                _ => item.text().to_string(),
            })
            .collect();
        changed.then(|| texts.join(";"))
    }
}

/// Register the color scheme filter under `KIND`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_in_place(kind(), filter);
}

/// The color scheme filter.
///
/// It sets the default colors of the theme at the start of its region, and
/// then it reads the output events with one `Scanner`. Each SGR color is
/// rewritten in the event where its sequence completes. Each other byte
/// stays in its event.
fn filter(
    events: &[Event],
    region: RegionView,
    options: &FilterOptions,
) -> Result<Vec<Event>, String> {
    let scheme = ColorScheme::parse(options)?;
    // An event at the start of such a region sets nothing that a frame
    // shows.
    if events.is_empty() && (region.open || region.length <= 0.0) {
        return Ok(Vec::new());
    }
    let mut editor = SchemeEditor::new(&scheme);
    let start = editor.wanted();
    let mut splicer = Splicer::new();
    for event in events {
        match event.event_type {
            EventType::Input => splicer.pass(event),
            EventType::Output => {
                let index = splicer.open_output(event.time);
                splicer.read(index, &event.data, &mut editor);
            }
        }
    }
    Ok(splicer.finish(
        start,
        |open| {
            let mut text = if open { editor.wanted() } else { String::new() };
            if !region.open {
                text.push_str(&editor.restore());
            }
            text
        },
        region.length,
    ))
}

/// The color scheme that the filter follows while a `Splicer` rewrites the
/// events of one region.
struct SchemeEditor {
    converter: Converter,
    /// The default foreground that the filter sets.
    wanted_foreground: Rgb,
    /// The default background that the filter sets.
    wanted_background: Rgb,
    /// The default foreground that the answer set last, or `None` when it is
    /// not known.
    shown_foreground: Option<Rgb>,
    /// The default background that the answer set last, or `None` when it is
    /// not known.
    shown_background: Option<Rgb>,
}

impl SchemeEditor {
    fn new(scheme: &ColorScheme) -> Self {
        Self {
            converter: Converter::new(scheme),
            wanted_foreground: scheme.foreground(),
            wanted_background: scheme.background(),
            shown_foreground: None,
            shown_background: None,
        }
    }

    /// The OSC 10 and OSC 11 that bring the answer to the default colors of
    /// the theme, for each color that the answer does not show yet.
    fn wanted(&mut self) -> String {
        let mut text = String::new();
        if self.shown_foreground != Some(self.wanted_foreground) {
            text.push_str(&default_color_sequence(10, self.wanted_foreground));
            self.shown_foreground = Some(self.wanted_foreground);
        }
        if self.shown_background != Some(self.wanted_background) {
            text.push_str(&default_color_sequence(11, self.wanted_background));
            self.shown_background = Some(self.wanted_background);
        }
        text
    }

    /// Follow and drop the default colors that an OSC `text` sets. The stub
    /// keeps each OSC.
    fn osc(&mut self, _text: &str, _terminator: &str) -> Option<String> {
        None
    }

    /// The sequences for the end of a region that is not open.
    fn restore(&self) -> String {
        String::new()
    }
}

/// The OSC `number` (10 or 11) that sets a default color to `color`.
fn default_color_sequence(number: u8, color: Rgb) -> String {
    format!("\x1b]{number};{}{ST}", hex(color))
}

impl SequenceEditor for SchemeEditor {
    fn rewrite(&mut self, sequence: &Sequence) -> Option<String> {
        match sequence {
            Sequence::Csi {
                params,
                intermediates,
                final_char: 'm',
                controls,
            } if intermediates.is_empty() && !params.starts_with(|c| ('<'..='?').contains(&c)) => {
                let new = self.converter.rewrite(params)?;
                Some(format!("{controls}\x1b[{new}m"))
            }
            Sequence::Osc { text, terminator } => self.osc(text, terminator),
            _ => None,
        }
    }

    fn settle(&mut self) -> String {
        self.wanted()
    }
}

#[cfg(test)]
mod tests;
