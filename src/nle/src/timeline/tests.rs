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
    let start = View::default().zoom(minute(), 4.0, 0.0).pan(minute(), 1600.0);
    let before = start.frame(minute()).time_at(900.0);
    let frame = start.zoom(minute(), 1.0 / 2.0, 900.0).frame(minute());
    assert_close(frame.pixels_per_second(), 40.0);
    assert_close(frame.time_at(900.0), before);
}

#[test]
fn a_zoom_stops_at_the_largest_zoom() {
    let frame = View::default()
        .zoom(minute(), 1e9, 600.0)
        .frame(minute());
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
    let frame = View::default()
        .zoom(minute(), 2.0, 1200.0)
        .frame(minute());
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
    assert_close(view.frame(Extent::new(60.0, 1800.0)).pixels_per_second(), 40.0);
    assert_close(view.frame(Extent::new(60.0, 3000.0)).pixels_per_second(), 50.0);
}

#[test]
fn a_pan_moves_the_window_by_the_pixels() {
    let view = View::default().zoom(minute(), 4.0, 0.0);
    let frame = view.pan(minute(), 160.0).frame(minute());
    assert_close(frame.start(), 2.0);
    let back = view.pan(minute(), 160.0).pan(minute(), -80.0).frame(minute());
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
    let view = View::default().zoom(minute(), 4.0, 0.0).pan(minute(), 100.0);
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
