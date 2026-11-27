//! Tests of the color scheme filter.

use serde_json::json;

use super::*;
use crate::error::NleError;
use crate::types::{EventType, Seconds};

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
    assert_eq!(nearest(&palette, (0, 0, 0)), palette[8]);
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

// Tests of the SGR rewrite.

/// A converter for Solarized Light with the rules that a test states.
fn converter(cube: CubeRule, grayscale: GrayscaleRule, truecolor: TruecolorRule) -> Converter {
    Converter::new(&ColorScheme {
        cube,
        grayscale,
        truecolor,
        ..ColorScheme::new(PreviewTheme::SolarizedLight)
    })
}

/// The rewrite of `params` with each rule at `Keep`.
fn rewrite(params: &str) -> Option<String> {
    converter(CubeRule::Keep, GrayscaleRule::Keep, TruecolorRule::Keep).rewrite(params)
}

/// The parameters that the texts of the items of `params` give.
fn joined(params: &str) -> String {
    sgr_items(params)
        .iter()
        .map(SgrItem::text)
        .collect::<Vec<_>>()
        .join(";")
}

/// The text of a 24-bit color of the code `code`.
fn rgb_text(code: u8, (red, green, blue): Rgb) -> String {
    format!("{code};2;{red};{green};{blue}")
}

#[test]
fn a_base_color_becomes_the_color_of_the_theme() {
    assert_eq!(
        rewrite("1;31;44").as_deref(),
        Some("1;38;2;220;50;47;48;2;38;139;210")
    );
}

#[test]
fn a_bright_color_uses_the_bright_half_of_the_palette() {
    let palette = solarized_light_palette();

    assert_eq!(rewrite("91"), Some(rgb_text(38, palette[9])));
    assert_eq!(rewrite("104"), Some(rgb_text(48, palette[12])));
    assert_eq!(rewrite("97"), Some(rgb_text(38, palette[15])));
    assert_eq!(rewrite("100"), Some(rgb_text(48, palette[8])));
}

#[test]
fn an_indexed_base_color_becomes_a_24_bit_color() {
    let palette = solarized_light_palette();

    assert_eq!(rewrite("38;5;1"), Some(rgb_text(38, palette[1])));
    assert_eq!(rewrite("48:5:4"), Some(rgb_text(48, palette[4])));
    assert_eq!(rewrite("58;5;1"), Some(rgb_text(58, palette[1])));
}

#[test]
fn a_list_with_no_color_to_change_gives_none() {
    for params in [
        "",
        "0",
        "1;4",
        "39;49;59",
        "38;5;196",
        "38;5;240",
        "38;2;1;2;3",
        "4:3",
    ] {
        assert_eq!(rewrite(params), None, "{params:?}");
    }
}

#[test]
fn a_cube_operand_is_not_read_as_a_color_code() {
    assert_eq!(rewrite("38;5;31"), None);
    assert_eq!(
        rewrite("1;38;5;31;32").as_deref(),
        Some(format!("1;38;5;31;{}", rgb_text(38, solarized_light_palette()[2])).as_str())
    );
    assert_eq!(
        rewrite("38;5;0;31")
            .as_deref()
            .map(|text| text.starts_with("38;2;")),
        Some(true)
    );
}

#[test]
fn an_incomplete_color_stays_as_written() {
    assert_eq!(rewrite("38;5"), None);
    assert_eq!(rewrite("38;2;1;2"), None);
    assert_eq!(rewrite("1;48;2;1"), None);
    assert_eq!(
        sgr_items("38;2;1;2"),
        vec![SgrItem::Other("38;2;1;2".to_string())]
    );
}

#[test]
fn a_color_with_a_bad_operand_stays_as_written() {
    assert_eq!(rewrite("38;2;300;0;0"), None);
    assert_eq!(rewrite("38;5;256"), None);
    assert_eq!(rewrite("38;5;1:2"), None);
    assert_eq!(
        sgr_items("38;2;300;0;0;1"),
        vec![
            SgrItem::Other("38;2;300;0;0".to_string()),
            SgrItem::Other("1".to_string()),
        ]
    );
}

