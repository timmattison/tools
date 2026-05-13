//! Golden-file tests that pin the locked-down ordinal policy for tsm-tuple.

use tsm_tuple::{derive_tuple, Env, LayoutText, TupleError};

const SESSION: &str = "fixture-session";

fn load(fixture: &str) -> LayoutText {
    let path = format!(
        "{}/tests/fixtures/{}.kdl",
        env!("CARGO_MANIFEST_DIR"),
        fixture
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("failed to read fixture {path:?}: {e}");
    });
    LayoutText(text)
}

fn env(pane_id: &str) -> Env {
    Env {
        zellij_session_name: SESSION.to_string(),
        zellij_pane_id: pane_id.to_string(),
    }
}

fn expect_tuple(fixture: &str, pane_id: &str, expected_tab: &str, expected_ordinal: u32) {
    let layout = load(fixture);
    let tup = derive_tuple(&env(pane_id), &layout)
        .unwrap_or_else(|e| panic!("derive_tuple failed for {fixture}/pane {pane_id}: {e}"));
    assert_eq!(
        tup.zellij_session_name.as_ref(),
        SESSION,
        "session name should round-trip"
    );
    assert_eq!(
        tup.tab_name.as_ref(),
        expected_tab,
        "tab name mismatch for {fixture}/pane {pane_id}"
    );
    assert_eq!(
        tup.pane_ordinal_within_tab.value(),
        expected_ordinal,
        "ordinal mismatch for {fixture}/pane {pane_id}"
    );
}

#[test]
fn simple_two_tabs_assigns_ordinals_per_tab() {
    expect_tuple("simple_two_tabs", "1", "editor", 0);
    expect_tuple("simple_two_tabs", "2", "editor", 1);
    expect_tuple("simple_two_tabs", "3", "logs", 0);
    expect_tuple("simple_two_tabs", "4", "logs", 1);
}

#[test]
fn floating_panes_are_appended_after_tiled_panes() {
    // Tiled first.
    expect_tuple("floating_panes", "10", "main", 0);
    expect_tuple("floating_panes", "11", "main", 1);
    // Floating panes get ordinals AFTER the tiled tree.
    expect_tuple("floating_panes", "20", "main", 2);
    expect_tuple("floating_panes", "21", "main", 3);
}

#[test]
fn plugin_panes_are_skipped_entirely() {
    // pane 30 is the first terminal pane.
    expect_tuple("plugin_panes", "30", "mixed", 0);
    // pane 31 follows a skipped `pane { plugin { ... } }`.
    expect_tuple("plugin_panes", "31", "mixed", 1);
    // pane 33 follows a skipped `pane plugin="..."`.
    expect_tuple("plugin_panes", "33", "mixed", 2);

    // Plugin panes themselves must NOT match.
    let layout = load("plugin_panes");
    let err = derive_tuple(&env("32"), &layout)
        .expect_err("plugin pane should not be findable as a terminal pane");
    assert!(matches!(err, TupleError::PaneNotFound { .. }));
}

#[test]
fn suppressed_panes_keep_their_tree_dfs_ordinal() {
    expect_tuple("suppressed_panes", "40", "work", 0);
    // The suppressed pane is counted at its document position.
    expect_tuple("suppressed_panes", "41", "work", 1);
    expect_tuple("suppressed_panes", "42", "work", 2);
}

#[test]
fn nested_split_uses_depth_first_left_to_right_order() {
    expect_tuple("nested_split", "50", "deep", 0);
    expect_tuple("nested_split", "51", "deep", 1);
    expect_tuple("nested_split", "52", "deep", 2);
    expect_tuple("nested_split", "53", "deep", 3);
    expect_tuple("nested_split", "54", "deep", 4);
    expect_tuple("nested_split", "55", "deep", 5);
}

#[test]
fn missing_pane_returns_pane_not_found() {
    let layout = load("simple_two_tabs");
    let err = derive_tuple(&env("9999"), &layout).expect_err("unknown pane id must error");
    match err {
        TupleError::PaneNotFound { pane_id } => assert_eq!(pane_id, "9999"),
        other => panic!("expected PaneNotFound, got {other:?}"),
    }
}

#[test]
fn empty_layout_returns_no_tabs() {
    let layout = LayoutText("layout {\n}\n".to_string());
    let err = derive_tuple(&env("1"), &layout).expect_err("no tabs should fail");
    assert!(matches!(err, TupleError::NoTabs));
}

#[test]
fn unparseable_layout_returns_layout_parse_error() {
    let layout = LayoutText("this is { not valid kdl ".to_string());
    let err = derive_tuple(&env("1"), &layout).expect_err("garbage must fail to parse");
    assert!(matches!(err, TupleError::LayoutParse(_)));
}

#[test]
fn ambiguous_pane_id_is_rejected() {
    let layout = LayoutText(
        "layout {\n  tab name=\"a\" {\n    pane id=7\n  }\n  tab name=\"b\" {\n    pane id=7\n  }\n}\n"
            .to_string(),
    );
    let err = derive_tuple(&env("7"), &layout).expect_err("duplicate id must fail");
    match err {
        TupleError::AmbiguousPaneId { pane_id } => assert_eq!(pane_id, "7"),
        other => panic!("expected AmbiguousPaneId, got {other:?}"),
    }
}

#[test]
fn tab_without_name_yields_tab_name_missing() {
    let layout = LayoutText(
        "layout {\n  tab {\n    pane id=42\n  }\n}\n".to_string(),
    );
    let err = derive_tuple(&env("42"), &layout).expect_err("nameless tab must fail");
    match err {
        TupleError::TabNameMissing { pane_id } => assert_eq!(pane_id, "42"),
        other => panic!("expected TabNameMissing, got {other:?}"),
    }
}

#[test]
fn utf8_tab_name_is_preserved() {
    let layout = LayoutText(
        "layout {\n  tab name=\"日本語 🎉 café\" {\n    pane id=1\n  }\n}\n".to_string(),
    );
    let tup = derive_tuple(&env("1"), &layout).expect("multi-byte tab name should parse");
    assert_eq!(tup.tab_name.as_ref(), "日本語 🎉 café");
    assert_eq!(tup.pane_ordinal_within_tab.value(), 0);
}
