//! Tests of the options of the keystroke HUD.

use super::{
    HudOptions, Position, Rgb, ACCENT_OPTION, BACKGROUND_OPTION, BORDER_OPTION, DURATION_OPTION,
    FADE_OPTION, POSITION_OPTION, TEXT_OPTION, THEME_OPTION,
};
use crate::filters::keystroke_hud::{applied, kind, KIND};
use crate::types::{FilterOptions, FilterRegion};

fn read(options: &FilterOptions) -> Result<HudOptions, String> {
    HudOptions::from_options(options)
}

#[test]
fn empty_options_give_the_defaults() {
    let options = read(&FilterOptions::new()).unwrap();
    assert_eq!(options, HudOptions::default());
    assert_eq!(options.position, Position::TopRight);
    assert!((options.duration - 1.5).abs() < 1e-12);
    assert!((options.fade - 0.2).abs() < 1e-12);
    assert_eq!(options.theme, "auto");
    assert_eq!(options.text, Rgb::new(255, 255, 255));
    assert_eq!(options.accent, Rgb::new(255, 200, 60));
    assert_eq!(options.background, Rgb::new(48, 48, 48));
    assert_eq!(options.border, Rgb::new(128, 128, 128));
}

#[test]
fn unknown_keys_are_ignored() {
    let options = FilterOptions::new().with("colour", "red");
    assert_eq!(read(&options).unwrap(), HudOptions::default());
}

#[test]
fn to_options_then_from_options_round_trips() {
    let options = HudOptions {
        position: Position::BottomCenter,
        duration: 3.25,
        fade: 0.0,
        theme: "solarized-light".to_string(),
        text: Rgb::new(1, 2, 3),
        accent: Rgb::new(4, 5, 6),
        background: Rgb::new(7, 8, 9),
        border: Rgb::new(250, 251, 252),
    };
    assert_eq!(read(&options.to_options()).unwrap(), options);
}

#[test]
fn to_options_writes_the_json_types() {
    let options = HudOptions::default().to_options();
    assert_eq!(options.get(POSITION_OPTION).unwrap(), "top-right");
    assert_eq!(options.get(DURATION_OPTION).unwrap().as_f64(), Some(1.5));
    assert_eq!(options.get(FADE_OPTION).unwrap().as_f64(), Some(0.2));
    assert_eq!(options.get(THEME_OPTION).unwrap(), "auto");
    assert_eq!(options.get(TEXT_OPTION).unwrap(), "#ffffff");
    assert_eq!(options.get(ACCENT_OPTION).unwrap(), "#ffc83c");
    assert_eq!(options.get(BACKGROUND_OPTION).unwrap(), "#303030");
    assert_eq!(options.get(BORDER_OPTION).unwrap(), "#808080");
}

const POSITION_ERROR: &str = "the option \"position\" must be one of top-left, top-center, \
     top-right, bottom-left, bottom-center, bottom-right";
const DURATION_ERROR: &str = "the option \"duration\" must be a number of seconds above zero";
const FADE_ERROR: &str = "the option \"fade\" must be a number of seconds, zero or more";
const THEME_ERROR: &str = "the option \"theme\" must be one of auto, black, dracula, monokai, \
     solarized-dark, solarized-light";

fn color_error(key: &str) -> String {
    format!("the option \"{key}\" must be a color in the form \"#rrggbb\"")
}

#[test]
fn position_errors() {
    for value in [
        serde_json::json!(3),
        serde_json::json!("middle"),
        serde_json::json!("TOP-LEFT"),
    ] {
        let options = FilterOptions::new().with(POSITION_OPTION, value);
        assert_eq!(read(&options).unwrap_err(), POSITION_ERROR);
    }
}

#[test]
fn duration_errors() {
    for value in [
        serde_json::json!("1"),
        serde_json::json!(0),
        serde_json::json!(-1.5),
        serde_json::json!(null),
    ] {
        let options = FilterOptions::new().with(DURATION_OPTION, value);
        assert_eq!(read(&options).unwrap_err(), DURATION_ERROR);
    }
}

#[test]
fn fade_errors() {
    for value in [
        serde_json::json!("0.2"),
        serde_json::json!(-0.1),
        serde_json::json!(true),
    ] {
        let options = FilterOptions::new().with(FADE_OPTION, value);
        assert_eq!(read(&options).unwrap_err(), FADE_ERROR);
    }
}

#[test]
fn zero_fade_is_valid() {
    let options = FilterOptions::new().with(FADE_OPTION, 0.0);
    assert!(read(&options).unwrap().fade.abs() < 1e-12);
}

#[test]
fn theme_errors() {
    for value in [serde_json::json!(5), serde_json::json!("neon")] {
        let options = FilterOptions::new().with(THEME_OPTION, value);
        assert_eq!(read(&options).unwrap_err(), THEME_ERROR);
    }
}

#[test]
fn theme_names_compare_in_lower_case() {
    let options = FilterOptions::new().with(THEME_OPTION, "Dracula");
    assert!(read(&options).is_ok());
}

#[test]
fn color_errors() {
    for key in [TEXT_OPTION, ACCENT_OPTION, BACKGROUND_OPTION, BORDER_OPTION] {
        for value in [serde_json::json!(7), serde_json::json!("ffc83c")] {
            let options = FilterOptions::new().with(key, value);
            assert_eq!(read(&options).unwrap_err(), color_error(key));
        }
    }
}

#[test]
fn rgb_parse_accepts_either_case() {
    assert_eq!(Rgb::parse("#FFc83C"), Some(Rgb::new(255, 200, 60)));
}

#[test]
fn rgb_parse_refuses_bad_text() {
    for text in ["ffc83c", "#ffc83", "#ffc83cc", "#gggggg", "#ffé83c", ""] {
        assert_eq!(Rgb::parse(text), None, "{text}");
    }
}

#[test]
fn rgb_to_hex_writes_lower_case() {
    assert_eq!(Rgb::new(255, 200, 60).to_hex(), "#ffc83c");
    assert_eq!(Rgb::new(0, 10, 171).to_hex(), "#000aab");
}

#[test]
fn position_names_agree_with_the_list() {
    for position in Position::ALL {
        assert_eq!(Position::from_name(position.as_str()), Some(position));
    }
    assert_eq!(Position::TopCenter.as_str(), "top-center");
    assert_eq!(Position::BottomRight.as_str(), "bottom-right");
    assert_eq!(Position::from_name("nowhere"), None);
}

#[test]
fn fade_from_is_the_background_of_the_theme() {
    let dracula = HudOptions {
        theme: "dracula".to_string(),
        ..HudOptions::default()
    };
    assert_eq!(dracula.fade_from(), Rgb::new(40, 42, 54));
    assert_eq!(HudOptions::default().fade_from(), Rgb::new(0, 0, 0));
}

#[test]
fn applied_holds_the_kind_the_region_and_the_options() {
    let options = HudOptions {
        position: Position::BottomLeft,
        ..HudOptions::default()
    };
    let filter = applied(FilterRegion::All, &options);
    assert_eq!(KIND, "keystroke-hud");
    assert_eq!(filter.kind, kind());
    assert_eq!(filter.region, FilterRegion::All);
    assert_eq!(filter.options, options.to_options());
    assert_eq!(filter.options.get(POSITION_OPTION).unwrap(), "bottom-left");
}
