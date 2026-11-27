//! Tests of the shell definitions: the kind of the shell, the marker, the
//! script, the parse of the output, the read of a real shell, and the resolver.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use wdid::command_words::{
    command_words, AliasDefinition, AliasKind, Aliases, CommandWord, Parsed,
};
use wdid::origin::{Origin, Places};
use wdid::shell::{
    parse_output, read, script, Definitions, Marker, Resolver, ShellError, ShellKind,
};
use wdid::starts::{Starts, Trigger};
use wdid::tool::{ToolName, ToolNames};

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
    assert!(zsh.contains("${(ok)functions}"), "{zsh}");
    assert!(zsh.contains("${functions[$__wdid_function]}"), "{zsh}");
    // `alias -L` does not list the suffix aliases, so `alias -sL` follows it.
    let regular_and_global = zsh.find("alias -L\n").expect(&zsh);
    let suffix = zsh.find("alias -sL\n").expect(&zsh);
    let autoload = zsh.find("autoload +X -m '[^_]*'").expect(&zsh);
    let loop_start = zsh.find("for __wdid_function").expect(&zsh);
    assert!(regular_and_global < suffix, "{zsh}");
    assert!(suffix < autoload, "{zsh}");
    assert!(autoload < loop_start, "{zsh}");
    let bash = script(ShellKind::Bash, &marker);
    assert!(!bash.contains("autoload"), "{bash}");
    assert!(!bash.contains("alias -s"), "{bash}");
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
fn a_global_alias_and_a_suffix_alias_are_read_with_their_kind() {
    let definitions = parsed(&framed(
        "alias -g ...=../..\nalias -s txt=less\nalias -gL G='| grep'\nalias ok=prcp\n",
    ));
    let aliases = &definitions.aliases;
    assert_eq!(aliases.len(), 4);
    assert_eq!(aliases.global("..."), Some("../.."));
    assert_eq!(aliases.global("G"), Some("| grep"));
    assert_eq!(aliases.get("G"), Some("| grep"));
    assert_eq!(aliases.global("ok"), None);
    assert_eq!(aliases.get("ok"), Some("prcp"));
    assert_eq!(aliases.suffix_of("notes.txt"), Some(("txt", "less")));
    assert_eq!(aliases.get("txt"), None);
    assert!(definitions.unparsed_aliases.is_empty());
}

