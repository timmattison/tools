//! Tests for the redaction filter.

use super::*;
use crate::filters;
use crate::types::{Event, EventType};

fn out(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Output,
        data: data.to_string(),
    }
}

fn inp(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Input,
        data: data.to_string(),
    }
}

fn text(find: &str, replacement: &str) -> Redaction {
    Redaction::new(
        Find::Text(find.to_string()),
        Replacement::Text(replacement.to_string()),
        false,
    )
    .unwrap()
}

fn run(events: Vec<Event>, redaction: &Redaction) -> Vec<Event> {
    filters::registry()
        .run(events, &[applied(FilterRegion::All, redaction)])
        .unwrap()
}

fn datas(events: &[Event]) -> Vec<&str> {
    events.iter().map(|e| e.data.as_str()).collect()
}

#[test]
fn the_registry_holds_the_redaction_filter() {
    assert!(filters::registry().contains(&kind()));
}

#[test]
fn text_is_replaced_in_one_output_event() {
    let got = run(
        vec![out(1.0, "token=hunter2\r\n")],
        &text("hunter2", "[REDACTED]"),
    );
    assert_eq!(datas(&got), vec!["token=[REDACTED]\r\n"]);
}

#[test]
fn input_events_and_times_stay() {
    let got = run(
        vec![out(1.0, "a hunter2"), inp(2.5, "hunter2"), out(4.0, "b")],
        &text("hunter2", "x"),
    );
    assert_eq!(datas(&got), vec!["a x", "hunter2", "b"]);
    assert_eq!(
        got.iter().map(|e| e.time).collect::<Vec<_>>(),
        vec![1.0, 2.5, 4.0]
    );
    assert_eq!(got[1].event_type, EventType::Input);
}

#[test]
fn a_match_split_across_two_output_events_is_replaced() {
    let got = run(
        vec![out(1.0, "hun"), out(1.1, "ter2")],
        &text("hunter2", "[REDACTED]"),
    );
    assert_eq!(datas(&got), vec!["[REDACTED]", ""]);
}

#[test]
fn an_input_event_between_two_output_events_does_not_break_a_match() {
    let got = run(
        vec![out(1.0, "hun"), inp(1.05, "x"), out(1.1, "ter2")],
        &text("hunter2", "[R]"),
    );
    assert_eq!(datas(&got), vec!["[R]", "x", ""]);
}

#[test]
fn multi_byte_text_split_across_events_is_replaced() {
    let got = run(
        vec![out(1.0, "日本"), out(1.1, "語 🎉 café")],
        &text("日本語", "?"),
    );
    assert_eq!(datas(&got), vec!["?", " 🎉 café"]);
}

fn one(data: &str, find: &str, replacement: &str) -> String {
    let got = run(vec![out(1.0, data)], &text(find, replacement));
    got.into_iter().map(|e| e.data).collect()
}

#[test]
fn escape_codes_around_a_match_stay() {
    assert_eq!(
        one("\x1b[31mhunter2\x1b[0m", "hunter2", "[R]"),
        "\x1b[31m[R]\x1b[0m"
    );
}

#[test]
fn text_that_looks_like_the_parameters_of_an_escape_code_is_not_found() {
    assert_eq!(one("\x1b[31mred", "31", "X"), "\x1b[31mred");
    assert_eq!(one("\x1b[31mred", "[31m", "X"), "\x1b[31mred");
}

#[test]
fn a_match_across_an_escape_code_keeps_the_code() {
    assert_eq!(
        one("hun\x1b[1mter2", "hunter2", "[REDACTED]"),
        "[REDACTED]\x1b[1m"
    );
}

#[test]
fn an_escape_code_split_across_events_is_one_code() {
    let got = run(vec![out(1.0, "\x1b[3"), out(1.1, "1m x")], &text("1m", "Z"));
    assert_eq!(datas(&got), vec!["\x1b[3", "1m x"]);
}

