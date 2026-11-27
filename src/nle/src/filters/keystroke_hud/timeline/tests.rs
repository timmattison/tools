//! Tests of the timeline of the keystroke HUD.

use super::{Piece, Press, Shown, Timeline, Timing, MAX_FADE_STEPS};
use crate::filters::keystroke_hud::keys::Key;

const TOLERANCE: f64 = 1e-9;

fn text(s: &str) -> Key {
    Key::Text(s.to_string())
}

fn special(label: &str) -> Key {
    Key::Special(label.to_string())
}

fn timeline(presses: &[(f64, Key)], timing: Timing) -> Timeline {
    let presses = presses
        .iter()
        .map(|(time, key)| Press {
            time: *time,
            key: key.clone(),
        })
        .collect();
    Timeline::new(presses, timing)
}

fn no_fade(duration: f64) -> Timing {
    Timing {
        duration,
        fade: 0.0,
        limit: f64::INFINITY,
    }
}

fn pieces(shown: Option<Shown>) -> Vec<Piece> {
    shown.expect("the HUD shows a box").pieces
}

fn piece_text(s: &str) -> Piece {
    Piece::Text(s.to_string())
}

fn piece_special(label: &str, count: u32) -> Piece {
    Piece::Special {
        label: label.to_string(),
        count,
    }
}

fn assert_close(left: f64, right: f64) {
    assert!((left - right).abs() < TOLERANCE, "{left} is not {right}");
}

#[test]
fn keys_close_in_time_make_one_box() {
    let line = timeline(&[(0.0, text("l")), (0.1, text("s"))], no_fade(1.0));
    let shown = line.at(0.15).unwrap();
    assert_eq!(shown.pieces, vec![piece_text("ls")]);
    assert_close(shown.alpha, 1.0);
    assert_eq!(pieces(line.at(0.05)), vec![piece_text("l")]);
}

#[test]
fn a_press_after_the_duration_starts_a_new_box() {
    let line = timeline(&[(0.0, text("a")), (1.0, text("b"))], no_fade(1.0));
    assert_eq!(pieces(line.at(0.5)), vec![piece_text("a")]);
    assert_eq!(pieces(line.at(1.0)), vec![piece_text("b")]);
    let gap = timeline(&[(0.0, text("a")), (2.0, text("b"))], no_fade(1.0));
    assert_eq!(gap.at(1.5), None);
    assert_eq!(pieces(gap.at(2.0)), vec![piece_text("b")]);
}

#[test]
fn a_press_just_before_the_duration_joins_the_box() {
    let line = timeline(&[(0.0, text("a")), (0.99, text("b"))], no_fade(1.0));
    assert_eq!(pieces(line.at(1.0)), vec![piece_text("ab")]);
}

#[test]
fn enter_closes_a_box() {
    let line = timeline(
        &[
            (0.0, text("l")),
            (0.1, text("s")),
            (0.2, special("Enter")),
            (0.3, text("cd")),
        ],
        no_fade(1.0),
    );
    assert_eq!(
        pieces(line.at(0.25)),
        vec![piece_text("ls"), piece_special("Enter", 1)]
    );
    assert_eq!(pieces(line.at(0.35)), vec![piece_text("cd")]);
}

#[test]
fn enter_alone_makes_a_box() {
    let line = timeline(&[(0.0, special("Enter"))], no_fade(1.0));
    assert_eq!(pieces(line.at(0.5)), vec![piece_special("Enter", 1)]);
}

#[test]
fn a_held_key_counts_its_presses() {
    let line = timeline(
        &[
            (0.0, special("↓")),
            (0.1, special("↓")),
            (0.2, special("↓")),
        ],
        no_fade(1.0),
    );
    assert_eq!(pieces(line.at(0.5)), vec![piece_special("↓", 3)]);
}

#[test]
fn a_special_key_and_text_make_two_pieces() {
    let line = timeline(
        &[(0.0, special("Ctrl+R")), (0.1, text("git"))],
        no_fade(1.0),
    );
    assert_eq!(
        pieces(line.at(0.5)),
        vec![piece_special("Ctrl+R", 1), piece_text("git")]
    );
}

#[test]
fn different_special_keys_do_not_merge() {
    let line = timeline(&[(0.0, special("↑")), (0.1, special("↓"))], no_fade(1.0));
    assert_eq!(
        pieces(line.at(0.5)),
        vec![piece_special("↑", 1), piece_special("↓", 1)]
    );
}

#[test]
fn text_of_only_spaces_becomes_a_space_key() {
    let line = timeline(&[(0.0, text("   "))], no_fade(1.0));
    assert_eq!(pieces(line.at(0.5)), vec![piece_special("Space", 3)]);
    let split = timeline(&[(0.0, text(" ")), (0.1, text(" "))], no_fade(1.0));
    assert_eq!(pieces(split.at(0.5)), vec![piece_special("Space", 2)]);
}

#[test]
fn spaces_inside_text_stay_text() {
    let line = timeline(&[(0.0, text("ls -la"))], no_fade(1.0));
    assert_eq!(pieces(line.at(0.5)), vec![piece_text("ls -la")]);
}

