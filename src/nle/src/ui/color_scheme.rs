//! The color scheme panel of the properties panel.
//!
//! The panel holds two fields for a region, five menus for a color scheme, the
//! button that applies them, and one row for each color scheme filter of the
//! project. It is the same kind of panel as the speed panel.

use dioxus::prelude::*;

use super::{add_filter, filter_rows, range_from, region_text, Document, END_LABEL, START_LABEL};
use crate::filters::color_scheme::{self, ColorScheme, CubeRule, GrayscaleRule, TruecolorRule};
use crate::preview::PreviewTheme;
use crate::types::FilterRegion;

/// The heading of the color scheme panel.
const COLOR_SCHEME_HEADING: &str = "Color Scheme";
/// The hint of the color scheme panel.
const COLOR_SCHEME_HINT: &str = "Times are in seconds, on the timeline after the filters above. Leave both times empty for the whole timeline.";
/// The label of the menu for the target theme.
const THEME_LABEL: &str = "Target theme";
/// The label of the menu for the default colors.
const DEFAULT_COLORS_LABEL: &str = "Default colors";
/// The label of the menu for the color cube.
const CUBE_LABEL: &str = "Color cube (16-231)";
/// The label of the menu for the grayscale ramp.
const GRAYSCALE_LABEL: &str = "Grayscale (232-255)";
/// The label of the menu for the 24-bit colors.
const TRUECOLOR_LABEL: &str = "Truecolor";
/// The text of the button that applies the color scheme.
const APPLY_COLOR_SCHEME_LABEL: &str = "Apply Color Scheme";
/// The start of the message for a color scheme that does not apply.
const COLOR_SCHEME_FAILED_PREFIX: &str = "Failed to change the color scheme";

/// The value of the option that keeps the default colors of the theme.
const DEFAULT_COLORS_THEME: &str = "theme";
/// The value of the option that swaps the default colors.
const DEFAULT_COLORS_SWAPPED: &str = "swapped";
/// The text of the option that keeps the default colors of the theme.
const DEFAULT_COLORS_THEME_TEXT: &str = "As in the theme";
/// The text of the option that swaps the default colors.
const DEFAULT_COLORS_SWAPPED_TEXT: &str = "Swapped";
/// The text of the option that keeps a color.
const KEEP_TEXT: &str = "Keep";
/// The text of the option that gives the nearest theme color.
const NEAREST_TEXT: &str = "Nearest theme color";
/// The text of the option that inverts the grayscale ramp.
const INVERT_TEXT: &str = "Invert";
/// The text of the option that inverts the lightness of a 24-bit color.
const INVERT_LIGHTNESS_TEXT: &str = "Invert lightness";

/// The text that a row adds for swapped default colors.
const SWAPPED_SUFFIX: &str = ", swapped colors";
/// The text that a row adds for the nearest cube colors.
const NEAREST_SUFFIX: &str = ", nearest cube colors";
/// The text that a row adds for an inverted grayscale ramp.
const INVERT_SUFFIX: &str = ", inverted grayscale";
/// The text that a row adds for inverted lightness of 24-bit colors.
const INVERT_LIGHTNESS_SUFFIX: &str = ", inverted truecolor lightness";

/// The text of an option of the cube menu.
fn cube_text(rule: CubeRule) -> &'static str {
    match rule {
        CubeRule::Keep => KEEP_TEXT,
        CubeRule::Nearest => NEAREST_TEXT,
    }
}

/// The text of an option of the grayscale menu.
fn grayscale_text(rule: GrayscaleRule) -> &'static str {
    match rule {
        GrayscaleRule::Keep => KEEP_TEXT,
        GrayscaleRule::Invert => INVERT_TEXT,
    }
}

/// The text of an option of the truecolor menu.
fn truecolor_text(rule: TruecolorRule) -> &'static str {
    match rule {
        TruecolorRule::Keep => KEEP_TEXT,
        TruecolorRule::InvertLightness => INVERT_LIGHTNESS_TEXT,
    }
}

/// The text of a row for a color scheme filter: the region, the theme, and
/// one phrase for each option that is not the default.
fn scheme_text(region: &FilterRegion, scheme: &ColorScheme) -> String {
    let mut text = format!("{}: {}", region_text(region), scheme.theme.label());
    if scheme.swap {
        text.push_str(SWAPPED_SUFFIX);
    }
    if scheme.cube != CubeRule::default() {
        text.push_str(NEAREST_SUFFIX);
    }
    if scheme.grayscale != GrayscaleRule::default() {
        text.push_str(INVERT_SUFFIX);
    }
    if scheme.truecolor != TruecolorRule::default() {
        text.push_str(INVERT_LIGHTNESS_SUFFIX);
    }
    text
}

/// One row for each color scheme filter of the project: the index of the
/// filter in the project, and the text of its row. Filters of other kinds have
/// no row, but they keep their place in the index.
fn scheme_rows(document: &Option<Document>) -> Vec<(usize, String)> {
    let Some(open) = document else {
        return Vec::new();
    };
    open.editor
        .project()
        .filters
        .iter()
        .enumerate()
        .filter_map(|(index, filter)| {
            let scheme = color_scheme::scheme_of(filter)?;
            Some((index, scheme_text(&filter.region, &scheme)))
        })
        .collect()
}

