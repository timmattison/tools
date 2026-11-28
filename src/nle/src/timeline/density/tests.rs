//! Tests of the event density of the clips.

use std::collections::BTreeMap;

use super::*;
use crate::types::{ClipId, Event, EventType};

/// The height of a tile in the tests, in pixels.
const HEIGHT: f64 = 40.0;

/// A recording of `duration` seconds with one output event at each time.
fn recording(duration: f64, times: &[f64]) -> Arc<Recording> {
    Arc::new(Recording {
        version: 2,
        width: 80,
        height: 24,
        timestamp: 0.0,
        duration,
        command: String::new(),
        title: String::new(),
        env: BTreeMap::new(),
        events: times
            .iter()
            .map(|&time| Event {
                time,
                event_type: EventType::Output,
                data: "x".into(),
            })
            .collect(),
        chapters: Vec::new(),
    })
}

/// A time in seconds.
fn seconds(value: f64) -> Seconds {
    Seconds::new(value).expect("a valid time")
}

/// A clip of `source` from `in_point` to `out_point`, at `offset` on the
/// timeline.
fn clip(source: &Arc<Recording>, in_point: f64, out_point: f64, offset: f64) -> Clip {
    Clip {
        id: ClipId(1),
        source: Arc::clone(source),
        in_point: seconds(in_point),
        out_point: seconds(out_point),
        timeline_offset: seconds(offset),
    }
}

/// The events of a clip of the whole of a recording of `length` seconds with
/// events at `times`.
fn events(length: f64, times: &[f64]) -> ClipEvents {
    ClipEvents::of(&clip(&recording(length, times), 0.0, length, 0.0))
}

#[test]
fn the_events_of_a_clip_are_its_times_from_its_start() {
    let source = recording(10.0, &[0.5, 2.0, 3.0, 4.0, 9.0]);
    let trimmed = ClipEvents::of(&clip(&source, 2.0, 4.0, 30.0));
    assert_eq!(trimmed.times(), &[0.0, 1.0]);
    assert_eq!(trimmed.length(), 2.0);
}

#[test]
fn a_clip_to_the_end_holds_the_event_at_its_end() {
    let source = recording(9.0, &[1.0, 9.0]);
    assert_eq!(
        ClipEvents::of(&clip(&source, 1.0, 9.0, 0.0)).times(),
        &[0.0, 8.0]
    );
}

#[test]
fn the_events_of_a_source_out_of_order_come_in_order() {
    let source = recording(5.0, &[3.0, 1.0, 2.0]);
    assert_eq!(
        ClipEvents::of(&clip(&source, 0.0, 5.0, 0.0)).times(),
        &[1.0, 2.0, 3.0]
    );
}

#[test]
fn two_lists_are_equal_only_when_they_are_one_list() {
    let source = recording(5.0, &[1.0]);
    let first = ClipEvents::of(&clip(&source, 0.0, 5.0, 0.0));
    assert_eq!(first, first.clone());
    assert_ne!(first, ClipEvents::of(&clip(&source, 0.0, 5.0, 0.0)));
}

#[test]
fn the_cache_gives_the_same_list_to_a_clip_that_did_not_change() {
    let source = recording(10.0, &[1.0, 5.0]);
    let mut cache = EventCache::default();
    let first = cache.events(&[clip(&source, 0.0, 10.0, 0.0)]);
    let again = cache.events(&[clip(&source, 0.0, 10.0, 0.0)]);
    assert_eq!(first, again);
}

#[test]
fn the_cache_keeps_the_list_of_a_moved_clip() {
    let source = recording(10.0, &[1.0, 5.0]);
    let mut cache = EventCache::default();
    let first = cache.events(&[clip(&source, 0.0, 10.0, 0.0)]);
    let moved = cache.events(&[clip(&source, 0.0, 10.0, 25.0)]);
    assert_eq!(first, moved);
}

#[test]
fn the_cache_gives_a_new_list_to_a_trimmed_clip_or_a_new_source() {
    let source = recording(10.0, &[1.0, 5.0]);
    let mut cache = EventCache::default();
    let first = cache.events(&[clip(&source, 0.0, 10.0, 0.0)]);
    let trimmed = cache.events(&[clip(&source, 2.0, 10.0, 0.0)]);
    assert_ne!(first, trimmed);
    assert_eq!(trimmed[0].times(), &[3.0]);
    let other = cache.events(&[clip(&recording(10.0, &[1.0, 5.0]), 2.0, 10.0, 0.0)]);
    assert_ne!(trimmed, other);
}