#[test]
fn a_bad_group_still_takes_its_operands() {
    // The 0 is an operand of the group, not a reset.
    assert_eq!(
        sgr_items("38;5;999;0"),
        vec![
            SgrItem::Other("38;5;999".to_string()),
            SgrItem::Reset("0".to_string()),
        ]
    );
    assert_eq!(
        sgr_items("38;5;256"),
        vec![SgrItem::Other("38;5;256".to_string())]
    );
}

#[test]
fn an_extended_code_with_no_mode_is_one_other_item() {
    assert_eq!(
        sgr_items("38;7;1"),
        vec![
            SgrItem::Other("38".to_string()),
            SgrItem::Other("7".to_string()),
            SgrItem::Other("1".to_string()),
        ]
    );
    assert_eq!(sgr_items("38"), vec![SgrItem::Other("38".to_string())]);
}

#[test]
fn the_colon_form_with_an_empty_color_space_is_read() {
    let converter = converter(
        CubeRule::Keep,
        GrayscaleRule::Keep,
        TruecolorRule::InvertLightness,
    );

    assert_eq!(
        sgr_items("38:2::10:20:30"),
        vec![SgrItem::Color {
            slot: Slot::Foreground,
            text: "38:2::10:20:30".to_string(),
            color: SgrColor::Rgb((10, 20, 30)),
        }]
    );
    assert_eq!(
        converter.rewrite("38:2::10:20:30").as_deref(),
        Some("38;2;225;235;245")
    );
    assert_eq!(
        converter.rewrite("48:2:0:10:20:30").as_deref(),
        Some("48;2;225;235;245")
    );
    assert_eq!(
        converter.rewrite("38:2:10:20:30").as_deref(),
        Some("38;2;225;235;245")
    );
}

#[test]
fn a_colon_form_that_is_not_a_color_stays_as_written() {
    assert_eq!(rewrite("38:5"), None);
    assert_eq!(rewrite("38:2:1:2"), None);
    assert_eq!(rewrite("38:3:1:2:3"), None);
    assert_eq!(rewrite("4:3;5"), None);
}

#[test]
fn the_cube_rule_nearest_gives_the_nearest_color_of_the_theme() {
    let converter = converter(CubeRule::Nearest, GrayscaleRule::Keep, TruecolorRule::Keep);

    assert_eq!(
        converter.rewrite("38;5;196").as_deref(),
        Some("38;2;220;50;47")
    );
    assert_eq!(converter.rewrite("38;5;240"), None);
}

#[test]
fn the_grayscale_rule_invert_mirrors_the_ramp() {
    let converter = converter(CubeRule::Keep, GrayscaleRule::Invert, TruecolorRule::Keep);

    assert_eq!(converter.rewrite("38;5;232").as_deref(), Some("38;5;255"));
    assert_eq!(converter.rewrite("48;5;243").as_deref(), Some("48;5;244"));
    assert_eq!(converter.rewrite("38;5;196"), None);
}

#[test]
fn the_truecolor_rule_leaves_a_color_that_does_not_change() {
    let converter = converter(
        CubeRule::Keep,
        GrayscaleRule::Keep,
        TruecolorRule::InvertLightness,
    );

    assert_eq!(converter.rewrite("38;2;255;0;0"), None);
    assert_eq!(
        converter.rewrite("38;2;255;255;255").as_deref(),
        Some("38;2;0;0;0")
    );
}