#[test]
fn an_alias_line_that_is_global_and_suffix_is_unparsed() {
    let definitions = parsed(&framed("alias -gs md=glow\nalias -g -s G=x\n"));
    assert!(definitions.aliases.is_empty());
    assert_eq!(
        definitions.unparsed_aliases,
        ["alias -gs md=glow", "alias -g -s G=x"]
    );
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

/// Wraps an alias listing and function sections in the markers of `M`.
fn with_functions(alias_lines: &str, sections: &[(&str, &str)]) -> String {
    let mut text = format!("noise\nM ALIASES\n{alias_lines}");
    for (name, body) in sections {
        text.push_str(&format!("M FUNCTION {name}\n{body}\n"));
    }
    text.push_str("M END\nnoise\n");
    text
}

#[test]
fn a_function_body_gives_its_command_words() {
    let definitions = parsed(&with_functions(
        "alias a=b\n",
        &[("gitclean", "\tgitnuke --safe \"$@\"")],
    ));
    assert_eq!(
        definitions.functions.get("gitclean"),
        Some(&vec![CommandWord::new("gitnuke", ["--safe", "\"$@\""])])
    );
    assert_eq!(definitions.functions.len(), 1);
    assert_eq!(definitions.aliases.len(), 1);
    assert!(definitions.unparsed_functions.is_empty());
}

#[test]
fn the_body_of_a_function_is_kept_as_the_shell_gave_it() {
    let definitions = parsed(&with_functions(
        "",
        &[("gitclean", "\tgitnuke --safe \"$@\"\n\tprcp")],
    ));
    assert_eq!(
        definitions
            .function_bodies
            .get("gitclean")
            .map(String::as_str),
        Some("\tgitnuke --safe \"$@\"\n\tprcp")
    );
}

#[test]
fn the_body_of_a_function_that_does_not_parse_is_kept_too() {
    let definitions = parsed(&with_functions("", &[("zed", "\techo $(")]));
    assert_eq!(definitions.unparsed_functions, ["zed"]);
    assert_eq!(
        definitions.function_bodies.get("zed").map(String::as_str),
        Some("\techo $(")
    );
}

#[test]
fn a_function_body_of_many_lines_is_read_as_one_script() {
    let body = "\tif [[ -n $1 ]]; then\n\t\tcwt \"$1\"\n\telse\n\t\tcat x | prhash\n\tfi";
    let definitions = parsed(&with_functions("", &[("wt", body)]));
    let words: Vec<&str> = definitions.functions["wt"]
        .iter()
        .map(|word| word.word.as_str())
        .collect();
    assert_eq!(words, ["cwt", "cat", "prhash"]);
}

#[test]
fn many_functions_are_kept_apart() {
    let definitions = parsed(&with_functions(
        "",
        &[("a", "\tprcp x"), ("b", "\tcwt"), ("c", "")],
    ));
    assert_eq!(
        definitions
            .functions
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(
        definitions.functions["a"],
        vec![CommandWord::new("prcp", ["x"])]
    );
    assert_eq!(
        definitions.functions["b"],
        vec![CommandWord::new("cwt", [] as [&str; 0])]
    );
    assert!(definitions.functions["c"].is_empty());
}

#[test]
fn a_function_body_is_not_read_as_aliases() {
    let definitions = parsed(&with_functions("", &[("f", "\talias x=y")]));
    assert!(definitions.aliases.is_empty());
    assert!(definitions.unparsed_aliases.is_empty());
    assert_eq!(
        definitions.functions["f"],
        vec![CommandWord::new("alias", ["x=y"])]
    );
}

#[test]
fn aliases_are_not_expanded_in_a_function_body() {
    let definitions = parsed(&with_functions("alias ll='ls -l'\n", &[("f", "\tll x")]));
    assert_eq!(
        definitions.functions["f"],
        vec![CommandWord::new("ll", ["x"])]
    );
}

#[test]
fn a_function_body_that_does_not_parse_is_listed_by_name_in_sorted_order() {
    let definitions = parsed(&with_functions(
        "",
        &[("zed", "\techo $("), ("ok", "\tprcp"), ("alpha", "\ta |")],
    ));
    assert_eq!(definitions.unparsed_functions, ["alpha", "zed"]);
    assert_eq!(definitions.functions.keys().collect::<Vec<_>>(), ["ok"]);
}

#[test]
fn a_multibyte_function_name_and_body_are_read() {
    let definitions = parsed(&with_functions("", &[("日本", "\tprcp 日本語.txt 🎉/")]));
    assert_eq!(
        definitions.functions["日本"],
        vec![CommandWord::new("prcp", ["日本語.txt", "🎉/"])]
    );
}

#[test]
fn functions_after_the_end_marker_are_ignored() {
    let stdout = "M ALIASES\nM FUNCTION a\n\tprcp\nM END\nM FUNCTION b\n\tcwt\n";
    let definitions = parsed(stdout);
    assert_eq!(definitions.functions.keys().collect::<Vec<_>>(), ["a"]);
}

/// Writes an executable stub shell into a new temporary directory.
fn stub(body: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stub-shell");
    gitscratch::testing::executable::write_executable(&path, format!("#!/bin/sh\n{body}\n"))
        .unwrap();
    (dir, path)
}

#[test]
fn a_shell_that_runs_too_long_is_timed_out_and_killed() {
    let (_dir, path) = stub("exec sleep 60");
    let started = std::time::Instant::now();
    let result = read(
        childenv::fixture_command(&path),
        ShellKind::Zsh,
        Duration::from_secs(1),
    );
    assert!(
        matches!(result, Err(ShellError::TimedOut { seconds: 1 })),
        "{result:?}"
    );
    // A broken kill waits for the 60 s sleep. The bound is half of that, so a
    // busy machine still passes and a broken kill still fails.
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_shell_that_exits_with_a_failure_is_failed_with_its_stderr() {
    let (_dir, path) = stub("echo 'bad config' >&2\nexit 3");
    let result = read(
        childenv::fixture_command(&path),
        ShellKind::Zsh,
        Duration::from_secs(10),
    );
    match result {
        Err(ShellError::Failed { status, stderr }) => {
            assert!(status.contains('3'), "{status}");
            assert_eq!(stderr, "bad config");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_shell_that_prints_no_markers_is_no_markers() {
    let (_dir, path) = stub("echo hello");
    let result = read(
        childenv::fixture_command(&path),
        ShellKind::Bash,
        Duration::from_secs(10),
    );
    assert!(matches!(result, Err(ShellError::NoMarkers)), "{result:?}");
}

#[test]
fn a_shell_that_does_not_exist_is_a_spawn_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no-such-shell");
    let result = read(
        childenv::fixture_command(&path),
        ShellKind::Zsh,
        Duration::from_secs(10),
    );
    match result {
        Err(ShellError::Spawn { shell, source }) => {
            assert_eq!(shell, path.to_string_lossy());
            assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_shell_gets_the_interactive_flag_and_the_script_with_stdin_closed() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record");
    let (_stub_dir, path) = stub(&format!(
        "printf '%s\\n' \"$1\" \"$2\" \"$#\" > {quoted}\nif read -r line; then echo \"stdin: $line\" >&2; fi\nprintf '%s' \"$3\" >> {quoted}\nexit 4",
        quoted = shellquote::shell_quote(&record.to_string_lossy())
    ));
    let result = read(
        childenv::fixture_command(&path),
        ShellKind::Zsh,
        Duration::from_secs(10),
    );
    match result {
        Err(ShellError::Failed { stderr, .. }) => assert_eq!(stderr, ""),
        other => panic!("{other:?}"),
    }
    let text = std::fs::read_to_string(&record).unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("-i"));
    assert_eq!(lines.next(), Some("-c"));
    assert_eq!(lines.next(), Some("3"));
    let script_text: Vec<&str> = lines.collect();
    let first = script_text.first().copied().unwrap_or_default();
    assert!(first.starts_with("print -r -- '__WDID_"), "{first}");
}

/// A shell error names no flag, because `wdid` and `wdid defs` both give it,
/// and only the tools report has `--no-shell`. The tools report adds the flag
/// itself.
#[test]
fn no_error_message_names_a_flag_of_one_report() {
    let errors = [
        ShellError::NotSet,
        ShellError::NotSupported {
            shell: "/bin/fish".to_owned(),
        },
        ShellError::Spawn {
            shell: "zsh".to_owned(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        },
        ShellError::TimedOut { seconds: 30 },
        ShellError::Failed {
            status: "exit status: 3".to_owned(),
            stderr: "boom".to_owned(),
        },
        ShellError::NoMarkers,
        ShellError::Zdotdir {
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        },
    ];
    let texts: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        texts,
        [
            "SHELL is not set, so wdid cannot read the aliases and functions",
            "wdid reads aliases and functions from zsh and bash only, and SHELL is /bin/fish",
            "cannot start zsh: entity not found",
            "the shell did not stop in 30 seconds",
            "the shell failed (exit status: 3): boom",
            "the shell output does not hold the start and end markers of wdid",
            "cannot make the temporary ZDOTDIR for zsh: permission denied",
        ]
    );
}

/// Finds a shell program for the real-shell tests: the fixed path first, then
/// the name on the PATH. Gives `None` when the shell is not installed.
fn find_shell(name: &str) -> Option<PathBuf> {
    let fixed = PathBuf::from("/bin").join(name);
    if fixed.exists() {
        return Some(fixed);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Makes a command for a real shell with `HOME` and `ZDOTDIR` in `home`.
fn real_shell(path: &Path, home: &Path) -> Command {
    let mut command = childenv::fixture_command(path);
    command
        .env("HOME", home)
        .env("ZDOTDIR", home)
        .env("TERM", "dumb");
    command
}

/// A process that `.zshrc` starts in the background keeps the pipes of the
/// shell open after the shell ends. The output is complete when the shell
/// ends, so `read` gives it after a short grace and does not wait for that
/// process. Before, `read` gave `TimedOut` after the grace and lost the
/// output.
#[test]
fn a_background_process_that_holds_the_pipes_does_not_lose_the_output() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".zshrc"),
        "alias wtm='wt --main'\n( sleep 60 & )\n",
    )
    .unwrap();
    let started = std::time::Instant::now();

    let definitions = read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    );

    let elapsed = started.elapsed();
    assert_eq!(definitions.unwrap().aliases.get("wtm"), Some("wt --main"));
    // A broken wait for the pipes lasts 60 s, the life of the sleep. The bound
    // is half of that, so a busy machine still passes and a broken wait fails.
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
}

#[test]
fn a_real_zsh_gives_its_aliases_and_functions() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let big_body: String = (0..4000)
        .map(|n| format!("\techo line {n} with some padding to make the output large\n"))
        .collect();
    std::fs::write(
        home.path().join(".zshrc"),
        format!(
            "alias wtm='wt --main'\nwt() {{ cwt \"$@\" }}\ngitclean() {{ gitnuke --safe \"$@\" }}\nbig() {{\n{big_body}}}\n"
        ),
    )
    .unwrap();
    let definitions = read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(definitions.aliases.get("wtm"), Some("wt --main"));
    assert_eq!(
        definitions.functions.get("wt"),
        Some(&vec![CommandWord::new("cwt", ["\"$@\""])])
    );
    assert_eq!(
        definitions.functions.get("gitclean"),
        Some(&vec![CommandWord::new("gitnuke", ["--safe", "\"$@\""])])
    );
    assert_eq!(definitions.functions["big"].len(), 4000);
    assert!(
        definitions.unparsed_aliases.is_empty(),
        "{:?}",
        definitions.unparsed_aliases
    );
}

/// Gives the first line of the function `name` in a parse of one function.
fn first_line_of(body: &str) -> String {
    let definitions = parsed(&with_functions("", &[("f", body)]));
    assert_eq!(definitions.function_lines.len(), 1);
    definitions.function_lines["f"].clone()
}

#[test]
fn the_first_line_of_a_zsh_body_loses_its_tab() {
    assert_eq!(first_line_of("\tcwt \"$@\""), "cwt \"$@\"");
}

#[test]
fn the_first_line_of_a_body_of_many_lines_is_its_first_line() {
    assert_eq!(
        first_line_of("\tif [[ -n $1 ]]; then\n\t\tcwt \"$1\"\n\tfi"),
        "if [[ -n $1 ]]; then"
    );
}

#[test]
fn the_first_line_of_a_bash_body_skips_the_opening_brace() {
    assert_eq!(first_line_of("{ \n    echo hi\n}"), "echo hi");
}

#[test]
fn the_first_line_skips_blank_lines() {
    assert_eq!(first_line_of("\n\t\n\tprcp x"), "prcp x");
}

#[test]
fn an_empty_body_has_an_empty_first_line_and_an_entry() {
    let definitions = parsed(&with_functions("", &[("f", "")]));
    assert_eq!(definitions.function_lines.get("f"), Some(&String::new()));
}

#[test]
fn the_first_line_of_a_body_that_did_not_parse_is_kept() {
    let definitions = parsed(&with_functions("", &[("f", "\techo $(")]));
    assert_eq!(definitions.unparsed_functions, vec!["f".to_owned()]);
    assert_eq!(definitions.function_lines["f"], "echo $(");
}

#[test]
fn the_first_line_keeps_multibyte_text() {
    assert_eq!(
        first_line_of("\tprcp 日本語.txt 🎉/"),
        "prcp 日本語.txt 🎉/"
    );
}

/// A real zsh gives the first line of each function and the kind of each
/// alias. It can define more aliases than the test does.
#[test]
fn a_real_zsh_gives_the_first_lines_and_the_kinds() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".zshrc"),
        "wt() { cwt \"$@\" }\nalias -g PH='| prhash'\nalias -s md=glow\nalias wtm='wt --main'\n",
    )
    .unwrap();
    let definitions = read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(definitions.function_lines["wt"], "cwt \"$@\"");
    let listed = definitions.aliases.definitions();
    for (name, kind, value) in [
        ("PH", AliasKind::Global, "| prhash"),
        ("*.md", AliasKind::Suffix, "glow"),
        ("wtm", AliasKind::Regular, "wt --main"),
    ] {
        assert!(
            listed
                .iter()
                .any(|a| a.name == name && a.kind == kind && a.value == value),
            "{name}: {listed:?}"
        );
    }
}

