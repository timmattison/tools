use super::*;

/// A time that the test states as a number.
fn secs(value: f64) -> Seconds {
    Seconds::new(value).expect("a valid time")
}

#[test]
fn advance_inside_the_timeline_adds_the_elapsed_time() {
    let (next, ended) = advance(secs(1.0), Duration::from_millis(500), secs(10.0));
    assert_eq!(next, secs(1.5));
    assert!(!ended);
}

#[test]
fn advance_exactly_to_the_end_ends_playback() {
    let (next, ended) = advance(secs(9.5), Duration::from_millis(500), secs(10.0));
    assert_eq!(next, secs(10.0));
    assert!(ended);
}

#[test]
fn advance_past_the_end_holds_the_playhead_at_the_end() {
    let (next, ended) = advance(secs(9.0), Duration::from_secs(5), secs(10.0));
    assert_eq!(next, secs(10.0));
    assert!(ended);
}

#[test]
fn advance_on_an_empty_timeline_ends_at_zero() {
    let (next, ended) = advance(Seconds::ZERO, Duration::from_millis(33), Seconds::ZERO);
    assert_eq!(next, Seconds::ZERO);
    assert!(ended);
}

#[test]
fn advance_with_an_enormous_elapsed_time_does_not_panic() {
    let (next, ended) = advance(secs(1.0), Duration::MAX, secs(10.0));
    assert_eq!(next, secs(10.0));
    assert!(ended);
}

#[test]
fn play_from_the_middle_keeps_the_playhead() {
    assert_eq!(play_from(secs(3.0), secs(10.0)), secs(3.0));
}

#[test]
fn play_from_the_end_starts_at_the_beginning() {
    assert_eq!(play_from(secs(10.0), secs(10.0)), Seconds::ZERO);
}

#[test]
fn play_from_past_the_end_starts_at_the_beginning() {
    assert_eq!(play_from(secs(12.0), secs(10.0)), Seconds::ZERO);
}
