//! Tests of the layout of the timeline.

use super::*;

/// The largest difference between two floats that the tests accept.
const TOLERANCE: f64 = 1e-9;

/// Fails the test when `actual` is not within `TOLERANCE` of `expected`.
fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= TOLERANCE,
        "{actual} is not {expected}"
    );
}

/// A project of 60 seconds in a window of 1,200 pixels. The whole project
/// fits at 20 pixels per second.
fn minute() -> Extent {
    Extent::new(60.0, 1200.0)
}

#[test]
fn the_default_view_shows_the_whole_project() {
    let frame = View::default().frame(minute());
    assert_close(frame.pixels_per_second(), 20.0);
    assert_close(frame.start(), 0.0);
    assert_close(frame.end(), 60.0);
    assert_close(frame.width(), 1200.0);
}

#[test]
fn a_short_project_stops_at_the_largest_zoom() {
    let frame = View::default().frame(Extent::new(0.5, 1200.0));
    assert_close(frame.pixels_per_second(), MAX_PIXELS_PER_SECOND);
    assert_close(frame.start(), 0.0);
}

#[test]
fn an_empty_project_shows_the_largest_zoom() {
    let frame = View::default().frame(Extent::new(0.0, 1200.0));
    assert_close(frame.pixels_per_second(), MAX_PIXELS_PER_SECOND);
    assert_close(frame.start(), 0.0);
}

#[test]
fn a_bad_extent_gives_a_zoom_above_zero() {
    for (duration, width) in [
        (60.0, 0.0),
        (60.0, -5.0),
        (60.0, f64::NAN),
        (f64::NAN, 1200.0),
        (-3.0, 1200.0),
        (f64::INFINITY, 1200.0),
    ] {
        let frame = View::default().frame(Extent::new(duration, width));
        let zoom = frame.pixels_per_second();
        assert!(zoom.is_finite() && zoom > 0.0, "{duration} {width}: {zoom}");
        assert!(frame.start().is_finite(), "{duration} {width}");
    }
}

#[test]
fn the_map_from_time_to_pixels_goes_both_ways() {
    let view = View::default()
        .zoom(minute(), 4.0, 0.0)
        .pan(minute(), 400.0);
    let frame = view.frame(minute());
    assert_close(frame.pixels_per_second(), 80.0);
    assert_close(frame.start(), 5.0);
    assert_close(frame.x_of(5.0), 0.0);
    assert_close(frame.x_of(10.0), 400.0);
    assert_close(frame.time_at(400.0), 10.0);
    assert_close(frame.end(), 20.0);
}

#[test]
fn a_zoom_keeps_the_time_under_the_anchor() {
    let start = View::default().zoom(minute(), 3.0, 600.0);
    let before = start.frame(minute()).time_at(250.0);
    let zoomed = start.zoom(minute(), ZOOM_STEP, 250.0);
    let frame = zoomed.frame(minute());
    assert_close(frame.pixels_per_second(), 60.0 * ZOOM_STEP);
    assert_close(frame.time_at(250.0), before);
}

#[test]
fn a_zoom_out_keeps_the_time_under_the_anchor() {
    let start = View::default()
        .zoom(minute(), 4.0, 0.0)
        .pan(minute(), 1600.0);
    let before = start.frame(minute()).time_at(900.0);
    let frame = start.zoom(minute(), 1.0 / 2.0, 900.0).frame(minute());
    assert_close(frame.pixels_per_second(), 40.0);
    assert_close(frame.time_at(900.0), before);
}

#[test]
fn a_zoom_stops_at_the_largest_zoom() {
    let frame = View::default().zoom(minute(), 1e9, 600.0).frame(minute());
    assert_close(frame.pixels_per_second(), MAX_PIXELS_PER_SECOND);
}

#[test]
fn a_zoom_out_past_the_whole_project_shows_the_whole_project() {
    let view = View::default()
        .zoom(minute(), 4.0, 600.0)
        .zoom(minute(), 1e-9, 600.0);
    assert_eq!(view, View::default());
}