/// A real zsh gives its global aliases through `alias -L` and its suffix
/// aliases through `alias -sL`. Both go into the table with their kind.
#[test]
fn a_real_zsh_gives_its_global_and_suffix_aliases() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".zshrc"),
        "alias -g PH='| prhash'\nalias -s md=glow\nalias wtm='wt --main'\n",
    )
    .unwrap();
    let definitions = read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    let aliases = &definitions.aliases;
    assert_eq!(aliases.global("PH"), Some("| prhash"));
    assert_eq!(aliases.suffix_of("README.md"), Some(("md", "glow")));
    assert_eq!(aliases.get("wtm"), Some("wt --main"));
    assert_eq!(aliases.global("wtm"), None);
    assert!(
        definitions.unparsed_aliases.is_empty(),
        "{:?}",
        definitions.unparsed_aliases
    );
}

/// An autoloaded zsh function is a stub until the shell loads it. The script
/// loads the stubs first, so the body counts. A name that starts with `_` is a
/// completion function and stays a stub.
#[test]
fn an_autoloaded_zsh_function_counts_through_its_body() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join("functions");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("wdidfn"), "prcp \"$@\"\n").unwrap();
    std::fs::write(directory.join("_wdidcomp"), "prhash\n").unwrap();
    std::fs::write(
        home.path().join(".zshrc"),
        format!(
            "fpath=({} $fpath)\nautoload -Uz wdidfn _wdidcomp\n",
            shellquote::shell_quote(&directory.to_string_lossy())
        ),
    )
    .unwrap();
    let definitions = read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(
        definitions.functions["wdidfn"],
        vec![CommandWord::new("prcp", ["\"$@\""])]
    );
    assert!(
        definitions.functions["_wdidcomp"]
            .iter()
            .all(|word| word.word != "prhash"),
        "{:?}",
        definitions.functions["_wdidcomp"]
    );
    assert!(!definitions
        .unparsed_functions
        .contains(&"wdidfn".to_string()));
}

/// `autoload +X` exits non-zero when a stub cannot load. A `.zshrc` with
/// `ERR_EXIT` must not stop the script because of that.
#[test]
fn an_autoload_stub_that_cannot_load_does_not_stop_the_script() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".zshrc"),
        "setopt ERR_EXIT\nalias wtm='wt --main'\nwt() { cwt \"$@\" }\nautoload -Uz wdidmissing\n",
    )
    .unwrap();
    let definitions = read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(definitions.aliases.get("wtm"), Some("wt --main"));
    assert_eq!(
        definitions.functions["wt"],
        vec![CommandWord::new("cwt", ["\"$@\""])]
    );
}

