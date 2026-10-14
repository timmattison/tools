//! Tests of the shell definitions: the kind of the shell, the marker, the
//! script, the parse of the output, the read of a real shell, and the resolver.

use std::path::Path;

use wdid::shell::{parse_output, script, Marker, ShellError, ShellKind};

#[test]
fn kind_comes_from_the_file_name() {
    let rows = [
        ("/bin/zsh", Some(ShellKind::Zsh)),
        ("/opt/homebrew/bin/bash", Some(ShellKind::Bash)),
        ("zsh", Some(ShellKind::Zsh)),
        ("/bin/fish", None),
        ("/bin/sh", None),
        ("/usr/bin/zsh5", None),
        ("/home/zsh/fish", None),
        ("", None),
    ];
    for (path, expected) in rows {
        assert_eq!(ShellKind::of(Path::new(path)), expected, "{path:?}");
    }
}

#[test]
fn unique_marker_holds_the_process_id_and_only_safe_characters() {
    let marker = Marker::unique();
    let text = marker.as_str();
    let prefix = format!("__WDID_{}_", std::process::id());
    assert!(text.starts_with(&prefix), "{text}");
    assert!(text.ends_with("__"), "{text}");
    assert!(
        text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "{text}"
    );
}

#[test]
fn new_marker_keeps_its_text() {
    assert_eq!(Marker::new("M_1").as_str(), "M_1");
}

#[test]
#[should_panic(expected = "marker")]
fn new_marker_refuses_a_quote() {
    let _ = Marker::new("a'b");
}

#[test]
#[should_panic(expected = "marker")]
fn new_marker_refuses_an_empty_text() {
    let _ = Marker::new("");
}

#[test]
fn script_holds_the_three_marker_lines_for_each_shell() {
    let marker = Marker::new("MARK_7");
    for kind in [ShellKind::Zsh, ShellKind::Bash] {
        let text = script(kind, &marker);
        assert!(text.contains("'MARK_7 ALIASES'"), "{kind:?}: {text}");
        assert!(
            text.contains("\"MARK_7 FUNCTION $__wdid_function\""),
            "{kind:?}: {text}"
        );
        assert!(text.contains("'MARK_7 END'"), "{kind:?}: {text}");
    }
}

#[test]
fn script_lists_aliases_and_functions_with_the_command_of_each_shell() {
    let marker = Marker::new("M");
    let zsh = script(ShellKind::Zsh, &marker);
    assert!(zsh.contains("alias -L"), "{zsh}");
    assert!(zsh.contains("${(ok)functions}"), "{zsh}");
    assert!(zsh.contains("${functions[$__wdid_function]}"), "{zsh}");
    let bash = script(ShellKind::Bash, &marker);
    assert!(bash.contains("alias -p"), "{bash}");
    assert!(bash.contains("compgen -A function"), "{bash}");
    assert!(
        bash.contains("declare -f -- \"$__wdid_function\" | tail -n +2"),
        "{bash}"
    );
}

/// Wraps alias lines in the markers of the marker `M`, with noise before and
/// after, as an interactive start can print it.
fn framed(alias_lines: &str) -> String {
    format!("welcome to the shell\nM ALIASES\n{alias_lines}M END\nbye\n")
}

/// Parses `stdout` with the marker `M` and gives the definitions.
fn parsed(stdout: &str) -> wdid::shell::Definitions {
    parse_output(stdout, &Marker::new("M")).unwrap()
}

#[test]
fn aliases_are_read_with_quote_removal() {
    let definitions = parsed(&framed(
        "alias ch='crap --here'\nalias -- -='cd -'\nalias _='sudo '\nalias ll=ls\n",
    ));
    let aliases = &definitions.aliases;
    assert_eq!(aliases.get("ch"), Some("crap --here"));
    assert_eq!(aliases.get("-"), Some("cd -"));
    assert_eq!(aliases.get("_"), Some("sudo "));
    assert_eq!(aliases.get("ll"), Some("ls"));
    assert_eq!(aliases.len(), 4);
    assert!(definitions.unparsed_aliases.is_empty());
}

