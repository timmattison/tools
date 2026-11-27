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