#[test]
fn a_real_bash_gives_its_aliases_and_functions() {
    let Some(bash) = find_shell("bash") else {
        eprintln!("skipped: bash is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".bashrc"),
        "alias wtm='wt --main'\nwt() { cwt \"$@\"; }\ngitclean() { gitnuke --safe \"$@\"; }\n",
    )
    .unwrap();
    let definitions = read(
        real_shell(&bash, home.path()),
        ShellKind::Bash,
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(definitions.aliases.get("wtm"), Some("wt --main"));
    let wt: Vec<&str> = definitions.functions["wt"]
        .iter()
        .map(|w| w.word.as_str())
        .collect();
    assert_eq!(wt, ["cwt"]);
    let gitclean: Vec<&str> = definitions.functions["gitclean"]
        .iter()
        .map(|w| w.word.as_str())
        .collect();
    assert_eq!(gitclean, ["gitnuke"]);
}

/// The tools of the fixture workspace.
fn tools() -> ToolNames {
    ["prcp", "cwt", "crap", "prhash"]
        .into_iter()
        .map(ToolName::new)
        .collect()
}

/// Makes definitions from function names and bodies. Each body is parsed with
/// no aliases, as the shell module parses it.
fn functions(rows: &[(&str, &str)]) -> Definitions {
    let mut definitions = Definitions::default();
    for (name, body) in rows {
        let Parsed::Words(words) = command_words(body, &Aliases::new()) else {
            panic!("the body of {name} does not parse: {body}");
        };
        definitions.functions.insert((*name).to_owned(), words);
    }
    definitions
}

/// Gives the names of the tools of a function, or `None` for a name that is
/// not a function.
fn tools_of(resolver: &Resolver, name: &str) -> Option<Vec<String>> {
    resolver
        .tools_of(name)
        .map(|set| set.iter().map(ToString::to_string).collect())
}

#[test]
fn a_function_resolves_to_the_tool_that_it_calls() {
    let resolver = Resolver::new(&functions(&[("wt", "cwt \"$@\"")]), &tools());
    assert_eq!(tools_of(&resolver, "wt"), Some(vec!["cwt".to_owned()]));
}

#[test]
fn a_function_that_calls_a_function_resolves_to_the_tools_of_both() {
    let definitions = functions(&[("wt", "cwt"), ("wtf", "wt -f\nprcp x")]);
    let resolver = Resolver::new(&definitions, &tools());
    assert_eq!(tools_of(&resolver, "wt"), Some(vec!["cwt".to_owned()]));
    assert_eq!(
        tools_of(&resolver, "wtf"),
        Some(vec!["cwt".to_owned(), "prcp".to_owned()])
    );
}

#[test]
fn a_chain_of_functions_resolves_to_any_depth() {
    let definitions = functions(&[("a", "b"), ("b", "c"), ("c", "d"), ("d", "cwt")]);
    let resolver = Resolver::new(&definitions, &tools());
    for name in ["a", "b", "c", "d"] {
        assert_eq!(
            tools_of(&resolver, name),
            Some(vec!["cwt".to_owned()]),
            "{name}"
        );
    }
}

#[test]
fn a_function_that_calls_two_tools_resolves_to_both() {
    let resolver = Resolver::new(
        &functions(&[("both", "prcp a && cat x | prhash")]),
        &tools(),
    );
    assert_eq!(
        tools_of(&resolver, "both"),
        Some(vec!["prcp".to_owned(), "prhash".to_owned()])
    );
}

#[test]
fn a_cycle_of_functions_stops_and_keeps_every_tool() {
    let definitions = functions(&[("a", "b"), ("b", "a\nprcp")]);
    let resolver = Resolver::new(&definitions, &tools());
    assert_eq!(tools_of(&resolver, "a"), Some(vec!["prcp".to_owned()]));
    assert_eq!(tools_of(&resolver, "b"), Some(vec!["prcp".to_owned()]));
}

#[test]
fn a_function_that_calls_itself_resolves() {
    let resolver = Resolver::new(&functions(&[("loop", "loop\ncwt")]), &tools());
    assert_eq!(tools_of(&resolver, "loop"), Some(vec!["cwt".to_owned()]));
}

#[test]
fn a_function_named_like_a_tool_that_calls_the_tool_through_command() {
    let definitions = functions(&[("crap", "command crap --here \"$@\"")]);
    let resolver = Resolver::new(&definitions, &tools());
    assert_eq!(tools_of(&resolver, "crap"), Some(vec!["crap".to_owned()]));
}

#[test]
fn a_tool_in_a_command_substitution_of_an_assignment_counts() {
    let resolver = Resolver::new(&functions(&[("f", "x=$(cwt)")]), &tools());
    assert_eq!(tools_of(&resolver, "f"), Some(vec!["cwt".to_owned()]));
}

#[test]
fn a_development_run_does_not_count() {
    let definitions = functions(&[("f", "./target/release/prcp x\ncargo run --bin cwt")]);
    let resolver = Resolver::new(&definitions, &tools());
    assert_eq!(tools_of(&resolver, "f"), Some(Vec::new()));
}

#[test]
fn a_function_without_a_tool_has_an_empty_entry() {
    let resolver = Resolver::new(&functions(&[("ll", "ls -l"), ("nothing", "")]), &tools());
    assert_eq!(tools_of(&resolver, "ll"), Some(Vec::new()));
    assert_eq!(tools_of(&resolver, "nothing"), Some(Vec::new()));
}

#[test]
fn a_name_that_is_not_a_function_has_no_entry() {
    let resolver = Resolver::new(&functions(&[("wt", "cwt")]), &tools());
    assert_eq!(tools_of(&resolver, "cwt"), None);
    assert_eq!(tools_of(&resolver, "missing"), None);
}

#[test]
fn a_function_that_did_not_parse_has_no_entry_and_adds_no_tool() {
    let mut definitions = functions(&[("caller", "broken\nprcp")]);
    definitions.unparsed_functions.push("broken".to_owned());
    let resolver = Resolver::new(&definitions, &tools());
    assert_eq!(tools_of(&resolver, "broken"), None);
    assert_eq!(tools_of(&resolver, "caller"), Some(vec!["prcp".to_owned()]));
}

#[test]
fn an_empty_resolver_has_no_entry() {
    let resolver = Resolver::new(&Definitions::default(), &tools());
    assert_eq!(tools_of(&resolver, "wt"), None);
}

/// Wraps lines in the markers of `M`, with noise before the start marker.
fn records(lines: &[&str]) -> String {
    let mut text = String::from("noise\nM ALIASES\n");
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("M END\n");
    text
}

/// Makes an origin in `file`, at `line` when it is given.
fn origin(file: impl Into<PathBuf>, line: Option<u32>) -> Origin {
    Origin {
        file: file.into(),
        line,
    }
}

/// Gives the alias `name` of the kind `kind` in `definitions`.
fn alias_named(definitions: &Definitions, name: &str, kind: AliasKind) -> AliasDefinition {
    definitions
        .aliases
        .definitions()
        .into_iter()
        .find(|alias| alias.name == name && alias.kind == kind)
        .unwrap_or_else(|| panic!("no {kind:?} alias {name}"))
}

/// Gives the origin of the alias `name` of the kind `kind`.
fn alias_origin(definitions: &Definitions, name: &str, kind: AliasKind) -> Option<Origin> {
    definitions
        .alias_origin(&alias_named(definitions, name, kind))
        .cloned()
}

#[test]
fn the_places_of_the_shell_are_read() {
    let definitions = parsed(&records(&[
        "alias a=b",
        "M HOME /home/me",
        "M ZSH /home/me/.oh-my-zsh",
        "M ZSH_CUSTOM /home/me/.oh-my-zsh/custom",
    ]));
    assert_eq!(
        definitions.places,
        Places {
            home: Some(PathBuf::from("/home/me")),
            zsh: Some(PathBuf::from("/home/me/.oh-my-zsh")),
            zsh_custom: Some(PathBuf::from("/home/me/.oh-my-zsh/custom")),
        }
    );
    assert_eq!(definitions.aliases.len(), 1);
    assert!(definitions.unparsed_aliases.is_empty());
}

#[test]
fn an_empty_or_relative_place_is_no_place() {
    let definitions = parsed(&records(&[
        "M HOME /home/me",
        "M ZSH ",
        "M ZSH_CUSTOM custom",
    ]));
    assert_eq!(definitions.places.home, Some(PathBuf::from("/home/me")));
    assert_eq!(definitions.places.zsh, None);
    assert_eq!(definitions.places.zsh_custom, None);
    assert!(definitions.unparsed_aliases.is_empty());
}

/// A suffix alias has a namespace of its own, thus the origin of the suffix
/// alias `md` is not the origin of the regular alias `md`.
#[test]
fn the_origin_of_an_alias_is_read_for_its_kind() {
    let definitions = parsed(&records(&[
        "alias wt=cwt",
        "alias -g G='| grep'",
        "alias -s md=glow",
        "alias md=other",
        "M ALIAS_ORIGIN wt",
        "M ORIGIN /home/me/.zshrc:3",
        "M ALIAS_ORIGIN G",
        "M ORIGIN /home/me/.zshrc:4",
        "M SUFFIX_ORIGIN md",
        "M ORIGIN /home/me/.zshrc:5",
    ]));
    assert_eq!(
        alias_origin(&definitions, "wt", AliasKind::Regular),
        Some(origin("/home/me/.zshrc", Some(3)))
    );
    assert_eq!(
        alias_origin(&definitions, "G", AliasKind::Global),
        Some(origin("/home/me/.zshrc", Some(4)))
    );
    assert_eq!(
        alias_origin(&definitions, "*.md", AliasKind::Suffix),
        Some(origin("/home/me/.zshrc", Some(5)))
    );
    assert_eq!(alias_origin(&definitions, "md", AliasKind::Regular), None);
    assert!(definitions.unparsed_aliases.is_empty());
}

/// A name that two files define has two places, in the order of the
/// definitions. A place that is not a file stays in the list as `None`.
#[test]
fn each_place_of_an_alias_is_read_in_order_and_for_its_kind() {
    let definitions = parsed(&records(&[
        "alias ll='ls -la'",
        "alias ev=true",
        "alias -s md=less",
        "alias nop=true",
        "M ALIAS_PLACE ll",
        "M ORIGIN /home/me/.zshrc:1",
        "M ALIAS_PLACE ll",
        "M ORIGIN /home/me/more.zsh:1",
        "M ALIAS_PLACE ev",
        "M ORIGIN (eval):1",
        "M SUFFIX_PLACE md",
        "M ORIGIN /home/me/.zshrc:3",
        "M SUFFIX_PLACE md",
        "M ORIGIN /home/me/.zshrc:4",
    ]));
    let places = |name: &str, kind: AliasKind| {
        definitions
            .alias_places(&alias_named(&definitions, name, kind))
            .to_vec()
    };
    assert_eq!(
        places("ll", AliasKind::Regular),
        [
            Some(origin("/home/me/.zshrc", Some(1))),
            Some(origin("/home/me/more.zsh", Some(1))),
        ]
    );
    assert_eq!(places("ev", AliasKind::Regular), [None]);
    assert_eq!(
        places("*.md", AliasKind::Suffix),
        [
            Some(origin("/home/me/.zshrc", Some(3))),
            Some(origin("/home/me/.zshrc", Some(4))),
        ]
    );
    assert!(places("nop", AliasKind::Regular).is_empty());
    assert_eq!(definitions.alias_places["ll"].len(), 2);
    assert_eq!(definitions.suffix_places["md"].len(), 2);
    assert!(!definitions.alias_places.contains_key("md"));
}

/// The lines of a real zsh that define aliases in more than one place.
const PLACES_ZSHRC: &str = "alias ll='ls -l'\n\
source $HOME/more.zsh\n\
alias -s md=cat\n\
alias -s md=less\n\
eval \"alias ev=true\"\n";

#[test]
fn a_real_zsh_gives_each_place_of_an_alias() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    write_file(home.path(), ".zshrc", PLACES_ZSHRC);
    write_file(home.path(), "more.zsh", "alias ll='ls -la'\n");
    let definitions = read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    let zshrc = home.path().join(".zshrc");
    assert_eq!(
        definitions.alias_places["ll"],
        [
            Some(origin(&zshrc, Some(1))),
            Some(origin(home.path().join("more.zsh"), Some(1))),
        ]
    );
    assert_eq!(
        definitions.suffix_places["md"],
        [Some(origin(&zshrc, Some(3))), Some(origin(&zshrc, Some(4)))]
    );
    assert_eq!(definitions.alias_places["ev"], [None]);
}

#[test]
fn an_alias_without_a_known_origin_has_no_origin() {
    let definitions = parsed(&records(&[
        "alias a=b",
        "alias c=d",
        "M ALIAS_ORIGIN c",
        "M ORIGIN (eval):1",
    ]));
    assert_eq!(alias_origin(&definitions, "a", AliasKind::Regular), None);
    assert_eq!(alias_origin(&definitions, "c", AliasKind::Regular), None);
}

#[test]
fn the_origin_of_a_function_is_not_a_line_of_its_body() {
    let stdout = "M ALIASES\nM FUNCTION wt\nM ORIGIN /home/me/.zshrc\n\tcwt \"$@\"\n\
                  M FUNCTION f\nM ORIGIN \n\tprcp\nM END\n";
    let definitions = parsed(stdout);
    assert_eq!(
        definitions.function_origin("wt"),
        Some(&origin("/home/me/.zshrc", None))
    );
    assert_eq!(definitions.function_origin("f"), None);
    assert_eq!(
        definitions.functions["wt"],
        vec![CommandWord::new("cwt", ["\"$@\""])]
    );
    assert_eq!(definitions.function_lines["wt"], "cwt \"$@\"");
    assert_eq!(
        definitions.functions["f"],
        vec![CommandWord::new("prcp", [] as [&str; 0])]
    );
}

#[test]
fn an_origin_before_each_header_is_ignored() {
    let definitions = parsed(&records(&["M ORIGIN /home/me/.zshrc:1", "alias a=b"]));
    assert_eq!(alias_origin(&definitions, "a", AliasKind::Regular), None);
    assert_eq!(definitions.aliases.len(), 1);
    assert!(definitions.unparsed_aliases.is_empty());
}

/// The wrapper function `alias` must not be in the listing, and the builtin
/// lists the aliases.
#[test]
fn the_zsh_script_removes_the_alias_function_before_it_lists_the_aliases() {
    let zsh = script(ShellKind::Zsh, &Marker::new("M"));
    let remove = zsh.find("unfunction alias").expect(&zsh);
    let list = zsh.find("alias -L\n").expect(&zsh);
    assert!(remove < list, "{zsh}");
}

/// Writes `text` into the file `relative` under `home`, and makes its
/// directories.
fn write_file(home: &Path, relative: &str, text: &str) {
    let path = home.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The `.zshrc` of a home with a stand-in of oh-my-zsh in `$HOME/omz`. Each
/// line defines one thing that the origin tests read.
const OMZ_ZSHRC: &str = "export ZSH=$HOME/omz\n\
ZSH_CUSTOM=$ZSH/custom\n\
alias wt='cwt'\n\
source $HOME/sourced.zsh\n\
source $ZSH/plugins/git/git.plugin.zsh\n\
source $ZSH_CUSTOM/mine.zsh\n\
eval 'evalfn() { prcp \"$@\" }'\n\
source <(print -r -- 'procfn() { prhash }')\n\
builtin alias sneaky=ls\n\
myfn() { cwt }\n\
alias -g G='| grep'\n\
alias -s md=glow\n\
alias -- -='cd -'\n\
source <(print -r -- 'alias procalias=y')\n";

/// Makes a home with [`OMZ_ZSHRC`], the file that it sources, a plugin of the
/// stand-in of oh-my-zsh, and a file in its custom directory.
fn omz_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    write_file(home.path(), ".zshrc", OMZ_ZSHRC);
    write_file(home.path(), "sourced.zsh", "alias fromsourced='echo s'\n");
    write_file(
        home.path(),
        "omz/plugins/git/git.plugin.zsh",
        "alias gst='git status'\ngfun() { git log }\n",
    );
    write_file(
        home.path(),
        "omz/custom/mine.zsh",
        "alias cust='echo custom'\n",
    );
    home
}

/// Reads the definitions of a real zsh in [`omz_home`]. Gives `None` when zsh
/// is not installed.
fn omz_definitions() -> Option<(tempfile::TempDir, Definitions)> {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return None;
    };
    let home = omz_home();
    let definitions = read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    Some((home, definitions))
}