#[test]
fn an_osc_title_is_searched() {
    assert_eq!(
        one("\x1b]0;user@prod-db\x07", "prod-db", "HOST"),
        "\x1b]0;user@HOST\x07"
    );
    assert_eq!(
        one("\x1b]0;user@prod-db\x1b\\x", "prod-db", "HOST"),
        "\x1b]0;user@HOST\x1b\\x"
    );
}

#[test]
fn a_match_does_not_span_visible_text_and_an_osc_payload() {
    assert_eq!(one("ab\x1b]0;cd\x07", "b0", "X"), "ab\x1b]0;cd\x07");
    assert_eq!(one("ab\x1b]0;cd\x07", "bcd", "X"), "ab\x1b]0;cd\x07");
}

#[test]
fn the_content_of_a_device_control_string_is_not_searched() {
    assert_eq!(
        one("\x1bPqhunter2\x1b\\ hunter2", "hunter2", "X"),
        "\x1bPqhunter2\x1b\\ X"
    );
    assert_eq!(
        one("\x1b_Gkitty-hunter2\x1b\\", "hunter2", "X"),
        "\x1b_Gkitty-hunter2\x1b\\"
    );
}

#[test]
fn a_charset_escape_is_not_text() {
    assert_eq!(one("\x1b(Bhello", "B", "X"), "\x1b(Bhello");
    assert_eq!(one("\x1b(BBello", "B", "X"), "\x1b(BXello");
}

#[test]
fn a_bare_escape_does_not_swallow_the_text_after_it() {
    assert_eq!(one("\x1b\x1b\rhunter2", "hunter2", "X"), "\x1b\x1b\rX");
    assert_eq!(one("a\x1b", "a", "X"), "X\x1b");
}

#[test]
fn multi_byte_text_beside_escape_codes_is_replaced() {
    assert_eq!(
        one("\x1b[1m日本語\x1b[0m café", "日本語", "?"),
        "\x1b[1m?\x1b[0m café"
    );
}

fn masked(find: Find, replacement: Replacement, events: Vec<Event>) -> Vec<Event> {
    run(events, &Redaction::new(find, replacement, false).unwrap())
}

fn find_text(text: &str) -> Find {
    Find::Text(text.to_string())
}

#[test]
fn asterisks_take_one_column_for_each_column_of_the_match() {
    let got = masked(
        find_text("hunter2"),
        Replacement::Asterisks,
        vec![out(1.0, "a hunter2 b")],
    );
    assert_eq!(datas(&got), vec!["a ******* b"]);
}

#[test]
fn blocks_take_one_column_for_each_column_of_the_match() {
    let got = masked(
        find_text("hunter2"),
        Replacement::Blocks,
        vec![out(1.0, "hunter2")],
    );
    assert_eq!(datas(&got), vec![BLOCK.to_string().repeat(7)]);
}

#[test]
fn a_wide_character_takes_two_columns() {
    let got = masked(
        find_text("パスワード"),
        Replacement::Asterisks,
        vec![out(1.0, "パスワード")],
    );
    assert_eq!(datas(&got), vec!["**********"]);
}

#[test]
fn accented_text_keeps_its_width() {
    let got = masked(
        find_text("café"),
        Replacement::Asterisks,
        vec![out(1.0, "un café!")],
    );
    assert_eq!(datas(&got), vec!["un ****!"]);
}

#[test]
fn a_combining_mark_adds_no_column() {
    let got = masked(
        find_text("cafe\u{301}"),
        Replacement::Asterisks,
        vec![out(1.0, "cafe\u{301}")],
    );
    assert_eq!(datas(&got), vec!["****"]);
}

#[test]
fn control_characters_inside_a_match_stay() {
    let got = masked(
        Find::Regex("a\r\nb".to_string()),
        Replacement::Asterisks,
        vec![out(1.0, "a\r\nb")],
    );
    assert_eq!(datas(&got), vec!["*\r\n*"]);
}

#[test]
fn a_match_across_events_is_masked_in_each_piece() {
    let got = masked(
        find_text("hunter2"),
        Replacement::Asterisks,
        vec![out(1.0, "hun"), out(1.1, "ter2")],
    );
    assert_eq!(datas(&got), vec!["***", "****"]);
}

