//! Tests of the color scheme filter.

use serde_json::json;

use super::*;

/// The options that `value`, a JSON object, gives.
fn options(value: serde_json::Value) -> FilterOptions {
    serde_json::from_value(value).unwrap()
}

/// The message that `ColorScheme::parse` gives for `value`, or a panic.
fn refusal(value: serde_json::Value) -> String {
    match ColorScheme::parse(&options(value.clone())) {
        Err(message) => message,
        Ok(scheme) => panic!("{value}: expected a refusal, got {scheme:?}"),
    }
}

/// Each scheme that the options can state.
fn every_scheme() -> Vec<ColorScheme> {
    let mut schemes = Vec::new();
    for theme in PreviewTheme::ALL {
        for swap in [false, true] {
            for cube in CubeRule::ALL {
                for grayscale in GrayscaleRule::ALL {
                    for truecolor in TruecolorRule::ALL {
                        schemes.push(ColorScheme {
                            theme,
                            swap,
                            cube,
                            grayscale,
                            truecolor,
                        });
                    }
                }
            }
        }
    }
    schemes
}

// Tests of the options.

#[test]
fn the_options_of_each_scheme_parse_back_to_the_scheme() {
    for scheme in every_scheme() {
        assert_eq!(ColorScheme::parse(&scheme.options()), Ok(scheme));
    }
}

#[test]
fn the_options_always_hold_all_five_keys() {
    let value = serde_json::to_value(ColorScheme::new(PreviewTheme::Dracula).options()).unwrap();

    assert_eq!(
        value,
        json!({
            "theme": "dracula",
            "swap": false,
            "cube": "keep",
            "grayscale": "keep",
            "truecolor": "keep",
        })
    );
}

#[test]
fn a_theme_alone_gives_the_defaults() {
    let parsed = ColorScheme::parse(&options(json!({ "theme": "monokai" })));

    assert_eq!(parsed, Ok(ColorScheme::new(PreviewTheme::Monokai)));
    let scheme = parsed.unwrap();
    assert!(!scheme.swap);
    assert_eq!(scheme.cube, CubeRule::Keep);
    assert_eq!(scheme.grayscale, GrayscaleRule::Keep);
    assert_eq!(scheme.truecolor, TruecolorRule::Keep);
}

#[test]
fn an_unknown_theme_names_the_themes_that_exist() {
    assert_eq!(
        refusal(json!({ "theme": "nord" })),
        "the theme \"nord\" is not one of auto, dracula, monokai, solarized-dark, solarized-light"
    );
}

#[test]
fn a_missing_theme_is_refused() {
    assert!(refusal(json!({ "swap": true })).contains("theme"));
    assert!(refusal(json!({})).contains("theme"));
}

#[test]
fn an_unknown_key_is_refused() {
    let message = refusal(json!({ "theme": "auto", "colour": "x" }));

    assert!(message.contains("colour"), "{message}");
}

#[test]
fn an_unknown_rule_value_is_refused() {
    for (key, value) in [
        (CUBE_OPTION, "invert"),
        (GRAYSCALE_OPTION, "nearest"),
        (TRUECOLOR_OPTION, "invert"),
    ] {
        let message = refusal(json!({ "theme": "auto", key: value }));
        assert!(message.contains(key), "{message}");
        assert!(message.contains(value), "{message}");
    }
}

#[test]
fn a_value_of_the_wrong_type_is_refused() {
    assert!(!refusal(json!({ "theme": "auto", "swap": "yes" })).is_empty());
    assert!(!refusal(json!({ "theme": "auto", "cube": true })).is_empty());
    assert!(!refusal(json!({ "theme": 3 })).is_empty());
}

#[test]
fn each_rule_name_reads_back_to_its_rule() {
    for rule in CubeRule::ALL {
        assert_eq!(CubeRule::from_name(rule.name()), Some(rule));
    }
    for rule in GrayscaleRule::ALL {
        assert_eq!(GrayscaleRule::from_name(rule.name()), Some(rule));
    }
    for rule in TruecolorRule::ALL {
        assert_eq!(TruecolorRule::from_name(rule.name()), Some(rule));
    }
    assert_eq!(CubeRule::from_name("nothing"), None);
}