#[test]
fn presses_out_of_order_are_sorted() {
    let line = timeline(&[(0.1, text("s")), (0.0, text("l"))], no_fade(1.0));
    assert_eq!(pieces(line.at(0.5)), vec![piece_text("ls")]);
}

#[test]
fn a_box_fades_in_and_out() {
    let timing = Timing {
        duration: 1.0,
        fade: 0.2,
        limit: f64::INFINITY,
    };
    let line = timeline(&[(1.0, text("a"))], timing);
    assert_eq!(line.at(1.0), None);
    assert_close(line.at(1.1).unwrap().alpha, 0.5);
    assert_close(line.at(1.5).unwrap().alpha, 1.0);
    assert_close(line.at(1.95).unwrap().alpha, 0.25);
    assert_eq!(line.at(2.0), None);
    assert_eq!(line.at(0.5), None);
}

#[test]
fn a_cut_box_vanishes_with_no_fade_out() {
    let timing = Timing {
        duration: 1.0,
        fade: 0.2,
        limit: f64::INFINITY,
    };
    let line = timeline(
        &[(0.0, text("a")), (0.2, special("Enter")), (0.5, text("b"))],
        timing,
    );
    assert_close(line.at(0.49).unwrap().alpha, 1.0);
    let second = line.at(0.5);
    assert_eq!(second, None);
    assert_eq!(pieces(line.at(0.6)), vec![piece_text("b")]);
}

#[test]
fn the_limit_cuts_the_window() {
    let timing = Timing {
        duration: 1.0,
        fade: 0.2,
        limit: 1.5,
    };
    let line = timeline(&[(1.0, text("a"))], timing);
    assert_close(line.at(1.49).unwrap().alpha, 1.0);
    assert_eq!(line.at(1.5), None);
}

#[test]
fn a_press_at_or_after_the_limit_shows_nothing() {
    let timing = Timing {
        duration: 1.0,
        fade: 0.0,
        limit: 2.0,
    };
    let line = timeline(&[(2.0, text("a")), (3.0, text("b"))], timing);
    assert_eq!(line.at(2.0), None);
    assert_eq!(line.at(3.5), None);
    assert!(line.frames().is_empty());
}

#[test]
fn frames_without_fade_are_the_presses_and_the_ends() {
    let line = timeline(
        &[(0.0, text("a")), (0.5, text("b")), (3.0, text("c"))],
        no_fade(1.0),
    );
    assert_eq!(line.frames(), vec![0.0, 0.5, 1.5, 3.0, 4.0]);
}

#[test]
fn a_cut_box_ends_where_the_next_box_starts() {
    let line = timeline(&[(0.0, special("Enter")), (0.5, text("b"))], no_fade(1.0));
    assert_eq!(line.frames(), vec![0.0, 0.5, 1.5]);
}

#[test]
fn frames_with_fade_hold_the_steps() {
    let timing = Timing {
        duration: 0.5,
        fade: 0.1,
        limit: f64::INFINITY,
    };
    let line = timeline(&[(1.0, text("a"))], timing);
    let frames = line.frames();
    let expected = [
        1.0,
        1.0 + 0.1 / 3.0,
        1.0 + 0.2 / 3.0,
        1.1,
        1.5 - 0.1 + 0.1 / 3.0,
        1.5 - 0.1 + 0.2 / 3.0,
        1.5,
    ];
    assert_eq!(frames.len(), expected.len(), "{frames:?}");
    for (frame, want) in frames.iter().zip(expected) {
        assert_close(*frame, want);
    }
}

#[test]
fn frames_are_sorted_and_have_no_equal_neighbors() {
    let timing = Timing {
        duration: 0.3,
        fade: 0.2,
        limit: 10.0,
    };
    let line = timeline(
        &[
            (0.0, text("a")),
            (0.1, text("b")),
            (0.2, special("Enter")),
            (0.25, text("c")),
        ],
        timing,
    );
    let frames = line.frames();
    assert!(!frames.is_empty());
    for pair in frames.windows(2) {
        assert!(pair[0] < pair[1], "{frames:?}");
    }
}

#[test]
fn a_long_fade_makes_a_bounded_frame_list() {
    let huge = Timing {
        duration: 1.0,
        fade: 1e300,
        limit: f64::INFINITY,
    };
    let line = timeline(&[(0.0, text("a"))], huge);
    assert!(line.frames().len() <= 2 * MAX_FADE_STEPS as usize + 3);
    let long = Timing {
        duration: 1000.0,
        fade: 100.0,
        limit: f64::INFINITY,
    };
    let line = timeline(&[(0.0, text("a"))], long);
    let frames = line.frames();
    let fade_in = frames.iter().filter(|time| **time <= 100.0).count();
    assert!(fade_in <= MAX_FADE_STEPS as usize + 1, "{fade_in}");
    assert!(frames.len() <= 2 * MAX_FADE_STEPS as usize + 3);
}