#[test]
fn a_mask_skips_the_escape_codes_inside_a_match() {
    let got = masked(
        find_text("hunter2"),
        Replacement::Blocks,
        vec![out(1.0, "hun\x1b[1mter2")],
    );
    assert_eq!(
        datas(&got),
        vec![format!(
            "{}\x1b[1m{}",
            "\u{2588}".repeat(3),
            "\u{2588}".repeat(4)
        )]
    );
}

fn with_input(find: &str, replacement: &str, events: Vec<Event>) -> Vec<Event> {
    let redaction = Redaction::new(
        find_text(find),
        Replacement::Text(replacement.to_string()),
        true,
    )
    .unwrap();
    run(events, &redaction)
}

#[test]
fn input_events_are_redacted_when_the_option_says_so() {
    let got = with_input(
        "hunter2",
        "X",
        vec![out(1.0, "hunter2"), inp(2.0, "hunter2\r")],
    );
    assert_eq!(datas(&got), vec!["X", "X\r"]);
    assert_eq!(got[1].event_type, EventType::Input);
}

#[test]
fn input_events_stay_when_the_option_is_off() {
    let got = run(vec![inp(2.0, "hunter2\r")], &text("hunter2", "X"));
    assert_eq!(datas(&got), vec!["hunter2\r"]);
}

#[test]
fn output_and_input_are_separate_streams() {
    let got = with_input("hunter2", "X", vec![out(1.0, "hun"), inp(1.1, "ter2")]);
    assert_eq!(datas(&got), vec!["hun", "ter2"]);
}

#[test]
fn a_match_across_input_events_is_replaced() {
    let got = with_input(
        "hunter2",
        "X",
        vec![inp(1.0, "hun"), out(1.05, "echo"), inp(1.1, "ter2")],
    );
    assert_eq!(datas(&got), vec!["X", "echo", ""]);
}

fn refusal(find: Find, replacement: Replacement) -> String {
    match Redaction::new(find, replacement, false) {
        Err(NleError::InvalidRedaction(message)) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn an_empty_pattern_is_refused() {
    let none = Replacement::Asterisks;
    assert_eq!(
        refusal(find_text(""), none.clone()),
        "the text to find is empty"
    );
    assert_eq!(
        refusal(Find::Regex(String::new()), none),
        "the text to find is empty"
    );
}

#[test]
fn a_regular_expression_that_does_not_compile_is_refused() {
    let message = refusal(Find::Regex("(unclosed".to_string()), Replacement::Asterisks);
    assert!(
        message.starts_with("the regular expression does not compile: "),
        "{message}"
    );
}

#[test]
fn a_replacement_with_a_control_character_is_refused() {
    for bad in ["a\u{1b}[2J", "a\nb", "\u{7}", "x\u{85}"] {
        assert_eq!(
            refusal(find_text("a"), Replacement::Text(bad.to_string())),
            "the replacement holds a control character",
            "{bad:?}"
        );
    }
}

#[test]
fn a_replacement_with_multi_byte_text_is_accepted() {
    let redaction = text("hunter2", "日本語 🎉");
    let got = run(vec![out(1.0, "hunter2")], &redaction);
    assert_eq!(datas(&got), vec!["日本語 🎉"]);
}

#[test]
fn a_regular_expression_finds_matches() {
    let redaction = Redaction::new(
        Find::Regex(r"\d{3}-\d{4}".to_string()),
        Replacement::Text("###".to_string()),
        false,
    )
    .unwrap();
    let got = run(vec![out(1.0, "call 555-1234 or 555-9876")], &redaction);
    assert_eq!(datas(&got), vec!["call ### or ###"]);
}

#[test]
fn only_the_redact_group_is_replaced() {
    let redaction = Redaction::new(
        Find::Regex(r"token=(?P<redact>\w+)".to_string()),
        Replacement::Asterisks,
        false,
    )
    .unwrap();
    let got = run(vec![out(1.0, "token=abc")], &redaction);
    assert_eq!(datas(&got), vec!["token=***"]);
}

#[test]
fn a_match_that_is_empty_changes_nothing() {
    let redaction = Redaction::new(
        Find::Regex("x*".to_string()),
        Replacement::Text("!".to_string()),
        false,
    )
    .unwrap();
    let got = run(vec![out(1.0, "abc")], &redaction);
    assert_eq!(datas(&got), vec!["abc"]);
}

#[test]
fn the_accessors_give_back_the_arguments() {
    let redaction = Redaction::new(find_text("a"), Replacement::Blocks, true).unwrap();
    assert_eq!(redaction.find(), &find_text("a"));
    assert_eq!(redaction.replacement(), &Replacement::Blocks);
    assert!(redaction.includes_input());
}

fn options_json(filter: &AppliedFilter) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for key in [FIND_OPTION, REPLACE_OPTION, INPUT_OPTION] {
        if let Some(value) = filter.options.get(key) {
            map.insert(key.to_string(), value.clone());
        }
    }
    serde_json::Value::Object(map)
}

fn filter_with(options: FilterOptions) -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region: FilterRegion::All,
        options,
    }
}