#[test]
fn a_real_zsh_gives_its_places_after_its_start() {
    let Some((home, definitions)) = omz_definitions() else {
        return;
    };
    assert_eq!(
        definitions.places,
        Places {
            home: Some(home.path().to_path_buf()),
            zsh: Some(home.path().join("omz")),
            zsh_custom: Some(home.path().join("omz/custom")),
        }
    );
}

#[test]
fn a_function_in_zshrc_has_zshrc_as_its_origin_and_is_mine() {
    let Some((home, definitions)) = omz_definitions() else {
        return;
    };
    let found = definitions.function_origin("myfn");
    assert_eq!(found, Some(&origin(home.path().join(".zshrc"), None)));
    assert!(definitions.places.is_mine(found));
}

/// A function that an `eval` in `.zshrc` makes is mine, because my file chose
/// to run the `eval`.
#[test]
fn a_function_from_an_eval_has_the_file_of_the_eval_and_is_mine() {
    let Some((home, definitions)) = omz_definitions() else {
        return;
    };
    let found = definitions.function_origin("evalfn");
    assert_eq!(found, Some(&origin(home.path().join(".zshrc"), None)));
    assert!(definitions.places.is_mine(found));
}

/// The directory of the file that zsh gives a process substitution, `<(…)`.
/// The build of zsh picks `/proc/self/fd` where it works, as on Linux, and
/// else `/dev/fd`, as on macOS.
const PROCESS_SUBSTITUTION_DIRECTORY: &str = if cfg!(target_os = "linux") {
    "/proc/self/fd"
} else {
    "/dev/fd"
};

#[test]
fn a_function_from_a_process_substitution_is_in_the_descriptor_directory_and_is_not_mine() {
    let Some((_home, definitions)) = omz_definitions() else {
        return;
    };
    let found = definitions.function_origin("procfn").expect("an origin");
    assert!(
        found.file.starts_with(PROCESS_SUBSTITUTION_DIRECTORY),
        "{found:?}"
    );
    assert!(!definitions.places.is_mine(Some(found)));
}

#[test]
fn a_function_of_an_oh_my_zsh_plugin_is_not_mine() {
    let Some((home, definitions)) = omz_definitions() else {
        return;
    };
    let found = definitions.function_origin("gfun");
    assert_eq!(
        found,
        Some(&origin(
            home.path().join("omz/plugins/git/git.plugin.zsh"),
            None
        ))
    );
    assert!(!definitions.places.is_mine(found));
}

/// The script resolves the file of each autoload stub whose name starts with
/// `_`, without a load of its body, so `--all` can group it by its file.
#[test]
fn an_autoloaded_function_and_a_private_stub_get_their_files() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join("functions");
    write_file(home.path(), "functions/wdidfn", "prcp \"$@\"\n");
    write_file(home.path(), "functions/_wdidcomp", "prhash\n");
    write_file(
        home.path(),
        ".zshrc",
        &format!(
            "fpath=({} $fpath)\nautoload -Uz wdidfn _wdidcomp\n",
            shellquote::shell_quote(&directory.to_string_lossy())
        ),
    );
    let definitions = read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    for name in ["wdidfn", "_wdidcomp"] {
        assert_eq!(
            definitions.function_origin(name),
            Some(&origin(directory.join(name), None)),
            "{name}"
        );
    }
}