#[test]
fn an_embedded_single_quote_in_an_alias_value_is_read() {
    let definitions = parsed(&framed("alias say='echo it'\\''s'\n"));
    assert_eq!(definitions.aliases.get("say"), Some("echo it's"));
}

#[test]
fn a_global_alias_and_a_suffix_alias_are_skipped() {
    let definitions = parsed(&framed(
        "alias -g ...=../..\nalias -s txt=less\nalias -gL G='| grep'\nalias ok=prcp\n",
    ));
    assert_eq!(definitions.aliases.len(), 1);
    assert_eq!(definitions.aliases.get("ok"), Some("prcp"));
    assert!(definitions.unparsed_aliases.is_empty());
}

#[test]
fn a_value_with_an_equals_sign_splits_at_the_first_one() {
    let definitions = parsed(&framed("alias e='env A=1 B=2'\nalias k=a=b\n"));
    assert_eq!(definitions.aliases.get("e"), Some("env A=1 B=2"));
    assert_eq!(definitions.aliases.get("k"), Some("a=b"));
}

#[test]
fn bash_style_aliases_are_read() {
    let definitions = parsed(&framed("alias ll='ls -l'\nalias wtm='wt --main'\n"));
    assert_eq!(definitions.aliases.get("ll"), Some("ls -l"));
    assert_eq!(definitions.aliases.get("wtm"), Some("wt --main"));
}

#[test]
fn a_multibyte_alias_is_read() {
    let definitions = parsed(&framed("alias 日本='echo 日本語 🎉'\n"));
    assert_eq!(definitions.aliases.get("日本"), Some("echo 日本語 🎉"));
}

#[test]
fn empty_lines_are_skipped() {
    let definitions = parsed(&framed("\nalias a=b\n\n"));
    assert_eq!(definitions.aliases.len(), 1);
    assert!(definitions.unparsed_aliases.is_empty());
}

#[test]
fn an_alias_line_that_does_not_parse_is_kept_in_output_order() {
    let definitions = parsed(&framed("alias a=b\nalias bad='x\nalias c=d\necho $(\n"));
    assert_eq!(definitions.aliases.get("a"), Some("b"));
    assert_eq!(definitions.aliases.get("c"), Some("d"));
    assert_eq!(definitions.aliases.len(), 2);
    assert_eq!(
        definitions.unparsed_aliases,
        vec!["alias bad='x", "echo $("]
    );
}

#[test]
fn a_line_that_is_not_one_alias_command_is_unparsed() {
    let definitions = parsed(&framed("echo hi\nalias a=b; alias c=d\n"));
    assert!(definitions.aliases.is_empty());
    assert_eq!(
        definitions.unparsed_aliases,
        vec!["echo hi", "alias a=b; alias c=d"]
    );
}

#[test]
fn text_before_the_start_marker_and_after_the_end_marker_is_ignored() {
    let stdout = "alias fake=nope\nM ALIASES\nalias real=yes\nM END\nalias late=nope\n";
    let definitions = parsed(stdout);
    assert_eq!(definitions.aliases.len(), 1);
    assert_eq!(definitions.aliases.get("real"), Some("yes"));
}

#[test]
fn output_without_the_start_marker_is_no_markers() {
    let result = parse_output("hello\nM END\n", &Marker::new("M"));
    assert!(matches!(result, Err(ShellError::NoMarkers)), "{result:?}");
}

#[test]
fn output_without_the_end_marker_is_no_markers() {
    let result = parse_output("M ALIASES\nalias a=b\n", &Marker::new("M"));
    assert!(matches!(result, Err(ShellError::NoMarkers)), "{result:?}");
}

#[test]
fn an_end_marker_before_the_start_marker_is_no_markers() {
    let result = parse_output("M END\nM ALIASES\n", &Marker::new("M"));
    assert!(matches!(result, Err(ShellError::NoMarkers)), "{result:?}");
}

#[test]
fn another_marker_text_is_not_a_marker() {
    let result = parse_output("N ALIASES\nalias a=b\nN END\n", &Marker::new("M"));
    assert!(matches!(result, Err(ShellError::NoMarkers)), "{result:?}");
}
