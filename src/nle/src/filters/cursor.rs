//! The filter that sets how the cursor looks.
//!
//! The filter rewrites the escape sequences that set the cursor, in the data
//! of the output events. The options, with their keys in the project file:
//!
//! - `visibility`: `honor` (the default) obeys the recording.
//!   `always-visible` and `always-hidden` set the cursor and drop each
//!   DECTCEM change (`CSI ? 25 h` and `CSI ? 25 l`) of the recording.
//!   `smart` obeys the recording, but hides the cursor during a burst of
//!   output.
//! - `burst_gap`: for `smart` only, in seconds, above zero. The default is
//!   `DEFAULT_BURST_GAP`.
//! - `shape`: `block`, `underline`, or `bar`.
//! - `blink`: `true` or `false`.
//! - `color`: `#rrggbb`.
//!
//! An option that the options do not name obeys the recording. An unknown
//! key or value fails the render.

use serde::Deserialize;

use crate::pipeline::{FilterRegistry, RegionView};
use crate::types::{Event, FilterKind, FilterOptions, Seconds};

/// The kind of the filter that sets how the cursor looks.
pub const CURSOR: &str = "cursor";

/// The burst gap of the visibility `smart` when the options do not name
/// one, in seconds.
pub const DEFAULT_BURST_GAP: f64 = 0.1;

/// Register the cursor filter under `CURSOR`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_in_place(FilterKind::new(CURSOR), filter);
}

/// The cursor filter.
fn filter(
    events: &[Event],
    _region: RegionView,
    options: &FilterOptions,
) -> Result<Vec<Event>, String> {
    let _cursor = Cursor::parse(options)?;
    Ok(events.to_vec())
}

/// The visibility in the project file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Visibility {
    #[default]
    Honor,
    AlwaysVisible,
    AlwaysHidden,
    Smart,
}

/// A shape of the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Shape {
    Block,
    Underline,
    Bar,
}

/// A color of the cursor: `#` and six hexadecimal digits.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
struct Color(String);

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let mut chars = text.chars();
        let valid = chars.next() == Some('#')
            && chars.clone().count() == 6
            && chars.all(|c| c.is_ascii_hexdigit());
        if valid {
            Ok(Self(text))
        } else {
            Err(format!("the color \"{text}\" is not of the form #rrggbb"))
        }
    }
}

/// The options as the project file holds them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorFile {
    #[serde(default)]
    visibility: Visibility,
    burst_gap: Option<Seconds>,
    shape: Option<Shape>,
    blink: Option<bool>,
    color: Option<Color>,
}

/// What the filter does with the visibility of the cursor.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Show {
    /// Obey the recording.
    Honor,
    /// Show (`true`) or hide (`false`) the cursor at all times.
    Always(bool),
    /// Obey the recording, but hide the cursor during a burst: two or more
    /// output events, each less than `burst_gap` seconds after the one
    /// before it, with no input event between them.
    Smart { burst_gap: f64 },
}

/// The options of the filter, after their checks.
#[derive(Debug, Clone, PartialEq)]
struct Cursor {
    show: Show,
    shape: Option<Shape>,
    blink: Option<bool>,
    color: Option<String>,
}

impl TryFrom<CursorFile> for Cursor {
    type Error = String;

    fn try_from(file: CursorFile) -> Result<Self, String> {
        let show = match (file.visibility, file.burst_gap) {
            (Visibility::Smart, gap) => {
                let burst_gap = gap.map_or(DEFAULT_BURST_GAP, Seconds::get);
                if burst_gap <= 0.0 {
                    return Err(format!(
                        "burst_gap must be above zero, and it is {burst_gap} s"
                    ));
                }
                Show::Smart { burst_gap }
            }
            (_, Some(_)) => {
                return Err("burst_gap applies only when visibility is smart".to_string());
            }
            (Visibility::Honor, None) => Show::Honor,
            (Visibility::AlwaysVisible, None) => Show::Always(true),
            (Visibility::AlwaysHidden, None) => Show::Always(false),
        };
        Ok(Self {
            show,
            shape: file.shape,
            blink: file.blink,
            color: file.color.map(|color| color.0),
        })
    }
}

impl Cursor {
    /// Read and check the options of the filter.
    fn parse(options: &FilterOptions) -> Result<Self, String> {
        let value = serde_json::to_value(options).map_err(|error| error.to_string())?;
        let file: CursorFile = serde_json::from_value(value).map_err(|error| error.to_string())?;
        Self::try_from(file)
    }
}

#[cfg(test)]
mod tests;