/// bash gives the file of a function through `declare -F`, and keeps no
/// origin for an alias.
#[test]
fn a_real_bash_gives_the_file_of_a_function_and_no_origin_for_an_alias() {
    let Some(bash) = find_shell("bash") else {
        eprintln!("skipped: bash is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    write_file(
        home.path(),
        ".bashrc",
        "alias wtm='wt --main'\nwt() { cwt \"$@\"; }\nsource \"$HOME/lib.sh\"\n",
    );
    write_file(home.path(), "lib.sh", "libfn() {\n  prcp\n}\n");
    let definitions = read(
        real_shell(&bash, home.path()),
        ShellKind::Bash,
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(
        definitions.function_origin("wt"),
        Some(&origin(home.path().join(".bashrc"), None))
    );
    assert_eq!(
        definitions.function_origin("libfn"),
        Some(&origin(home.path().join("lib.sh"), None))
    );
    assert_eq!(alias_origin(&definitions, "wtm", AliasKind::Regular), None);
    assert_eq!(definitions.places.home, Some(home.path().to_path_buf()));
    assert_eq!(definitions.places.zsh, None);
}

/// The start of the name of each temporary `ZDOTDIR`.
const ZDOTDIR_PREFIX: &str = "wdid-zdotdir-";

/// Says if `directory` has the name of a temporary `ZDOTDIR`.
fn is_temporary_zdotdir(directory: &Path) -> bool {
    directory
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with(ZDOTDIR_PREFIX))
}

/// Asserts that the alias `name` of the kind `kind` in [`omz_home`] has
/// `expected` as its origin, and that it is mine when `mine` is true.
fn assert_omz_alias(name: &str, kind: AliasKind, expected: Option<(&str, u32)>, mine: bool) {
    let Some((home, definitions)) = omz_definitions() else {
        return;
    };
    let found = alias_origin(&definitions, name, kind);
    let expected = expected.map(|(file, line)| origin(home.path().join(file), Some(line)));
    assert_eq!(found, expected, "{name}");
    assert_eq!(definitions.places.is_mine(found.as_ref()), mine, "{name}");
}

#[test]
fn an_alias_in_zshrc_has_its_file_and_its_line_and_is_mine() {
    assert_omz_alias("wt", AliasKind::Regular, Some((".zshrc", 3)), true);
}

#[test]
fn an_alias_in_a_file_that_zshrc_sources_is_mine() {
    assert_omz_alias(
        "fromsourced",
        AliasKind::Regular,
        Some(("sourced.zsh", 1)),
        true,
    );
}

#[test]
fn an_alias_of_an_oh_my_zsh_plugin_is_not_mine() {
    assert_omz_alias(
        "gst",
        AliasKind::Regular,
        Some(("omz/plugins/git/git.plugin.zsh", 1)),
        false,
    );
}

#[test]
fn an_alias_in_the_custom_directory_of_oh_my_zsh_is_mine() {
    assert_omz_alias(
        "cust",
        AliasKind::Regular,
        Some(("omz/custom/mine.zsh", 1)),
        true,
    );
}

/// `builtin alias` goes around the wrapper function, thus zsh keeps no
/// origin. An unknown origin is mine.
#[test]
fn an_alias_from_builtin_alias_has_no_origin_and_is_mine() {
    let Some((_home, definitions)) = omz_definitions() else {
        return;
    };
    assert_eq!(definitions.aliases.get("sneaky"), Some("ls"));
    assert_omz_alias("sneaky", AliasKind::Regular, None, true);
}

#[test]
fn a_global_alias_a_suffix_alias_and_an_alias_after_two_dashes_get_their_lines() {
    assert_omz_alias("G", AliasKind::Global, Some((".zshrc", 11)), true);
    assert_omz_alias("*.md", AliasKind::Suffix, Some((".zshrc", 12)), true);
    assert_omz_alias("-", AliasKind::Regular, Some((".zshrc", 13)), true);
}

#[test]
fn an_alias_from_a_process_substitution_is_in_the_descriptor_directory_and_is_not_mine() {
    let Some((_home, definitions)) = omz_definitions() else {
        return;
    };
    let found = alias_origin(&definitions, "procalias", AliasKind::Regular).expect("an origin");
    assert!(
        found.file.starts_with(PROCESS_SUBSTITUTION_DIRECTORY),
        "{found:?}"
    );
    assert_eq!(found.line, Some(1));
    assert!(!definitions.places.is_mine(Some(&found)));
}

/// The wrapper function `alias` records the origins, and the script removes
/// it before the listing. It is not a definition of the person.
#[test]
fn the_wrapper_of_alias_is_not_a_function_of_the_shell() {
    let Some((_home, definitions)) = omz_definitions() else {
        return;
    };
    assert!(!definitions.functions.contains_key("alias"));
    assert!(!definitions.function_lines.contains_key("alias"));
    assert!(!definitions.unparsed_functions.contains(&"alias".to_owned()));
    assert_eq!(definitions.aliases.get("wt"), Some("cwt"));
}

#[test]
fn the_real_zshenv_still_runs_and_its_alias_has_its_origin() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    write_file(home.path(), ".zshenv", "alias envalias='echo env'\n");
    write_file(home.path(), ".zshrc", "alias rc=x\n");
    let definitions = read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(definitions.aliases.get("envalias"), Some("echo env"));
    assert_eq!(definitions.aliases.get("rc"), Some("x"));
    assert_eq!(
        alias_origin(&definitions, "envalias", AliasKind::Regular),
        Some(origin(home.path().join(".zshenv"), Some(1)))
    );
}

/// Makes a command for a real zsh with `HOME` in `home` and no `ZDOTDIR`, so
/// zsh reads its files from `HOME`.
fn zsh_without_zdotdir(path: &Path, home: &Path) -> Command {
    let mut command = childenv::fixture_command(path);
    command
        .env("HOME", home)
        .env_remove("ZDOTDIR")
        .env("TERM", "dumb");
    command
}