/// The region of the two fields: the whole timeline when both are empty, else
/// the range that the fields state.
fn region_from_fields(start: &str, end: &str) -> Result<FilterRegion, String> {
    if start.trim().is_empty() && end.trim().is_empty() {
        Ok(FilterRegion::All)
    } else {
        range_from(start, end)
    }
}

/// The color scheme panel: two fields for a region, five menus for the scheme,
/// the button that applies them, and the color scheme filters of the project.
/// `on_result` gets the outcome of each Apply Color Scheme and each Remove.
#[component]
pub(super) fn ColorSchemePanel(
    loaded: Signal<Option<Document>>,
    on_result: EventHandler<Result<(), String>>,
) -> Element {
    let mut start = use_signal(String::new);
    let mut end = use_signal(String::new);
    let mut scheme = use_signal(|| ColorScheme::new(PreviewTheme::default()));
    let rows = scheme_rows(&loaded.read());
    let current = scheme();
    rsx! {
        div { class: "color-scheme-panel",
            h3 { {COLOR_SCHEME_HEADING} }
            p { class: "color-scheme-hint", {COLOR_SCHEME_HINT} }
            label { class: "color-scheme-field",
                {START_LABEL}
                input {
                    r#type: "number",
                    step: "any",
                    min: "0",
                    value: "{start}",
                    oninput: move |event| start.set(event.value()),
                }
            }
            label { class: "color-scheme-field",
                {END_LABEL}
                input {
                    r#type: "number",
                    step: "any",
                    min: "0",
                    value: "{end}",
                    oninput: move |event| end.set(event.value()),
                }
            }
            label { class: "color-scheme-field",
                {THEME_LABEL}
                select {
                    onchange: move |event| {
                        if let Some(chosen) = PreviewTheme::from_name(&event.value()) {
                            scheme.write().theme = chosen;
                        }
                    },
                    for choice in PreviewTheme::ALL {
                        option {
                            key: "{choice.name()}",
                            value: choice.name(),
                            selected: choice == current.theme,
                            {choice.label()}
                        }
                    }
                }
            }
            label { class: "color-scheme-field",
                {DEFAULT_COLORS_LABEL}
                select {
                    onchange: move |event| match event.value().as_str() {
                        DEFAULT_COLORS_THEME => scheme.write().swap = false,
                        DEFAULT_COLORS_SWAPPED => scheme.write().swap = true,
                        _ => {}
                    },
                    option {
                        value: DEFAULT_COLORS_THEME,
                        selected: !current.swap,
                        {DEFAULT_COLORS_THEME_TEXT}
                    }
                    option {
                        value: DEFAULT_COLORS_SWAPPED,
                        selected: current.swap,
                        {DEFAULT_COLORS_SWAPPED_TEXT}
                    }
                }
            }
            label { class: "color-scheme-field",
                {CUBE_LABEL}
                select {
                    onchange: move |event| {
                        if let Some(chosen) = CubeRule::from_name(&event.value()) {
                            scheme.write().cube = chosen;
                        }
                    },
                    for rule in CubeRule::ALL {
                        option {
                            key: "{rule.name()}",
                            value: rule.name(),
                            selected: rule == current.cube,
                            {cube_text(rule)}
                        }
                    }
                }
            }
            label { class: "color-scheme-field",
                {GRAYSCALE_LABEL}
                select {
                    onchange: move |event| {
                        if let Some(chosen) = GrayscaleRule::from_name(&event.value()) {
                            scheme.write().grayscale = chosen;
                        }
                    },
                    for rule in GrayscaleRule::ALL {
                        option {
                            key: "{rule.name()}",
                            value: rule.name(),
                            selected: rule == current.grayscale,
                            {grayscale_text(rule)}
                        }
                    }
                }
            }
            label { class: "color-scheme-field",
                {TRUECOLOR_LABEL}
                select {
                    onchange: move |event| {
                        if let Some(chosen) = TruecolorRule::from_name(&event.value()) {
                            scheme.write().truecolor = chosen;
                        }
                    },
                    for rule in TruecolorRule::ALL {
                        option {
                            key: "{rule.name()}",
                            value: rule.name(),
                            selected: rule == current.truecolor,
                            {truecolor_text(rule)}
                        }
                    }
                }
            }
            button {
                onclick: move |_| {
                    let change = region_from_fields(&start.read(), &end.read())
                        .map(|region| color_scheme::applied(region, &scheme.read()));
                    add_filter(loaded, on_result, COLOR_SCHEME_FAILED_PREFIX, change);
                },
                {APPLY_COLOR_SCHEME_LABEL}
            }
            {filter_rows(loaded, on_result, "color-scheme-row", COLOR_SCHEME_FAILED_PREFIX, rows)}
        }
    }
}