#[test]
fn the_options_have_the_shape_that_project_files_store() {
    let cases = [
        (
            Redaction::new(find_text("hunter2"), Replacement::Asterisks, false).unwrap(),
            serde_json::json!({"find":{"kind":"text","value":"hunter2"},"replace":{"kind":"asterisks"},"input":false}),
        ),
        (
            Redaction::new(Find::Regex("a+".to_string()), Replacement::Blocks, true).unwrap(),
            serde_json::json!({"find":{"kind":"regex","value":"a+"},"replace":{"kind":"blocks"},"input":true}),
        ),
        (
            text("a", "[REDACTED]"),
            serde_json::json!({"find":{"kind":"text","value":"a"},"replace":{"kind":"text","value":"[REDACTED]"},"input":false}),
        ),
    ];
    for (redaction, expected) in cases {
        let filter = applied(FilterRegion::All, &redaction);
        assert_eq!(filter.kind, kind());
        assert_eq!(options_json(&filter), expected);
    }
}

#[test]
fn redaction_of_reads_back_an_applied_filter() {
    let redaction = Redaction::new(
        Find::Regex("日本+".to_string()),
        Replacement::Text("🎉".to_string()),
        true,
    )
    .unwrap();
    let filter = applied(FilterRegion::All, &redaction);
    assert_eq!(redaction_of(&filter), Some(redaction));
}

#[test]
fn redaction_of_is_none_for_another_kind_or_bad_options() {
    let other = AppliedFilter {
        kind: FilterKind::new("time-dilation"),
        ..applied(FilterRegion::All, &text("a", "b"))
    };
    assert_eq!(redaction_of(&other), None);
    assert_eq!(redaction_of(&filter_with(FilterOptions::new())), None);
}

#[test]
fn a_missing_input_option_means_false() {
    let options = FilterOptions::new()
        .with(FIND_OPTION, serde_json::json!({"kind":"text","value":"a"}))
        .with(REPLACE_OPTION, serde_json::json!({"kind":"asterisks"}));
    let redaction = redaction_of(&filter_with(options)).unwrap();
    assert!(!redaction.includes_input());
}

#[test]
fn a_render_with_a_missing_or_bad_option_fails() {
    let good_find = serde_json::json!({"kind":"text","value":"a"});
    let good_replace = serde_json::json!({"kind":"asterisks"});
    let cases = [
        FilterOptions::new().with(REPLACE_OPTION, good_replace.clone()),
        FilterOptions::new().with(FIND_OPTION, good_find.clone()),
        FilterOptions::new()
            .with(FIND_OPTION, "a")
            .with(REPLACE_OPTION, good_replace.clone()),
        FilterOptions::new()
            .with(FIND_OPTION, good_find)
            .with(REPLACE_OPTION, good_replace)
            .with(INPUT_OPTION, "yes"),
    ];
    for options in cases {
        let result = filters::registry().run(vec![out(1.0, "a")], &[filter_with(options)]);
        match result {
            Err(NleError::RenderError(message)) => {
                assert!(message.contains("filter \"redaction\" failed"), "{message}");
                assert!(message.contains("is missing or is not valid"), "{message}");
            }
            other => panic!("expected a render error, got {other:?}"),
        }
    }
}

