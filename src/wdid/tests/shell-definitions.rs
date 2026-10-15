//! Tests of the shell definitions: the kind of the shell, the marker, the
//! script, the parse of the output, the read of a real shell, and the resolver.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use wdid::command_words::{command_words, Aliases, CommandWord, Parsed};
use wdid::shell::{
    parse_output, read, script, Definitions, Marker, Resolver, ShellError, ShellKind,
};
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
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    (dir, path)
}

#[test]
fn a_shell_that_runs_too_long_is_timed_out_and_killed() {
    let (_dir, path) = stub("exec sleep 60");
    let started = std::time::Instant::now();
    let result = read(Command::new(&path), ShellKind::Zsh, Duration::from_secs(1));
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
    let result = read(Command::new(&path), ShellKind::Zsh, Duration::from_secs(10));
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
        Command::new(&path),
        ShellKind::Bash,
        Duration::from_secs(10),
    );
    assert!(matches!(result, Err(ShellError::NoMarkers)), "{result:?}");
}

#[test]
fn a_shell_that_does_not_exist_is_a_spawn_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no-such-shell");
    let result = read(Command::new(&path), ShellKind::Zsh, Duration::from_secs(10));
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
    let result = read(Command::new(&path), ShellKind::Zsh, Duration::from_secs(10));
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

#[test]
fn every_error_message_ends_with_the_no_shell_hint() {
    const HINT: &str = " (wdid --no-shell skips the aliases and functions)";
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
    ];
    for error in &errors {
        assert!(error.to_string().ends_with(HINT), "{error}");
    }
    assert_eq!(
        errors[0].to_string(),
        format!("SHELL is not set, so wdid cannot read the aliases and functions{HINT}")
    );
    assert_eq!(
        errors[1].to_string(),
        format!(
            "wdid reads aliases and functions from zsh and bash only, and SHELL is /bin/fish{HINT}"
        )
    );
    assert_eq!(
        errors[3].to_string(),
        format!("the shell did not stop in 30 seconds{HINT}")
    );
    assert_eq!(
        errors[4].to_string(),
        format!("the shell failed (exit status: 3): boom{HINT}")
    );
    assert_eq!(
        errors[5].to_string(),
        format!("the shell output does not hold the start and end markers of wdid{HINT}")
    );
    assert_eq!(
        errors[2].to_string(),
        format!("cannot start zsh: entity not found{HINT}")
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
    let mut command = Command::new(path);
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
