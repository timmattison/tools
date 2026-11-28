//! Tests of the shared splicer.

use std::cell::Cell;

use super::*;

/// The replacement that the test editor gives for `CSI 1 m`.
const REPLACEMENT: &str = "\x1b[2m";

/// An editor that replaces each `CSI 1 m` with `REPLACEMENT`. Its `settle`
/// gives the text that the test set.
struct TestEditor {
    settle_text: String,
}

impl TestEditor {
    fn new(settle_text: &str) -> Self {
        Self {
            settle_text: settle_text.to_string(),
        }
    }
}

impl SequenceEditor for TestEditor {
    fn rewrite(&mut self, sequence: &Sequence) -> Option<String> {
        match sequence {
            Sequence::Csi {
                params,
                intermediates,
                final_char: 'm',
                ..
            } if params == "1" && intermediates.is_empty() => Some(REPLACEMENT.to_string()),
            _ => None,
        }
    }

    fn settle(&mut self) -> String {
        self.settle_text.clone()
    }
}

/// Read each of `chunks` into its own output event, at the times 1, 2, and
/// so on. Returns the splicer.
fn splice(chunks: &[&str], editor: &mut TestEditor) -> Splicer {
    let mut splicer = Splicer::new();
    for (position, chunk) in chunks.iter().enumerate() {
        let index = splicer.open_output(f64::from(u32::try_from(position + 1).unwrap()));
        splicer.read(index, chunk, editor);
    }
    splicer
}

/// The data of each event of `events`.
fn datas(events: &[Event]) -> Vec<&str> {
    events.iter().map(|event| event.data.as_str()).collect()
}

#[test]
fn a_sequence_that_completes_in_a_later_event_leaves_both_pieces() {
    let mut editor = TestEditor::new("");
    let events =
        splice(&["text\x1b[", "1m"], &mut editor).finish(String::new(), |_| String::new(), 9.0);
    assert_eq!(datas(&events), vec!["text", REPLACEMENT]);
}

#[test]
fn a_rewritten_sequence_that_ended_an_osc_gets_an_st_where_it_started() {
    let mut editor = TestEditor::new("");
    let events =
        splice(&["\x1b]0;title\x1b[1m"], &mut editor).finish(String::new(), |_| String::new(), 9.0);
    assert_eq!(
        datas(&events),
        vec![format!("\x1b]0;title{ST}{REPLACEMENT}")]
    );
}

#[test]
fn finish_puts_the_tail_before_a_sequence_that_is_open() {
    let mut editor = TestEditor::new("");
    let saw_open = Cell::new(None);
    let events = splice(&["ab\x1b[1"], &mut editor).finish(
        String::new(),
        |open| {
            saw_open.set(Some(open));
            "TAIL".to_string()
        },
        9.0,
    );
    assert_eq!(saw_open.get(), Some(true));
    assert_eq!(datas(&events), vec!["abTAIL\x1b[1"]);
}

#[test]
fn finish_without_an_open_sequence_adds_events_only_for_text() {
    let saw_open = Cell::new(None);
    let mut editor = TestEditor::new("");
    let with_tail = splice(&["ab"], &mut editor).finish(
        "START".to_string(),
        |open| {
            saw_open.set(Some(open));
            "TAIL".to_string()
        },
        9.0,
    );
    assert_eq!(saw_open.get(), Some(false));
    assert_eq!(datas(&with_tail), vec!["START", "ab", "TAIL"]);
    let times: Vec<f64> = with_tail.iter().map(|event| event.time).collect();
    assert_eq!(times, vec![0.0, 1.0, 9.0]);

    let without = splice(&["ab"], &mut editor).finish(String::new(), |_| String::new(), 9.0);
    assert_eq!(datas(&without), vec!["ab"]);
}