#[test]
fn the_default_colors_are_those_of_the_theme() {
    let theme = PreviewTheme::SolarizedLight;
    let scheme = ColorScheme::new(theme);

    assert_eq!(scheme.foreground(), (101, 123, 131));
    assert_eq!(scheme.background(), (253, 246, 227));
}

#[test]
fn a_swap_exchanges_the_default_colors() {
    let scheme = ColorScheme {
        swap: true,
        ..ColorScheme::new(PreviewTheme::SolarizedLight)
    };

    assert_eq!(scheme.foreground(), (253, 246, 227));
    assert_eq!(scheme.background(), (101, 123, 131));
}

#[test]
fn an_applied_filter_gives_its_scheme_back() {
    let scheme = ColorScheme {
        swap: true,
        cube: CubeRule::Nearest,
        ..ColorScheme::new(PreviewTheme::Dracula)
    };
    let filter = applied(FilterRegion::All, &scheme);

    assert_eq!(filter.kind, kind());
    assert_eq!(filter.kind, FilterKind::new(KIND));
    assert_eq!(filter.region, FilterRegion::All);
    assert_eq!(scheme_of(&filter), Some(scheme));
}

#[test]
fn a_filter_of_another_kind_has_no_scheme() {
    let filter = AppliedFilter {
        kind: FilterKind::new("cursor"),
        region: FilterRegion::All,
        options: ColorScheme::new(PreviewTheme::Auto).options(),
    };

    assert_eq!(scheme_of(&filter), None);
}

#[test]
fn a_filter_with_bad_options_has_no_scheme() {
    let filter = AppliedFilter {
        kind: kind(),
        region: FilterRegion::All,
        options: options(json!({ "theme": "nord" })),
    };

    assert_eq!(scheme_of(&filter), None);
}

// Tests of the color math.

/// The palette of Solarized Light.
fn solarized_light_palette() -> [Rgb; THEME_COLORS] {
    Converter::new(&ColorScheme::new(PreviewTheme::SolarizedLight)).palette
}

#[test]
fn the_nearest_color_is_the_palette_color_at_the_smallest_distance() {
    let palette = solarized_light_palette();

    assert_eq!(nearest(&palette, (255, 0, 0)), (220, 50, 47));
    assert_eq!(nearest(&palette, (38, 139, 210)), (38, 139, 210));
    assert_eq!(nearest(&palette, (0, 0, 0)), palette[0]);
}

#[test]
fn a_tie_in_the_nearest_color_goes_to_the_lower_index() {
    let mut palette = [(200, 200, 200); THEME_COLORS];
    palette[2] = (0, 0, 0);
    palette[5] = (10, 0, 0);

    assert_eq!(nearest(&palette, (5, 0, 0)), (0, 0, 0));
}

#[test]
fn inverting_the_lightness_of_white_and_black_swaps_them() {
    assert_eq!(invert_lightness((255, 255, 255)), (0, 0, 0));
    assert_eq!(invert_lightness((0, 0, 0)), (255, 255, 255));
}

#[test]
fn inverting_the_lightness_keeps_a_pure_hue() {
    assert_eq!(invert_lightness((255, 0, 0)), (255, 0, 0));
}

#[test]
fn inverting_the_lightness_of_a_dark_color_gives_a_light_one() {
    assert_eq!(invert_lightness((40, 42, 54)), (201, 203, 215));
}

#[test]
fn inverting_the_lightness_twice_gives_the_color_back() {
    for color in [(40, 42, 54), (1, 2, 3), (250, 100, 7), (128, 128, 128)] {
        assert_eq!(invert_lightness(invert_lightness(color)), color);
    }
}

#[test]
fn a_gray_index_mirrors_inside_the_ramp() {
    assert_eq!(invert_gray(232), 255);
    assert_eq!(invert_gray(255), 232);
    assert_eq!(invert_gray(243), 244);
    assert_eq!(invert_gray(244), 243);
}