#[test]
fn an_invalid_redaction_in_the_options_fails_the_render() {
    let options = FilterOptions::new()
        .with(FIND_OPTION, serde_json::json!({"kind":"text","value":""}))
        .with(REPLACE_OPTION, serde_json::json!({"kind":"asterisks"}));
    let result = filters::registry().run(vec![out(1.0, "a")], &[filter_with(options)]);
    match result {
        Err(NleError::RenderError(message)) => {
            assert!(message.contains("the text to find is empty"), "{message}");
        }
        other => panic!("expected a render error, got {other:?}"),
    }
}

#[test]
fn a_range_redacts_only_inside_the_range() {
    use crate::types::Seconds;
    let region = FilterRegion::Range {
        start: Seconds::new(2.0).unwrap(),
        end: Seconds::new(4.0).unwrap(),
    };
    let events = vec![
        out(1.0, "hunter2"),
        out(3.0, "hunter2"),
        out(5.0, "hunter2"),
    ];
    let got = filters::registry()
        .run(events, &[applied(region, &text("hunter2", "X"))])
        .unwrap();
    assert_eq!(datas(&got), vec!["hunter2", "X", "hunter2"]);
    assert_eq!(
        got.iter().map(|e| e.time).collect::<Vec<_>>(),
        vec![1.0, 3.0, 5.0]
    );
}

fn mask_with(preset: Preset, data: &str) -> String {
    let redaction = Redaction::new(Find::Preset(preset), Replacement::Asterisks, false).unwrap();
    run(vec![out(1.0, data)], &redaction).remove(0).data
}

fn assert_masked(preset: Preset, data: &str) {
    let want = "*".repeat(data.chars().count());
    assert_eq!(mask_with(preset, data), want, "{data}");
}

fn assert_unchanged(preset: Preset, data: &str) {
    assert_eq!(mask_with(preset, data), data);
}

#[test]
fn the_email_preset_masks_an_email_address() {
    assert_masked(Preset::Email, "tim@example.com");
    assert_masked(Preset::Email, "first.last+tag@mail.example.co.uk");
    assert_eq!(
        mask_with(Preset::Email, "mail tim@example.com now"),
        "mail *************** now"
    );
}

#[test]
fn the_email_preset_keeps_text_that_is_not_an_email_address() {
    assert_unchanged(Preset::Email, "user@localhost");
    assert_unchanged(Preset::Email, "a @ b.com");
}

#[test]
fn the_email_preset_keeps_multi_byte_text_around_a_match() {
    assert_eq!(
        mask_with(Preset::Email, "メール: tim@example.com 🎉"),
        "メール: *************** 🎉"
    );
}

#[test]
fn the_presets_are_listed_in_menu_order_with_their_labels() {
    assert_eq!(
        Preset::ALL,
        [
            Preset::Email,
            Preset::IpAddress,
            Preset::AwsKey,
            Preset::Token,
            Preset::Hostname
        ]
    );
    let labels: Vec<&str> = Preset::ALL.iter().map(|p| p.label()).collect();
    assert_eq!(
        labels,
        vec![
            "Email addresses",
            "IP addresses",
            "AWS keys",
            "Tokens",
            "Hostnames"
        ]
    );
}

#[test]
fn each_preset_makes_a_redaction() {
    for preset in Preset::ALL {
        let made = Redaction::new(Find::Preset(preset), Replacement::Asterisks, false);
        assert!(made.is_ok(), "{preset:?}");
    }
}

#[test]
fn a_preset_find_is_written_as_kind_and_value() {
    let json = serde_json::to_value(Find::Preset(Preset::IpAddress)).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"kind": "preset", "value": "ip-address"})
    );
    let back: Find = serde_json::from_value(json).unwrap();
    assert_eq!(back, Find::Preset(Preset::IpAddress));
}