#[test]
fn the_cache_keeps_only_the_clips_of_the_last_call() {
    let source = recording(10.0, &[1.0]);
    let mut cache = EventCache::default();
    let first = cache.events(&[clip(&source, 0.0, 10.0, 0.0)]);
    cache.events(&[]);
    assert_ne!(cache.events(&[clip(&source, 0.0, 10.0, 0.0)]), first);
}

#[test]
fn the_cache_gives_each_of_two_clips_its_own_list() {
    let source = recording(10.0, &[1.0, 6.0]);
    let mut cache = EventCache::default();
    let lists = cache.events(&[clip(&source, 0.0, 5.0, 0.0), clip(&source, 5.0, 10.0, 5.0)]);
    assert_eq!(lists[0].times(), &[1.0]);
    assert_eq!(lists[1].times(), &[1.0]);
    assert_ne!(lists[0], lists[1]);
}

#[test]
fn the_tiles_of_a_clip_in_the_window_render_with_the_overscan() {
    assert_eq!(visible_tiles(0.0, 2000.0, 1200.0), 0..4);
    assert_eq!(visible_tiles(0.0, 10_000.0, 1200.0), 0..4);
    assert_eq!(visible_tiles(-5000.0, 10_000.0, 1200.0), 8..14);
}

#[test]
fn a_clip_out_of_the_window_and_the_overscan_has_no_tile() {
    assert!(visible_tiles(1801.0, 2000.0, 1200.0).is_empty());
    assert!(visible_tiles(-20_000.0, 10_000.0, 1200.0).is_empty());
    assert!(visible_tiles(0.0, 0.0, 1200.0).is_empty());
}

#[test]
fn a_bad_place_gives_no_tile() {
    for (x, width) in [
        (f64::NAN, 2000.0),
        (0.0, f64::NAN),
        (0.0, f64::INFINITY),
        (-1e300, 1e300),
    ] {
        let tiles = visible_tiles(x, width, 1200.0);
        assert!(tiles.len() <= 8, "{x} {width}: {tiles:?}");
    }
}

#[test]
fn the_last_tile_is_narrower() {
    let widths: Vec<f64> = (0..4).map(|index| tile_width(2000.0, index)).collect();
    assert_eq!(widths, [512.0, 512.0, 512.0, 464.0]);
    assert_eq!(tile_width(2000.0, 4), 0.0);
}

#[test]
fn the_density_has_one_line_for_each_column_with_events() {
    let events = events(1.0, &[0.0, 0.01, 0.5, 0.5, 1.0]);
    assert_eq!(
        density_path(&events, 0, 100.0, HEIGHT),
        "M0.5 40v-6.6M1.5 40v-6.6M50.5 40v-10.5M99.5 40v-6.6"
    );
}

#[test]
fn sixty_four_events_in_one_column_fill_the_tile() {
    let times = vec![2.0; 100];
    assert_eq!(
        density_path(&events(5.0, &times), 0, 1.0, HEIGHT),
        "M2.5 40v-40"
    );
    let times = vec![2.0; 64];
    assert_eq!(
        density_path(&events(5.0, &times), 0, 1.0, HEIGHT),
        "M2.5 40v-40"
    );
}

#[test]
fn a_later_tile_counts_its_columns_from_its_own_left_edge() {
    let events = events(10.0, &[1.0, 6.0]);
    assert_eq!(density_path(&events, 1, 100.0, HEIGHT), "M88.5 40v-6.6");
    assert_eq!(density_path(&events, 0, 100.0, HEIGHT), "M100.5 40v-6.6");
}

#[test]
fn a_tile_with_no_event_or_past_the_end_has_an_empty_path() {
    let events = events(10.0, &[1.0]);
    assert_eq!(density_path(&events, 1, 100.0, HEIGHT), "");
    assert_eq!(density_path(&events, 5, 100.0, HEIGHT), "");
    assert_eq!(density_path(&events, usize::MAX, 100.0, HEIGHT), "");
}

#[test]
fn a_bad_zoom_gives_an_empty_path() {
    let events = events(10.0, &[1.0]);
    for zoom in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(density_path(&events, 0, zoom, HEIGHT), "", "{zoom}");
    }
}

#[test]
fn a_dense_clip_has_at_most_one_line_for_each_column() {
    let times: Vec<f64> = (0..100_000)
        .map(|index| f64::from(index) / 1000.0)
        .collect();
    let path = density_path(&events(100.0, &times), 0, 12.0, HEIGHT);
    assert_eq!(path.matches('M').count(), 512);
}