#[test]
fn a_zoom_near_the_end_keeps_the_window_inside_the_project() {
    let frame = View::default().zoom(minute(), 2.0, 1200.0).frame(minute());
    assert_close(frame.pixels_per_second(), 40.0);
    assert_close(frame.end(), 60.0);
    assert_close(frame.start(), 30.0);
}

#[test]
fn a_bad_zoom_factor_changes_nothing() {
    let view = View::default().zoom(minute(), 2.0, 600.0);
    for factor in [0.0, -2.0, f64::NAN, f64::INFINITY] {
        assert_eq!(view.zoom(minute(), factor, 600.0), view, "{factor}");
    }
}

#[test]
fn a_bad_anchor_is_the_center_of_the_window() {
    let from_center = View::default().zoom(minute(), 2.0, 600.0);
    assert_eq!(View::default().zoom(minute(), 2.0, f64::NAN), from_center);
}

#[test]
fn the_whole_project_follows_a_new_width_of_the_window() {
    let frame = View::default().frame(Extent::new(60.0, 600.0));
    assert_close(frame.pixels_per_second(), 10.0);
    assert_close(frame.end(), 60.0);
}

#[test]
fn a_zoom_stays_when_the_window_grows_until_it_shows_the_whole_project() {
    let view = View::default().zoom(minute(), 2.0, 0.0);
    assert_close(
        view.frame(Extent::new(60.0, 1800.0)).pixels_per_second(),
        40.0,
    );
    assert_close(
        view.frame(Extent::new(60.0, 3000.0)).pixels_per_second(),
        50.0,
    );
}

#[test]
fn a_pan_moves_the_window_by_the_pixels() {
    let view = View::default().zoom(minute(), 4.0, 0.0);
    let frame = view.pan(minute(), 160.0).frame(minute());
    assert_close(frame.start(), 2.0);
    let back = view
        .pan(minute(), 160.0)
        .pan(minute(), -80.0)
        .frame(minute());
    assert_close(back.start(), 1.0);
}

#[test]
fn a_pan_stops_at_the_start_and_at_the_end() {
    let view = View::default().zoom(minute(), 4.0, 0.0);
    assert_close(view.pan(minute(), -500.0).frame(minute()).start(), 0.0);
    let end = view.pan(minute(), 1e9).frame(minute());
    assert_close(end.start(), 45.0);
    assert_close(end.end(), 60.0);
}

#[test]
fn a_pan_of_the_whole_project_changes_nothing() {
    assert_eq!(View::default().pan(minute(), 300.0), View::default());
}

#[test]
fn a_bad_pan_changes_nothing() {
    let view = View::default()
        .zoom(minute(), 4.0, 0.0)
        .pan(minute(), 100.0);
    for pixels in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(view.pan(minute(), pixels), view, "{pixels}");
    }
}

#[test]
fn a_shorter_project_pulls_the_window_back_inside_it() {
    let view = View::default().zoom(minute(), 4.0, 0.0).pan(minute(), 1e9);
    let frame = view.frame(Extent::new(30.0, 1200.0));
    assert_close(frame.start(), 15.0);
    assert_close(frame.end(), 30.0);
}

/// A wheel event of `x` and `y` pixels with no modifier.
fn wheel(x: f64, y: f64) -> Wheel {
    Wheel {
        x,
        y,
        unit: WheelUnit::Pixels,
        command: false,
        shift: false,
    }
}

/// The factor of a zoom action, or a failure for another action.
fn zoom_factor(action: WheelAction) -> f64 {
    match action {
        WheelAction::Zoom(factor) => factor,
        other => panic!("{other:?} is not a zoom"),
    }
}