#[test]
fn a_reset_and_a_default_keep_their_text() {
    assert_eq!(sgr_items("0"), vec![SgrItem::Reset("0".to_string())]);
    assert_eq!(sgr_items(""), vec![SgrItem::Reset(String::new())]);
    assert_eq!(
        sgr_items("39;49;59"),
        vec![
            SgrItem::Default(Slot::Foreground, "39".to_string()),
            SgrItem::Default(Slot::Background, "49".to_string()),
            SgrItem::Default(Slot::Underline, "59".to_string()),
        ]
    );
}

#[test]
fn a_number_that_does_not_fit_stays_as_written() {
    assert_eq!(
        sgr_items("99999999999;1"),
        vec![
            SgrItem::Other("99999999999".to_string()),
            SgrItem::Other("1".to_string()),
        ]
    );
}

#[test]
fn the_texts_of_the_items_give_back_the_parameters() {
    for params in [
        "",
        "0",
        ";",
        "1;31;44",
        "91;104",
        "38;5;1",
        "48:5:4",
        "58;5;1",
        "38;5;196",
        "38;2;1;2;3",
        "38;2;1;2",
        "38;5",
        "38",
        "38;2;300;0;0;1",
        "38;5;999;0",
        "38;7;1",
        "38:2::10:20:30",
        "38:2:1:2",
        "4:3;5",
        "39;49;59",
        "99999999999",
        "1;;3",
        "00;038;5;1",
        "x;y",
    ] {
        assert_eq!(joined(params), params, "{params:?}");
    }
}

// Tests of the stream.

/// The OSC 10 and OSC 11 that set Solarized Light.
const SOLARIZED_LIGHT_START: &str = "\x1b]10;#657b83\x1b\\\x1b]11;#fdf6e3\x1b\\";

/// The SGR that sets the Solarized Light red as the foreground.
const LIGHT_RED: &str = "\x1b[38;2;220;50;47m";

/// An output event.
fn out(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Output,
        data: data.to_string(),
    }
}

/// An input event.
fn input(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Input,
        data: data.to_string(),
    }
}

fn range(start: f64, end: f64) -> FilterRegion {
    FilterRegion::Range {
        start: Seconds::new(start).unwrap(),
        end: Seconds::new(end).unwrap(),
    }
}

/// Run the color scheme filter with `options` on `region` of `events`.
fn run(
    events: Vec<Event>,
    options: serde_json::Value,
    region: FilterRegion,
) -> Result<Vec<Event>, NleError> {
    let mut registry = FilterRegistry::new();
    register(&mut registry);
    registry.run(
        events,
        &[AppliedFilter {
            kind: kind(),
            region,
            options: self::options(options),
        }],
    )
}

/// The events of `run` with the Solarized Light theme, as (time, data).
fn light(events: Vec<Event>, region: FilterRegion) -> Vec<(f64, String)> {
    run(events, json!({ "theme": "solarized-light" }), region)
        .unwrap()
        .into_iter()
        .map(|event| (event.time, event.data))
        .collect()
}

#[test]
fn the_answer_starts_with_the_default_colors_of_the_theme() {
    let answer = light(vec![out(1.0, "hi")], FilterRegion::All);

    assert_eq!(
        answer,
        vec![
            (0.0, SOLARIZED_LIGHT_START.to_string()),
            (1.0, "hi".to_string())
        ]
    );
}

#[test]
fn an_sgr_color_becomes_the_color_of_the_theme_in_the_same_event() {
    let answer = light(vec![out(1.0, "\x1b[31mred")], FilterRegion::All);

    assert_eq!(answer.get(1), Some(&(1.0, format!("{LIGHT_RED}red"))));
}

#[test]
fn text_other_sequences_and_input_events_stay() {
    let events = vec![
        out(1.0, "a\x1b[1;4mb\x1b[2J\x1b[?25l"),
        input(1.5, "\x1b[31m"),
        out(2.0, "c"),
    ];

    let answer = light(events, FilterRegion::All);

    assert_eq!(
        answer,
        vec![
            (0.0, SOLARIZED_LIGHT_START.to_string()),
            (1.0, "a\x1b[1;4mb\x1b[2J\x1b[?25l".to_string()),
            (1.5, "\x1b[31m".to_string()),
            (2.0, "c".to_string()),
        ]
    );
}

