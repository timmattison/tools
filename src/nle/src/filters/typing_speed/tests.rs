//! Tests for the typing speed filter.
//!
//! Most tests use 4 characters per second, so each interval is 0.25 s, which
//! a float holds exactly.

use super::*;
use crate::filters;
use crate::types::{Event, EventType};

fn key(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Input,
        data: data.to_owned(),
    }
}

fn times(events: &[Event]) -> Vec<f64> {
    events.iter().map(|event| event.time).collect()
}

/// A normalization at `rate` characters per second, with a pause threshold
/// of 2 s and no jitter.
fn typing(rate: f64) -> Normalization {
    Normalization {
        rate: TypingRate::new(rate).unwrap(),
        pause: Seconds::new(2.0).unwrap(),
        jitter: Jitter::NONE,
    }
}

fn run(events: Vec<Event>, filters_list: &[AppliedFilter]) -> Vec<Event> {
    filters::registry().run(events, filters_list).unwrap()
}

#[test]
fn each_key_of_a_burst_comes_at_the_target_rate() {
    let input = vec![
        key(1.0, "a"),
        key(1.1, "é"),
        key(1.7, "🎉"),
        key(1.8, "日本"),
    ];

    let got = run(input, &[applied(FilterRegion::All, typing(4.0))]);

    assert_eq!(times(&got), vec![1.0, 1.25, 1.5, 1.75]);
    let data: Vec<&str> = got.iter().map(|event| event.data.as_str()).collect();
    assert_eq!(data, vec!["a", "é", "🎉", "日本"]);
    assert!(got.iter().all(|event| event.event_type == EventType::Input));
}