/// The real `.zshenv` can set `ZDOTDIR`. zsh then reads `.zshrc` from that
/// directory, not from `HOME`.
#[test]
fn a_zshenv_that_moves_zdotdir_loads_the_zshrc_of_the_new_directory() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    write_file(home.path(), ".zshenv", "ZDOTDIR=$HOME/config\n");
    write_file(home.path(), "config/.zshrc", "alias moved='echo moved'\n");
    write_file(home.path(), ".zshrc", "alias stale=x\n");
    let definitions = read(
        zsh_without_zdotdir(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(definitions.aliases.get("moved"), Some("echo moved"));
    assert_eq!(definitions.aliases.get("stale"), None);
    assert_eq!(
        alias_origin(&definitions, "moved", AliasKind::Regular),
        Some(origin(home.path().join("config/.zshrc"), Some(1)))
    );
}

/// The `.zshrc` of the tests that read `ZDOTDIR` in the startup files.
const ZDOTDIR_ZSHRC: &str = "alias zd=\"${ZDOTDIR-unset}\"\n\
alias leak=\"${__WDID_REAL_ZDOTDIR-none}\"\n";

/// The startup files see the `ZDOTDIR` of the environment: the same value
/// when it is set, and no value when it is not set. The variable that carries
/// the real value does not stay.
#[test]
fn the_startup_files_see_the_zdotdir_of_the_environment() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    write_file(home.path(), ".zshrc", ZDOTDIR_ZSHRC);
    let set = read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(
        set.aliases.get("zd"),
        Some(home.path().to_string_lossy().as_ref())
    );
    assert_eq!(set.aliases.get("leak"), Some("none"));
    let unset = read(
        zsh_without_zdotdir(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(unset.aliases.get("zd"), Some("unset"));
    assert_eq!(unset.aliases.get("leak"), Some("none"));
}

#[test]
fn the_temporary_zdotdir_is_gone_after_a_run() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    write_file(
        home.path(),
        ".zshrc",
        "zmodload zsh/parameter\nprint -r -- \"${functions_source[alias]-}\" > \"$HOME/record\"\n",
    );
    read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    let recorded = std::fs::read_to_string(home.path().join("record")).unwrap();
    let zshenv = PathBuf::from(recorded.trim_end());
    assert_eq!(
        zshenv.file_name().and_then(|name| name.to_str()),
        Some(".zshenv"),
        "{recorded:?}"
    );
    let directory = zshenv.parent().unwrap();
    assert!(is_temporary_zdotdir(directory), "{directory:?}");
    assert!(!directory.exists(), "{directory:?} is still there");
}

#[test]
fn the_temporary_zdotdir_is_gone_after_a_timeout() {
    let records = tempfile::tempdir().unwrap();
    let record = records.path().join("record");
    let copy = records.path().join("zshenv");
    let (_dir, path) = stub(&format!(
        "printf '%s' \"$ZDOTDIR\" > {record}\ncp \"$ZDOTDIR/.zshenv\" {copy}\nexec sleep 60",
        record = shellquote::shell_quote(&record.to_string_lossy()),
        copy = shellquote::shell_quote(&copy.to_string_lossy()),
    ));
    let result = read(
        childenv::fixture_command(&path),
        ShellKind::Zsh,
        Duration::from_secs(1),
    );
    assert!(
        matches!(result, Err(ShellError::TimedOut { seconds: 1 })),
        "{result:?}"
    );
    let directory = PathBuf::from(std::fs::read_to_string(&record).unwrap());
    assert!(is_temporary_zdotdir(&directory), "{directory:?}");
    assert!(!directory.exists(), "{directory:?} is still there");
    let zshenv = std::fs::read_to_string(&copy).unwrap();
    assert!(zshenv.contains("function alias"), "{zshenv}");
    assert!(zshenv.contains("builtin alias \"$@\""), "{zshenv}");
}

#[test]
fn each_run_has_a_temporary_zdotdir_of_its_own() {
    let records = tempfile::tempdir().unwrap();
    let record = records.path().join("record");
    let (_dir, path) = stub(&format!(
        "printf '%s\\n' \"$ZDOTDIR\" >> {}\nexit 1",
        shellquote::shell_quote(&record.to_string_lossy()),
    ));
    for _ in 0..2 {
        let result = read(
            childenv::fixture_command(&path),
            ShellKind::Zsh,
            Duration::from_secs(10),
        );
        assert!(
            matches!(result, Err(ShellError::Failed { .. })),
            "{result:?}"
        );
    }
    let text = std::fs::read_to_string(&record).unwrap();
    let directories: Vec<PathBuf> = text.lines().map(PathBuf::from).collect();
    assert_eq!(directories.len(), 2, "{text}");
    assert_ne!(directories[0], directories[1], "{text}");
    for directory in &directories {
        assert!(is_temporary_zdotdir(directory), "{directory:?}");
        assert!(!directory.exists(), "{directory:?} is still there");
    }
}

/// bash does not read `ZDOTDIR`, thus a bash shell gets the `ZDOTDIR` of its
/// command unchanged.
#[test]
fn a_bash_shell_gets_no_temporary_zdotdir() {
    let records = tempfile::tempdir().unwrap();
    let record = records.path().join("record");
    let (_dir, path) = stub(&format!(
        "printf '%s' \"$ZDOTDIR\" > {}\nexit 1",
        shellquote::shell_quote(&record.to_string_lossy()),
    ));
    let mut command = childenv::fixture_command(&path);
    command.env("ZDOTDIR", "/nowhere/zdotdir");
    let result = read(command, ShellKind::Bash, Duration::from_secs(10));
    assert!(
        matches!(result, Err(ShellError::Failed { .. })),
        "{result:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        "/nowhere/zdotdir"
    );
}

/// Parses `lines` between the markers of `M` and gives the starts.
fn starts_of(lines: &[&str]) -> Starts {
    let definitions = parsed(&records(lines));
    assert!(
        definitions.unparsed_aliases.is_empty(),
        "a line of a start is not an alias line: {:?}",
        definitions.unparsed_aliases
    );
    definitions.starts
}

/// Makes a map of text to text from pairs.
fn text_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

/// Makes a set of text.
fn text_set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// A hook record names the hook and the function, a trap record names the
/// signal and the function, and the handler record names the handler.
#[test]
fn the_hook_trap_and_handler_records_give_their_functions() {
    let starts = starts_of(&[
        "M HOOK precmd precmd",
        "M HOOK chpwd mychpwd",
        "M TRAP INT TRAPINT",
        "M NOT_FOUND command_not_found_handler",
    ]);
    assert_eq!(
        starts.functions,
        [
            (Trigger::Hook("precmd".into()), "precmd".to_owned()),
            (Trigger::Hook("chpwd".into()), "mychpwd".to_owned()),
            (Trigger::Trap("INT".into()), "TRAPINT".to_owned()),
            (
                Trigger::CommandNotFound,
                "command_not_found_handler".to_owned()
            ),
        ]
    );
}

/// `zle -lL` writes one `zle` command for each widget. The function of `-N`
/// is the widget itself when the line names no function, and the function of
/// `-C` comes after the completion widget.
#[test]
fn the_widget_lines_give_the_function_of_each_widget() {
    let starts = starts_of(&[
        "M WIDGETS",
        "zle -N lonely",
        "zle -N my-widget myw",
        "zle -N zle-keymap-select azhw:zle-keymap-select",
        "zle -C mycomp complete-word _mycomp",
        "zle -N 'odd widget' oddfn",
        "zle -N",
        "echo $(",
    ]);
    assert_eq!(
        starts.widgets,
        text_map(&[
            ("lonely", "lonely"),
            ("my-widget", "myw"),
            ("zle-keymap-select", "azhw:zle-keymap-select"),
            ("mycomp", "_mycomp"),
            ("odd widget", "oddfn"),
        ])
    );
}

/// zsh writes `--` after the option of `zle` when the name of the widget
/// starts with `-`. The widget and its function stay the same.
#[test]
fn the_widget_lines_skip_the_end_of_the_options() {
    let starts = starts_of(&[
        "M WIDGETS",
        "zle -N -- -x g",
        "zle -N -- -y",
        "zle -C -- -c complete-word f",
        "zle -N -- --",
    ]);
    assert_eq!(
        starts.widgets,
        text_map(&[("-x", "g"), ("-y", "-y"), ("-c", "f"), ("--", "--")])
    );
}

/// `bindkey -M <keymap> -L` writes one `bindkey` command for each key. The
/// key is the word before the widget, without its quotes. A key in two
/// keymaps is one key. A string binding (`-s`) starts no widget.
#[test]
fn the_keymap_lines_give_the_keys_of_each_widget() {
    let starts = starts_of(&[
        "M WIDGETS",
        "zle -N my-widget myw",
        "M KEYMAP emacs",
        "bindkey -M emacs \"^R\" my-widget",
        "bindkey -s \"^Xa\" \"abc\"",
        "bindkey -R -M emacs \"a\"-\"z\" self-insert",
        "M KEYMAP vicmd",
        "bindkey -a \"^X^E\" my-widget",
        "bindkey -a \"-\" vi-up-line-or-history",
        "M KEYMAP main",
        "bindkey \"^R\" my-widget",
        "bindkey \" \" my-widget",
    ]);
    let expected: BTreeMap<String, BTreeSet<String>> = [
        ("my-widget", text_set(&["^R", "^X^E", " "])),
        ("self-insert", text_set(&["a-z"])),
        ("vi-up-line-or-history", text_set(&["-"])),
    ]
    .into_iter()
    .map(|(widget, keys)| (widget.to_owned(), keys))
    .collect();
    assert_eq!(starts.keys, expected);
    assert_eq!(starts.widgets, text_map(&[("my-widget", "myw")]));
}

/// A record of `add-zle-hook-widget` names the special widget and the widget
/// that it runs, in the order that the shell gives.
#[test]
fn the_zle_hook_records_give_the_widgets_of_each_special_widget() {
    let starts = starts_of(&[
        "M ZLE_HOOK zle-keymap-select kmsel",
        "M ZLE_HOOK zle-line-init user:zle-line-init",
        "M ZLE_HOOK zle-line-init other",
    ]);
    let expected: BTreeMap<String, Vec<String>> = [
        ("zle-keymap-select", vec!["kmsel".to_owned()]),
        (
            "zle-line-init",
            vec!["user:zle-line-init".to_owned(), "other".to_owned()],
        ),
    ]
    .into_iter()
    .map(|(special, widgets)| (special.to_owned(), widgets))
    .collect();
    assert_eq!(starts.hook_widgets, expected);
}

/// A function record after the starts still reads its body, and a start
/// record with no value gives nothing.
#[test]
fn a_function_after_the_starts_keeps_its_body() {
    let definitions = parsed(&records(&[
        "M WIDGETS",
        "zle -N w f",
        "M HOOK precmd",
        "M FUNCTION f",
        "\tprcp x",
    ]));
    assert_eq!(definitions.starts.widgets, text_map(&[("w", "f")]));
    assert!(definitions.starts.functions.is_empty());
    assert_eq!(
        definitions.functions["f"],
        vec![CommandWord::new("prcp", ["x"])]
    );
}

/// The `.zshrc` of the test of the starts of a real zsh. `ERR_EXIT` and
/// `NO_UNSET` must not stop the script, also when a hook array is not set.
const STARTS_ZSHRC: &str = "setopt ERR_EXIT NO_UNSET\n\
precmd() { prompt_helper }\n\
prompt_helper() { true }\n\
mychpwd() { true }\n\
autoload -Uz add-zsh-hook\n\
add-zsh-hook chpwd mychpwd\n\
TRAPINT() { return 128 }\n\
TRAPPED() { true }\n\
command_not_found_handler() { return 127 }\n\
searchfn() { true }\n\
zle -N history-search-wdid searchfn\n\
bindkey '^R' history-search-wdid\n\
bindkey -M vicmd '^X^E' history-search-wdid\n\
lonelyfn() { true }\n\
zle -N lonely-widget lonelyfn\n\
kmsel() { true }\n\
autoload -Uz add-zle-hook-widget\n\
add-zle-hook-widget keymap-select kmsel\n";

/// A real zsh gives its hooks, its trap functions, its handler, its widgets,
/// the keys of each widget, and the widgets of `add-zle-hook-widget`.
#[test]
fn a_real_zsh_gives_its_hooks_traps_and_widgets() {
    let Some(zsh) = find_shell("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    write_file(home.path(), ".zshrc", STARTS_ZSHRC);
    let definitions = read(
        real_shell(&zsh, home.path()),
        ShellKind::Zsh,
        Duration::from_secs(60),
    )
    .unwrap();
    let starts = &definitions.starts;
    for (trigger, name) in [
        (Trigger::Hook("precmd".into()), "precmd"),
        (Trigger::Hook("chpwd".into()), "mychpwd"),
        (Trigger::Trap("INT".into()), "TRAPINT"),
        (Trigger::CommandNotFound, "command_not_found_handler"),
    ] {
        assert!(
            starts
                .functions
                .contains(&(trigger.clone(), name.to_owned())),
            "{trigger:?} {name}: {:?}",
            starts.functions
        );
    }
    assert!(
        !starts.functions.iter().any(|(_, name)| name == "TRAPPED"),
        "PED is no signal: {:?}",
        starts.functions
    );
    assert_eq!(
        starts
            .widgets
            .get("history-search-wdid")
            .map(String::as_str),
        Some("searchfn")
    );
    assert_eq!(
        starts.widgets.get("lonely-widget").map(String::as_str),
        Some("lonelyfn")
    );
    assert_eq!(
        starts.keys.get("history-search-wdid"),
        Some(&text_set(&["^R", "^X^E"]))
    );
    assert_eq!(starts.keys.get("lonely-widget"), None);
    assert_eq!(
        starts.hook_widgets.get("zle-keymap-select"),
        Some(&vec!["kmsel".to_owned()])
    );
    assert!(starts.code.is_empty(), "{:?}", starts.code);
    assert_eq!(
        definitions.functions["precmd"],
        vec![CommandWord::new("prompt_helper", [] as [&str; 0])]
    );
}

/// `trap -p` writes one `trap` command for each signal, with the code in
/// single quotes. The code of each signal is one text, also when it holds a
/// quote or a newline. The name of a signal loses its `SIG`. An empty code
/// ignores the signal, thus it runs nothing.
#[test]
fn the_trap_lines_give_the_code_of_each_signal() {
    let starts = starts_of(&[
        "M TRAPS",
        "trap -- 'trapfn' EXIT",
        "trap -- 'echo it'\\''s; other' SIGINT",
        "trap -- 'multi",
        "line' SIGUSR1",
        "trap -- '' SIGTSTP",
        "trap -- 'dbgfn' DEBUG",
    ]);
    assert_eq!(
        starts.code,
        [
            (Trigger::Trap("EXIT".into()), "trapfn".to_owned()),
            (Trigger::Trap("INT".into()), "echo it's; other".to_owned()),
            (Trigger::Trap("USR1".into()), "multi\nline".to_owned()),
            (Trigger::Trap("DEBUG".into()), "dbgfn".to_owned()),
        ]
    );
}

/// Each element of `PROMPT_COMMAND` has a record of its own, and its lines
/// are one text.
#[test]
fn each_element_of_prompt_command_is_one_code() {
    let starts = starts_of(&[
        "M PROMPT_COMMAND",
        "pcfn; history -a",
        "M PROMPT_COMMAND",
        "second",
        "line2",
        "M PROMPT_COMMAND",
    ]);
    assert_eq!(
        starts.code,
        [
            (Trigger::PromptCommand, "pcfn; history -a".to_owned()),
            (Trigger::PromptCommand, "second\nline2".to_owned()),
        ]
    );
}

/// `bind -X` writes the key and the command of each `bind -x` binding, each in
/// double quotes. bash 5.3 divides them with a space, and an older bash with a
/// colon and a space. The command loses one level of backslashes. A line of a
/// different form gives nothing.
#[test]
fn the_key_command_lines_give_the_command_of_each_key() {
    let starts = starts_of(&[
        "M KEY_COMMANDS",
        "\"\\C-x\\C-r\" \"keyfn\"",
        "\"\\C-g\": \"echo \\\\\\\"q\\\\\\\" x\"",
        "\"\\C-a\" \"\"",
        "garbage",
        "\"unclosed",
    ]);
    assert_eq!(
        starts.code,
        [
            (Trigger::KeyCommand("\\C-x\\C-r".into()), "keyfn".to_owned()),
            (
                Trigger::KeyCommand("\\C-g".into()),
                "echo \\\"q\\\" x".to_owned()
            ),
        ]
    );
}

/// readline writes each control byte and each byte with the high bit of a
/// `bind -x` command as an escape: `\C-j` for a newline, `\C-?` for DEL, `\e`
/// for the escape character, and `\M-` before the form of the byte without
/// its high bit. The command gets back each byte, thus a second line and a
/// name in UTF-8. A backslash before a different character stays.
#[test]
fn the_key_command_lines_decode_the_escapes_of_readline() {
    let starts = starts_of(&[
        "M KEY_COMMANDS",
        r#""\C-g" "printf a\C-jecho b""#,
        r#""\C-t" "caf\M-C\M-)_fn""#,
        r#""\C-u" "printf \"\C-ix\"""#,
        r#""\C-v" "go_\M-b\M-\C-f\M-\C-r""#,
        r#""\C-w" "x\e[A\C-?\q""#,
    ]);
    let key = |key: &str| Trigger::KeyCommand(key.to_owned());
    assert_eq!(
        starts.code,
        [
            (key("\\C-g"), "printf a\necho b".to_owned()),
            (key("\\C-t"), "café_fn".to_owned()),
            (key("\\C-u"), "printf \"\tx\"".to_owned()),
            (key("\\C-v"), "go_→".to_owned()),
            (key("\\C-w"), "x\u{1b}[A\u{7f}\\q".to_owned()),
        ]
    );
}

/// The `.bashrc` of the test of the starts of a real bash. `set -u` must not
/// stop the script. The script removes the `DEBUG` trap before it lists the
/// functions, thus it must list the traps first.
const STARTS_BASHRC: &str = "set -u\n\
pcfn() { pc_helper; }\n\
pc_helper() { true; }\n\
PROMPT_COMMAND='pcfn; history -a'\n\
trapfn() { true; }\n\
trap 'trapfn' EXIT\n\
dbgfn() { true; }\n\
trap 'dbgfn' DEBUG\n";

/// A real bash gives its `PROMPT_COMMAND` and the code of each trap.
#[test]
fn a_real_bash_gives_its_prompt_command_and_its_traps() {
    let Some(bash) = find_shell("bash") else {
        eprintln!("skipped: bash is not installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    write_file(home.path(), ".bashrc", STARTS_BASHRC);
    let definitions = read(
        real_shell(&bash, home.path()),
        ShellKind::Bash,
        Duration::from_secs(60),
    )
    .unwrap();
    let code = &definitions.starts.code;
    for expected in [
        (Trigger::PromptCommand, "pcfn; history -a"),
        (Trigger::Trap("EXIT".into()), "trapfn"),
        (Trigger::Trap("DEBUG".into()), "dbgfn"),
    ] {
        assert!(
            code.contains(&(expected.0.clone(), expected.1.to_owned())),
            "{expected:?}: {code:?}"
        );
    }
    assert!(definitions.starts.widgets.is_empty());
    assert!(definitions.starts.functions.is_empty());
    assert_eq!(
        definitions.functions["pcfn"],
        vec![CommandWord::new("pc_helper", [] as [&str; 0])]
    );
}

/// Finds a bash whose `bind` has the option `-X`, which lists the `bind -x`
/// bindings. bash has it from version 4.3 on. The bash of macOS is 3.2, thus
/// the search reads each bash on the `PATH` and the fixed paths.
fn bash_with_key_listing() -> Option<PathBuf> {
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    let candidates = std::env::split_paths(&search_path)
        .chain(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from))
        .map(|dir| dir.join("bash"));
    candidates.filter(|path| path.is_file()).find(|path| {
        childenv::fixture_command(path)
            .args(["-c", "help bind"])
            .output()
            .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("PSVX]"))
    })
}

/// The `.bashrc` of the test of the `bind -x` keys of a real bash: one command
/// of one line, one command of two lines, and one command with a name in
/// UTF-8.
const KEY_COMMANDS_BASHRC: &str = "keyfn() { true; }\n\
bind -x '\"\\C-x\\C-r\": keyfn' 2>/dev/null\n\
bind -x '\"\\C-g\": printf a\n\
echo b' 2>/dev/null\n\
bind -x '\"\\C-t\": café_fn' 2>/dev/null\n";

/// A real bash gives the command of each `bind -x` key, with each line and
/// each byte of the command.
#[test]
fn a_real_bash_gives_its_key_commands() {
    let Some(bash) = bash_with_key_listing() else {
        eprintln!("skipped: no bash with bind -X is installed");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    write_file(home.path(), ".bashrc", KEY_COMMANDS_BASHRC);
    let definitions = read(
        real_shell(&bash, home.path()),
        ShellKind::Bash,
        Duration::from_secs(60),
    )
    .unwrap();
    let code = &definitions.starts.code;
    for (key, command) in [
        ("\\C-x\\C-r", "keyfn"),
        ("\\C-g", "printf a\necho b"),
        ("\\C-t", "café_fn"),
    ] {
        let expected = (Trigger::KeyCommand(key.to_owned()), command.to_owned());
        assert!(code.contains(&expected), "{bash:?}: {expected:?}: {code:?}");
    }
    assert_eq!(code.len(), 3, "{bash:?}: {code:?}");
}