#[test]
fn an_sgr_split_over_two_events_gives_its_replacement_in_the_second() {
    let answer = light(
        vec![out(1.0, "ab\x1b[3"), out(2.0, "1mx")],
        FilterRegion::All,
    );

    assert_eq!(
        answer,
        vec![
            (0.0, SOLARIZED_LIGHT_START.to_string()),
            (1.0, "ab".to_string()),
            (2.0, format!("{LIGHT_RED}x")),
        ]
    );
}

#[test]
fn swap_writes_the_background_in_osc_10_and_the_foreground_in_osc_11() {
    let answer = run(
        vec![out(1.0, "hi")],
        json!({ "theme": "solarized-light", "swap": true }),
        FilterRegion::All,
    )
    .unwrap();

    assert_eq!(
        answer.first().map(|event| event.data.as_str()),
        Some("\x1b]10;#fdf6e3\x1b\\\x1b]11;#657b83\x1b\\")
    );
}

#[test]
fn an_empty_region_of_all_gives_no_event() {
    assert_eq!(light(Vec::new(), FilterRegion::All), vec![]);
}

#[test]
fn bad_options_fail_the_render_with_the_kind_in_the_message() {
    let result = run(
        vec![out(0.0, "hi")],
        json!({ "theme": "no-such-theme" }),
        FilterRegion::All,
    );

    match result {
        Err(NleError::RenderError(message)) => assert!(message.contains("color-scheme")),
        other => panic!("expected a RenderError, got {other:?}"),
    }
}

// Tests of the default colors of the recording.

/// The data of the one output event of the recording that `data` holds, after
/// the filter with Solarized Light on `FilterRegion::All`. The start event is
/// not in the answer.
fn filtered(data: &str) -> String {
    let answer = light(vec![out(1.0, data)], FilterRegion::All);
    assert_eq!(answer.len(), 2, "{answer:?}");
    answer.get(1).map(|event| event.1.clone()).unwrap()
}

#[test]
fn an_osc_10_or_11_that_sets_a_color_drops() {
    assert_eq!(filtered("a\x1b]10;#123456\x07b"), "ab");
    assert_eq!(filtered("a\x1b]11;rgb:12/34/56\x1b\\b"), "ab");
}

#[test]
fn the_default_color_values_of_a_list_drop_and_the_others_stay() {
    assert_eq!(
        filtered("\x1b]10;#111111;#222222;#333333\x07"),
        "\x1b]12;#333333\x07"
    );
    assert_eq!(filtered("\x1b]10;?;#222222\x07"), "\x1b]10;?\x07");
}

#[test]
fn a_query_of_a_default_color_stays() {
    assert_eq!(filtered("\x1b]10;?\x07"), "\x1b]10;?\x07");
    assert_eq!(filtered("\x1b]11;?\x1b\\"), "\x1b]11;?\x1b\\");
}

#[test]
fn an_osc_110_or_111_drops() {
    assert_eq!(filtered("a\x1b]110\x07b\x1b]111\x1b\\c"), "abc");
}

#[test]
fn the_other_osc_sequences_stay() {
    assert_eq!(
        filtered("\x1b]0;title\x07\x1b]12;#ff0000\x07"),
        "\x1b]0;title\x07\x1b]12;#ff0000\x07"
    );
}

#[test]
fn a_full_reset_sets_the_colors_of_the_theme_again_right_after_it() {
    assert_eq!(
        filtered("a\x1bcb"),
        format!("a\x1bc{SOLARIZED_LIGHT_START}b")
    );
}

// Tests of the end of a range.

/// The OSC 110 and OSC 111 that give the default colors back.
const RESET_COLORS: &str = "\x1b]110\x1b\\\x1b]111\x1b\\";