#[test]
fn a_horizontal_wheel_scrolls() {
    assert_eq!(
        wheel_action(wheel(30.0, 0.0), 1200.0),
        WheelAction::Pan(30.0)
    );
    assert_eq!(
        wheel_action(wheel(-12.0, 4.0), 1200.0),
        WheelAction::Pan(-12.0)
    );
}

#[test]
fn a_vertical_wheel_with_no_modifier_does_nothing() {
    assert_eq!(wheel_action(wheel(0.0, 40.0), 1200.0), WheelAction::Nothing);
}

#[test]
fn a_vertical_wheel_with_shift_scrolls() {
    let event = Wheel {
        shift: true,
        ..wheel(0.0, 25.0)
    };
    assert_eq!(wheel_action(event, 1200.0), WheelAction::Pan(25.0));
}

#[test]
fn a_wheel_with_command_zooms_at_the_rate_of_remotion() {
    let up = Wheel {
        command: true,
        ..wheel(0.0, -5.0)
    };
    assert_close(zoom_factor(wheel_action(up, 1200.0)), (0.3_f64).exp());
    let down = Wheel {
        command: true,
        ..wheel(3.0, 5.0)
    };
    assert_close(zoom_factor(wheel_action(down, 1200.0)), (-0.3_f64).exp());
}

#[test]
fn one_wheel_event_zooms_by_at_most_a_factor_of_two() {
    let up = Wheel {
        command: true,
        ..wheel(0.0, -100.0)
    };
    assert_close(zoom_factor(wheel_action(up, 1200.0)), 2.0);
    let down = Wheel {
        command: true,
        ..wheel(0.0, 100.0)
    };
    assert_close(zoom_factor(wheel_action(down, 1200.0)), 0.5);
}

#[test]
fn a_wheel_with_command_and_no_vertical_delta_does_nothing() {
    let event = Wheel {
        command: true,
        ..wheel(20.0, 0.0)
    };
    assert_eq!(wheel_action(event, 1200.0), WheelAction::Nothing);
}

#[test]
fn a_wheel_in_lines_or_pages_counts_in_pixels() {
    let lines = Wheel {
        unit: WheelUnit::Lines,
        ..wheel(2.0, 0.0)
    };
    assert_eq!(wheel_action(lines, 1200.0), WheelAction::Pan(32.0));
    let pages = Wheel {
        unit: WheelUnit::Pages,
        ..wheel(0.5, 0.0)
    };
    assert_eq!(wheel_action(pages, 1200.0), WheelAction::Pan(600.0));
}

#[test]
fn a_wheel_with_a_bad_delta_does_nothing() {
    for (x, y) in [(f64::NAN, 0.0), (f64::INFINITY, 0.0), (0.0, f64::NAN)] {
        for command in [false, true] {
            let event = Wheel {
                command,
                shift: true,
                ..wheel(x, y)
            };
            assert_eq!(
                wheel_action(event, 1200.0),
                WheelAction::Nothing,
                "{x} {y} {command}"
            );
        }
    }
}

#[test]
fn the_zoom_keys_are_the_keys_of_remotion() {
    for (key, action) in [
        ("+", KeyAction::ZoomIn),
        ("=", KeyAction::ZoomIn),
        ("-", KeyAction::ZoomOut),
        ("_", KeyAction::ZoomOut),
        ("0", KeyAction::Fit),
    ] {
        assert_eq!(key_action(key, false), Some(action), "{key}");
    }
}

#[test]
fn other_keys_and_keys_with_a_modifier_do_nothing() {
    for key in ["+", "-", "0"] {
        assert_eq!(key_action(key, true), None, "{key}");
    }
    for key in ["a", "1", " ", "ArrowLeft", "日"] {
        assert_eq!(key_action(key, false), None, "{key}");
    }
}

/// The labels of the ruler of `frame`, with their places.
fn labels(frame: Frame) -> Vec<(f64, String)> {
    ruler_ticks(frame)
        .into_iter()
        .filter_map(|tick| tick.label.map(|label| (tick.x, label)))
        .collect()
}