#[test]
fn the_ip_preset_masks_ipv4_and_ipv6_addresses() {
    for address in [
        "10.0.0.1",
        "192.168.1.254",
        "2001:db8::1",
        "fe80::1ff:fe23:4567:890a",
        "2001:0db8:85a3:0000:0000:8a2e:0370:7334",
    ] {
        assert_masked(Preset::IpAddress, address);
    }
}

#[test]
fn the_ip_preset_keeps_text_that_is_not_an_ip_address() {
    for text in [
        "10:42:07",
        "1.2.3",
        "999.1.1.1",
        "v1.2.3.4",
        "aa:bb:cc:dd:ee:ff",
        "std::fs::read",
        "Dec::new",
    ] {
        assert_unchanged(Preset::IpAddress, text);
    }
}

const AWS_SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

/// Mask `value` inside `before` and `after`. Only `value` changes.
fn assert_value_masked(preset: Preset, before: &str, value: &str, after: &str) {
    let got = mask_with(preset, &format!("{before}{value}{after}"));
    let want = format!("{before}{}{after}", "*".repeat(value.chars().count()));
    assert_eq!(got, want);
}

#[test]
fn the_aws_preset_masks_an_access_key_id() {
    assert_masked(Preset::AwsKey, "AKIAIOSFODNN7EXAMPLE");
}

#[test]
fn the_aws_preset_masks_only_the_value_of_a_secret_key() {
    assert_value_masked(Preset::AwsKey, "aws_secret_access_key = ", AWS_SECRET, "");
    assert_value_masked(Preset::AwsKey, "\"SecretAccessKey\": \"", AWS_SECRET, "\"");
    assert_value_masked(
        Preset::AwsKey,
        "AWS_SESSION_TOKEN=",
        "IQoJb3JpZ2luX2VjEXAMPLETOKEN1234567890",
        "",
    );
}

#[test]
fn the_aws_preset_keeps_a_git_sha() {
    assert_unchanged(Preset::AwsKey, "9c12c0325a1b2c3d4e5f60718293a4b5c6d7e8f9");
}

#[test]
fn the_token_preset_masks_known_token_formats() {
    let alnum36 = "aB3dE6gH9jK2mN5pQ8sT1vW4yZ7bC0dE3fGh";
    assert_eq!(alnum36.len(), 36);
    let tokens = [
        format!("ghp_{alnum36}"),
        "github_pat_11ABCDEFG0abcdefghij12".to_string(),
        "xoxb-1234567890-abcdefghij".to_string(),
        "sk-ant-api03-abcdefghij0123456789".to_string(),
        "sk_live_abcdefghij012345".to_string(),
        format!("AIza{}", "abcdefghij0123456789ABCDEFGHIJ-_012"),
        format!("npm_{alnum36}"),
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abc123_-XYZ".to_string(),
    ];
    for token in &tokens {
        assert_masked(Preset::Token, token);
    }
}

#[test]
fn the_token_preset_masks_only_the_token_after_bearer() {
    assert_value_masked(Preset::Token, "Authorization: Bearer ", "abc.def-ghi", "");
}

#[test]
fn the_token_preset_keeps_text_that_is_not_a_token() {
    for text in [
        "risk-assessment-for-the-new-project-x",
        "desk-top",
        "ghp_short",
    ] {
        assert_unchanged(Preset::Token, text);
    }
}

#[test]
fn the_hostname_preset_masks_hostnames() {
    for host in [
        "api.example.com",
        "ip-10-0-0-1.ec2.internal",
        "db.prod.local",
    ] {
        assert_masked(Preset::Hostname, host);
    }
    assert_value_masked(
        Preset::Hostname,
        "https://",
        "github.com",
        "/timmattison/tools",
    );
}

#[test]
fn the_hostname_preset_keeps_file_names_and_single_labels() {
    for text in [
        "main.rs",
        "notes.txt",
        "Cargo.toml",
        "index.html",
        "localhost",
        "v1.2.3",
    ] {
        assert_unchanged(Preset::Hostname, text);
    }
}