/// The answer for `events` and the range 0 s to 2 s, with the Solarized Light
/// theme and `extra` options.
fn restored(events: Vec<Event>, extra: serde_json::Value) -> Vec<(f64, String)> {
    let mut value = json!({ "theme": "solarized-light" });
    if let (Some(map), Some(more)) = (value.as_object_mut(), extra.as_object()) {
        map.extend(more.clone());
    }
    run(events, value, range(0.0, 2.0))
        .unwrap()
        .into_iter()
        .map(|event| (event.time, event.data))
        .collect()
}

#[test]
fn the_end_of_a_range_gives_the_colors_of_the_recording_back() {
    let answer = restored(vec![out(0.0, "\x1b[31mA"), out(5.0, "B")], json!({}));

    assert_eq!(
        answer,
        vec![
            (0.0, SOLARIZED_LIGHT_START.to_string()),
            (0.0, format!("{LIGHT_RED}A")),
            (2.0, format!("{RESET_COLORS}\x1b[31m")),
            (5.0, "B".to_string()),
        ]
    );
}

#[test]
fn the_restore_sets_the_default_colors_that_the_recording_set() {
    let answer = restored(
        vec![out(0.0, "\x1b]10;#123456\x07A"), out(5.0, "B")],
        json!({}),
    );

    assert_eq!(
        answer.get(2),
        Some(&(
            2.0,
            "\x1b]110\x1b\\\x1b]10;#123456\x1b\\\x1b]111\x1b\\".to_string()
        ))
    );
}

#[test]
fn the_restore_sets_the_background_that_the_recording_set() {
    let answer = restored(
        vec![out(0.0, "\x1b]11;#abcdef\x07A"), out(5.0, "B")],
        json!({}),
    );

    assert_eq!(
        answer.get(2),
        Some(&(
            2.0,
            "\x1b]110\x1b\\\x1b]111\x1b\\\x1b]11;#abcdef\x1b\\".to_string()
        ))
    );
}

#[test]
fn a_slot_that_the_recording_reset_or_set_with_a_kept_color_gets_no_sgr() {
    let reset = restored(vec![out(0.0, "\x1b[31mA\x1b[0m"), out(5.0, "B")], json!({}));
    let kept = restored(
        vec![out(0.0, "\x1b[38;5;196mA"), out(5.0, "B")],
        json!({ "cube": "keep" }),
    );

    assert_eq!(reset.get(2), Some(&(2.0, RESET_COLORS.to_string())));
    assert_eq!(kept.get(2), Some(&(2.0, RESET_COLORS.to_string())));
}

#[test]
fn the_restore_sets_the_foreground_background_and_underline_in_one_sgr() {
    let answer = restored(
        vec![out(0.0, "\x1b[58;5;1m\x1b[44m\x1b[31mA"), out(5.0, "B")],
        json!({}),
    );

    assert_eq!(
        answer.get(2),
        Some(&(2.0, format!("{RESET_COLORS}\x1b[31;44;58;5;1m")))
    );
}

#[test]
fn a_range_that_reaches_the_end_of_the_recording_gives_no_restore() {
    let answer = light(vec![out(0.0, "\x1b[31mA")], FilterRegion::All);
    assert_eq!(answer.len(), 2);

    let open = run(
        vec![out(0.0, "\x1b[31mA")],
        json!({ "theme": "solarized-light" }),
        range(0.0, 2.0),
    )
    .unwrap();
    assert_eq!(open.len(), 2);
}

#[test]
fn a_sequence_open_at_the_end_of_a_range_gets_the_restore_before_it() {
    let answer = restored(vec![out(0.0, "A\x1b[3"), out(5.0, "B")], json!({}));

    assert_eq!(
        answer.get(1),
        Some(&(0.0, format!("A{RESET_COLORS}\x1b[3")))
    );
}