/// The texts of the labels of the ruler of `frame`.
fn label_texts(frame: Frame) -> Vec<String> {
    labels(frame).into_iter().map(|(_, label)| label).collect()
}

#[test]
fn the_ruler_of_a_minute_has_a_label_each_five_seconds_and_a_tick_each_second() {
    let frame = View::default().frame(minute());
    let ticks = ruler_ticks(frame);
    assert_eq!(ticks.len(), 61, "{ticks:?}");
    let expected: Vec<String> = (0..=12)
        .map(|index| format!("{}:{:02}", index * 5 / 60, index * 5 % 60))
        .collect();
    assert_eq!(label_texts(frame), expected);
    let (x, label) = &labels(frame)[1];
    assert_eq!(label, "0:05");
    assert_close(*x, 100.0);
    for pair in ticks.windows(2) {
        assert_close(pair[1].x - pair[0].x, 20.0);
    }
}

#[test]
fn the_ruler_at_the_largest_zoom_shows_tenths() {
    let frame = View::default().zoom(minute(), 1e9, 0.0).frame(minute());
    let texts = label_texts(frame);
    assert_eq!(texts.first().map(String::as_str), Some("0:00.0"));
    assert_eq!(texts.get(3).map(String::as_str), Some("0:00.3"));
    assert_eq!(texts.last().map(String::as_str), Some("0:01.2"));
    assert_eq!(texts.len(), 13, "{texts:?}");
    assert_eq!(ruler_ticks(frame).len(), 61);
}

#[test]
fn the_ruler_of_ten_hours_has_a_label_each_hour() {
    let frame = View::default().frame(Extent::new(36_000.0, 1200.0));
    let texts = label_texts(frame);
    assert_eq!(texts.len(), 11, "{texts:?}");
    assert_eq!(texts[1], "1:00:00");
    assert_eq!(texts[10], "10:00:00");
}

#[test]
fn the_ruler_past_the_last_step_doubles_the_step() {
    let frame = View::default().frame(Extent::new(360_000.0, 1200.0));
    let texts = label_texts(frame);
    assert_eq!(
        texts.get(1).map(String::as_str),
        Some("8:00:00"),
        "{texts:?}"
    );
}

#[test]
fn the_ruler_of_a_scrolled_window_starts_at_the_left_edge() {
    let frame = View::default()
        .zoom(minute(), 4.0, 0.0)
        .pan(minute(), 200.0)
        .frame(minute());
    let ticks = ruler_ticks(frame);
    assert_close(ticks[0].x, 8.0);
    assert_eq!(ticks[0].label, None);
    let (x, label) = &labels(frame)[0];
    assert_eq!(label, "0:03");
    assert_close(*x, 40.0);
    assert!(ticks.iter().all(|tick| (0.0..=1200.0).contains(&tick.x)));
}

#[test]
fn a_ruler_never_holds_more_ticks_than_the_window_has_room_for() {
    for extent in [
        Extent::new(1e-6, 1200.0),
        Extent::new(1e15, 1200.0),
        Extent::new(60.0, 1.0),
        Extent::new(60.0, 1e6),
    ] {
        let frame = View::default().frame(extent);
        let count = ruler_ticks(frame).len();
        let room = frame.width() / MIN_TICK_SPACING + 2.0;
        assert!(
            f64::from(u32::try_from(count).unwrap()) <= room,
            "{extent:?}: {count}"
        );
    }
}

#[test]
fn a_ruler_at_the_smallest_zoom_has_no_tick_at_a_bad_place() {
    // At 1e-307 pixels per second, the label step grows past the range of
    // `f64` before it leaves room for a label. The end of the window stays
    // finite, so the ruler still looks for ticks.
    let frame = View::default().frame(Extent::new(1e307, 1.0));
    assert!(frame.end().is_finite());
    assert!(ruler_ticks(frame).iter().all(|tick| tick.x.is_finite()));
}
