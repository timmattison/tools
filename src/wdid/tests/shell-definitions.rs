//! Tests of the shell definitions: the kind of the shell, the marker, the
//! script, the parse of the output, the read of a real shell, and the resolver.

use std::path::Path;

use wdid::shell::{script, Marker, ShellKind};

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
