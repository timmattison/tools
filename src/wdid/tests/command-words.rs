//! Tests of the command words of one command line.

use wdid::command_words::{command_words, Aliases, CommandWord, Parsed};

/// Runs every row of a table. Each row is a line and the value that
/// `command_words` must give for it, with an empty alias table.
fn check(rows: Vec<(&str, Parsed)>) {
    let aliases = Aliases::new();
    for (line, expected) in rows {
        assert_eq!(command_words(line, &aliases), expected, "line: {line:?}");
    }
}

#[test]
fn a_line_that_the_parser_refuses_gives_unparsed() {
    check(vec![
        (";;", Parsed::Unparsed),
        ("a |", Parsed::Unparsed),
        ("echo $(", Parsed::Unparsed),
    ]);
}

/// Makes the expected value for a line that parses.
fn words(list: Vec<CommandWord>) -> Parsed {
    Parsed::Words(list)
}

/// Makes a command word that the person typed.
fn word<const N: usize>(name: &str, args: [&str; N]) -> CommandWord {
    CommandWord::new(name, args)
}

#[test]
fn the_walk_finds_the_command_words_of_every_list_and_group() {
    check(vec![
        ("prcp a b", words(vec![word("prcp", ["a", "b"])])),
        (
            "cat x | prhash",
            words(vec![word("cat", ["x"]), word("prhash", [])]),
        ),
        ("a && b", words(vec![word("a", []), word("b", [])])),
        ("a || b", words(vec![word("a", []), word("b", [])])),
        ("a ; b", words(vec![word("a", []), word("b", [])])),
        ("a & b", words(vec![word("a", []), word("b", [])])),
        ("(a; b)", words(vec![word("a", []), word("b", [])])),
        ("{ a; b; }", words(vec![word("a", []), word("b", [])])),
        (
            "! a | b && c",
            words(vec![word("a", []), word("b", []), word("c", [])]),
        ),
        (
            "if a; then b; elif c; then d; else e; fi",
            words(vec![
                word("a", []),
                word("b", []),
                word("c", []),
                word("d", []),
                word("e", []),
            ]),
        ),
        (
            "for f in 1 2; do prcp $f; done",
            words(vec![word("prcp", ["$f"])]),
        ),
        (
            "for ((i=0; i<3; i++)); do a; done",
            words(vec![word("a", [])]),
        ),
        (
            "while a; do b; done",
            words(vec![word("a", []), word("b", [])]),
        ),
        (
            "until a; do b; done",
            words(vec![word("a", []), word("b", [])]),
        ),
        (
            "case $x in p) a;; q) b; c;; esac",
            words(vec![word("a", []), word("b", []), word("c", [])]),
        ),
        ("time prcp x", words(vec![word("prcp", ["x"])])),
        ("f() { prcp; }", words(vec![])),
        ("prcp >out.txt 2>&1", words(vec![word("prcp", [])])),
        ("(( 1 + 2 ))", words(vec![])),
    ]);
}

#[test]
fn an_empty_line_and_a_comment_line_give_no_words() {
    check(vec![
        ("", words(vec![])),
        ("   \t ", words(vec![])),
        ("# prcp a b", words(vec![])),
        ("\n\n", words(vec![])),
    ]);
}

#[test]
fn the_walk_finds_the_command_words_inside_substitutions() {
    check(vec![
        (
            "echo $(prcp x)",
            words(vec![word("echo", ["$(prcp x)"]), word("prcp", ["x"])]),
        ),
        (
            "echo `prcp x`",
            words(vec![word("echo", ["`prcp x`"]), word("prcp", ["x"])]),
        ),
        (
            "echo \"$(prcp x)\"",
            words(vec![word("echo", ["\"$(prcp x)\""]), word("prcp", ["x"])]),
        ),
        (
            "echo \"a $(b \"$(c)\")\"",
            words(vec![
                word("echo", ["\"a $(b \"$(c)\")\""]),
                word("b", ["\"$(c)\""]),
                word("c", []),
            ]),
        ),
        (
            "eval $(dirc -paste)",
            words(vec![
                word("eval", ["$(dirc -paste)"]),
                word("dirc", ["-paste"]),
            ]),
        ),
        (
            "for f in $(ls); do prcp $f; done",
            words(vec![word("ls", []), word("prcp", ["$f"])]),
        ),
        (
            "case $(a) in p) b;; esac",
            words(vec![word("a", []), word("b", [])]),
        ),
        ("x=$(cwt)", words(vec![word("cwt", [])])),
        ("FOO=bar prcp", words(vec![word("prcp", [])])),
        (
            "FOO=$(a) b c=$(d)",
            words(vec![word("a", []), word("b", ["c=$(d)"]), word("d", [])]),
        ),
        (
            "diff <(a) <(b)",
            words(vec![word("diff", []), word("a", []), word("b", [])]),
        ),
        ("cat < <(a)", words(vec![word("cat", []), word("a", [])])),
        ("[[ $(a) == x ]]", words(vec![word("a", [])])),
        (
            "[[ -n x && ( $(a) != $(b) ) ]] >$(c)",
            words(vec![word("a", []), word("b", []), word("c", [])]),
        ),
        ("a >$(b)", words(vec![word("a", []), word("b", [])])),
        ("{ a; } >$(b)", words(vec![word("a", []), word("b", [])])),
        (
            "cat <<EOF\n$(prcp)\nEOF",
            words(vec![word("cat", []), word("prcp", [])]),
        ),
        ("cat <<'EOF'\nprcp\nEOF", words(vec![word("cat", [])])),
        ("cat <<EOF\nprcp $x\nEOF", words(vec![word("cat", [])])),
        (
            "cat <<<$(prcp)",
            words(vec![word("cat", []), word("prcp", [])]),
        ),
        ("f() { $(a); }", words(vec![])),
        ("echo $((1 + 2))", words(vec![word("echo", ["$((1 + 2))"])])),
    ]);
}

#[test]
fn a_substitution_that_the_parser_refuses_gives_unparsed() {
    check(vec![
        ("echo \"$(;;)\"", Parsed::Unparsed),
        ("echo $(a |)", Parsed::Unparsed),
        ("echo `a |`", Parsed::Unparsed),
        ("echo $(echo $(;;))", Parsed::Unparsed),
        ("cat <<EOF\n$(a |)\nEOF", Parsed::Unparsed),
    ]);
}

/// Makes a line that nests `levels` command substitutions around `prcp`.
fn nested(levels: usize) -> String {
    format!("{}prcp{}", "echo $(".repeat(levels), ")".repeat(levels))
}

#[test]
fn the_depth_limit_is_64_levels_of_substitutions() {
    let aliases = Aliases::new();
    let Parsed::Words(inside) = command_words(&nested(64), &aliases) else {
        panic!("64 levels must parse");
    };
    assert_eq!(inside.len(), 65);
    assert_eq!(inside.last().unwrap().word, "prcp");
    assert_eq!(command_words(&nested(65), &aliases), Parsed::Unparsed);
}

#[test]
fn quote_removal_applies_to_the_command_word_and_the_arguments() {
    check(vec![
        (
            "git commit -m \"fix prcp\"",
            words(vec![word("git", ["commit", "-m", "fix prcp"])]),
        ),
        ("echo 'prcp'", words(vec![word("echo", ["prcp"])])),
        (
            "echo '$x' \"$y\" $z",
            words(vec![word("echo", ["$x", "\"$y\"", "$z"])]),
        ),
        ("echo '' \"\"", words(vec![word("echo", ["", ""])])),
        ("\"prcp\" x", words(vec![word("prcp", ["x"])])),
        ("'prcp' x", words(vec![word("prcp", ["x"])])),
        ("pr\"c\"'p' x", words(vec![word("prcp", ["x"])])),
        ("\\prcp", words(vec![word("prcp", [])])),
        ("$EDITOR x", words(vec![])),
        ("pr$(x)cp y", words(vec![word("x", [])])),
        ("$(a) x", words(vec![word("a", [])])),
        (
            "~/.cargo/bin/prcp x",
            words(vec![word("~/.cargo/bin/prcp", ["x"])]),
        ),
        ("~tim/bin/prcp", words(vec![word("~tim/bin/prcp", [])])),
        ("~+/prcp", words(vec![])),
        (
            "export FOO=\"a b\" BAR=$(a)",
            words(vec![word("export", ["FOO=a b", "BAR=$(a)"]), word("a", [])]),
        ),
        ("cat <<'EOF'\nprcp\nEOF", words(vec![word("cat", [])])),
    ]);
}

#[test]
fn an_escape_sequence_gives_the_character_after_the_backslash() {
    check(vec![
        ("echo a\\ b", words(vec![word("echo", ["a b"])])),
        ("echo \\$HOME", words(vec![word("echo", ["$HOME"])])),
        ("echo x\\'y", words(vec![word("echo", ["x'y"])])),
        ("echo a\\\\b", words(vec![word("echo", ["a\\b"])])),
        ("echo a\\\nb", words(vec![word("echo", ["ab"])])),
        ("echo \"a\\\"b\"", words(vec![word("echo", ["a\"b"])])),
        ("echo \"a\\\\b\"", words(vec![word("echo", ["a\\b"])])),
        ("echo \"a\\$b\"", words(vec![word("echo", ["a$b"])])),
        ("echo \"a\\nb\"", words(vec![word("echo", ["a\\nb"])])),
        ("echo a\\ b\\ c", words(vec![word("echo", ["a b c"])])),
        ("a\\ b x", words(vec![word("a b", ["x"])])),
    ]);
}

#[test]
fn a_piece_that_is_not_text_keeps_the_raw_text_of_the_argument() {
    check(vec![
        (
            "echo ~ ~tim ~+",
            words(vec![word("echo", ["~", "~tim", "~+"])]),
        ),
        (
            "echo ${x} $'a\\tb'",
            words(vec![word("echo", ["${x}", "$'a\\tb'"])]),
        ),
        (
            "echo a${x}b \"a${x}b\"",
            words(vec![word("echo", ["a${x}b", "\"a${x}b\""])]),
        ),
        ("echo $((1 + 2))", words(vec![word("echo", ["$((1 + 2))"])])),
    ]);
}

#[test]
fn multibyte_text_survives_the_walk_and_the_quote_removal() {
    check(vec![
        (
            "prcp 日本語.txt 🎉/",
            words(vec![word("prcp", ["日本語.txt", "🎉/"])]),
        ),
        (
            "echo \"日本語 prcp\" 'café' 🎉",
            words(vec![word("echo", ["日本語 prcp", "café", "🎉"])]),
        ),
        (
            "echo 日本語$(prcp 🎉)",
            words(vec![
                word("echo", ["日本語$(prcp 🎉)"]),
                word("prcp", ["🎉"]),
            ]),
        ),
        ("日本語 x", words(vec![word("日本語", ["x"])])),
    ]);
}

#[test]
fn a_wrapper_gives_the_command_word_that_it_runs() {
    check(vec![
        ("sudo prcp x", words(vec![word("prcp", ["x"])])),
        ("doas prcp x", words(vec![word("prcp", ["x"])])),
        (
            "env time -o out.txt prcp x",
            words(vec![word("prcp", ["x"])]),
        ),
        ("nice prcp x", words(vec![word("prcp", ["x"])])),
        ("nohup prcp x", words(vec![word("prcp", ["x"])])),
        ("env prcp x", words(vec![word("prcp", ["x"])])),
        ("command prcp x", words(vec![word("prcp", ["x"])])),
        ("exec -a name prcp x", words(vec![word("prcp", ["x"])])),
        ("noglob prcp x", words(vec![word("prcp", ["x"])])),
        ("builtin prcp x", words(vec![word("prcp", ["x"])])),
        ("caffeinate -t 10 prcp x", words(vec![word("prcp", ["x"])])),
        ("xargs prcp x", words(vec![word("prcp", ["x"])])),
        ("watch -n 5 prcp x", words(vec![word("prcp", ["x"])])),
        ("timeout 10 prcp x", words(vec![word("prcp", ["x"])])),
        ("gtimeout 10 prcp x", words(vec![word("prcp", ["x"])])),
        ("sudo nohup prcp", words(vec![word("prcp", [])])),
        (
            "sudo env nice -n 5 timeout 9 prcp x",
            words(vec![word("prcp", ["x"])]),
        ),
        ("\"sudo\" prcp", words(vec![word("prcp", [])])),
        (
            "cat x | xargs prhash",
            words(vec![word("cat", ["x"]), word("prhash", [])]),
        ),
        ("a && sudo b", words(vec![word("a", []), word("b", [])])),
        (
            "sudo prcp $(cwt)",
            words(vec![word("prcp", ["$(cwt)"]), word("cwt", [])]),
        ),
        ("sudo >out.txt prcp", words(vec![word("prcp", [])])),
        ("prcp sudo x", words(vec![word("prcp", ["sudo", "x"])])),
        ("sudoku x", words(vec![word("sudoku", ["x"])])),
    ]);
}

#[test]
fn a_wrapper_skips_its_options_in_the_getopt_form() {
    check(vec![
        ("sudo -u root prcp x", words(vec![word("prcp", ["x"])])),
        ("sudo -uroot prcp x", words(vec![word("prcp", ["x"])])),
        ("sudo --user=root prcp x", words(vec![word("prcp", ["x"])])),
        ("sudo --user root prcp x", words(vec![word("prcp", ["x"])])),
        (
            "sudo --preserve-env prcp x",
            words(vec![word("prcp", ["x"])]),
        ),
        ("sudo -nu root prcp x", words(vec![word("prcp", ["x"])])),
        ("sudo -n -E -u root prcp", words(vec![word("prcp", [])])),
        ("sudo -s prcp", words(vec![word("prcp", [])])),
        ("sudo -- prcp x", words(vec![word("prcp", ["x"])])),
        ("sudo -- -u", words(vec![word("-u", [])])),
        ("sudo \"-u\" root prcp", words(vec![word("prcp", [])])),
        ("nice -n 10 prcp", words(vec![word("prcp", [])])),
        ("nice -n10 prcp", words(vec![word("prcp", [])])),
        ("nice -10 prcp", words(vec![word("prcp", [])])),
        ("nice --adjustment 5 prcp", words(vec![word("prcp", [])])),
        ("nohup -u prcp", words(vec![word("prcp", [])])),
        ("env -u FOO prcp", words(vec![word("prcp", [])])),
        ("env -i -u FOO -C /tmp prcp", words(vec![word("prcp", [])])),
        ("env --unset FOO prcp", words(vec![word("prcp", [])])),
        ("env - prcp", words(vec![word("prcp", [])])),
        ("sudo - prcp", words(vec![word("-", ["prcp"])])),
        ("xargs -I {} prcp {}", words(vec![word("prcp", ["{}"])])),
        ("xargs -0 prcp", words(vec![word("prcp", [])])),
        ("xargs -n 1 -P 4 prcp", words(vec![word("prcp", [])])),
        ("xargs -n1 -0 prcp", words(vec![word("prcp", [])])),
        ("xargs --max-args 1 prcp", words(vec![word("prcp", [])])),
        ("xargs --null prcp", words(vec![word("prcp", [])])),
        ("watch -n 5 -d prcp", words(vec![word("prcp", [])])),
        ("exec -a name prcp", words(vec![word("prcp", [])])),
        ("doas -u root prcp", words(vec![word("prcp", [])])),
        ("caffeinate -i -t 10 prcp", words(vec![word("prcp", [])])),
        ("sudo -u root -- prcp x", words(vec![word("prcp", ["x"])])),
    ]);
}

#[test]
fn the_special_rules_of_the_wrappers() {
    check(vec![
        ("sudo -E FOO=1 prcp", words(vec![word("prcp", [])])),
        ("sudo FOO=1 BAR=2 prcp x", words(vec![word("prcp", ["x"])])),
        ("env FOO=bar BAZ=1 prcp", words(vec![word("prcp", [])])),
        ("env -i FOO=bar prcp", words(vec![word("prcp", [])])),
        ("env FOO=$x prcp", words(vec![word("prcp", [])])),
        ("env _a1=2 prcp", words(vec![word("prcp", [])])),
        ("env 1A=2 prcp", words(vec![word("1A=2", ["prcp"])])),
        ("env =2 prcp", words(vec![word("=2", ["prcp"])])),
        ("env A-B=2 prcp", words(vec![word("A-B=2", ["prcp"])])),
        ("nice FOO=1 prcp", words(vec![word("FOO=1", ["prcp"])])),
        ("timeout 10 prcp", words(vec![word("prcp", [])])),
        ("timeout -s KILL 10 prcp", words(vec![word("prcp", [])])),
        ("timeout -sKILL 10 prcp", words(vec![word("prcp", [])])),
        (
            "timeout --signal KILL 10 prcp",
            words(vec![word("prcp", [])]),
        ),
        (
            "timeout --signal=KILL 10 prcp",
            words(vec![word("prcp", [])]),
        ),
        (
            "timeout -k 5 -s KILL 10 prcp x",
            words(vec![word("prcp", ["x"])]),
        ),
        (
            "gtimeout --foreground 10 prcp",
            words(vec![word("prcp", [])]),
        ),
        ("timeout 10", words(vec![])),
        ("timeout", words(vec![])),
        ("command -v prcp", words(vec![])),
        ("command -V prcp", words(vec![])),
        ("command -pv prcp", words(vec![])),
        ("command -p -v prcp", words(vec![])),
        ("command -p prcp", words(vec![word("prcp", [])])),
        ("command -- prcp", words(vec![word("prcp", [])])),
        ("sudo command -v prcp", words(vec![])),
        ("sudo -v prcp", words(vec![word("prcp", [])])),
        ("sudo", words(vec![])),
        ("sudo -u", words(vec![])),
        ("sudo -u root", words(vec![])),
        ("sudo --", words(vec![])),
        ("sudo nohup", words(vec![])),
        ("env FOO=1", words(vec![])),
        ("sudo $CMD", words(vec![])),
        ("sudo $CMD prcp", words(vec![])),
        ("sudo \"$CMD\" prcp", words(vec![])),
        ("sudo -u $USER prcp", words(vec![word("prcp", [])])),
        ("sudo $(a)", words(vec![word("a", [])])),
        ("xargs -I", words(vec![])),
        ("sudo 日本語 🎉", words(vec![word("日本語", ["🎉"])])),
    ]);
}

/// Makes an alias table from pairs of a name and a value.
fn table<const N: usize>(pairs: [(&str, &str); N]) -> Aliases {
    pairs
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

/// Runs every row of a table with the alias table `aliases`.
fn check_with(aliases: &Aliases, rows: Vec<(&str, Parsed)>) {
    for (line, expected) in rows {
        assert_eq!(command_words(line, aliases), expected, "line: {line:?}");
    }
}

#[test]
fn an_alias_at_command_position_gives_the_words_of_its_value() {
    let aliases = table([
        ("wtm", "wt --main"),
        ("crr", "cargo run --release"),
        ("a", "b x"),
        ("b", "prcp"),
        ("e", ""),
    ]);
    check_with(
        &aliases,
        vec![
            ("wtm", words(vec![word("wt", ["--main"]).through("wtm")])),
            (
                "crr --bin prcp",
                words(vec![
                    word("cargo", ["run", "--release", "--bin", "prcp"]).through("crr")
                ]),
            ),
            ("a y", words(vec![word("prcp", ["x", "y"]).through("a")])),
            ("b", words(vec![word("prcp", []).through("b")])),
            (
                "cat x | wtm | prhash",
                words(vec![
                    word("cat", ["x"]),
                    word("wt", ["--main"]).through("wtm"),
                    word("prhash", []),
                ]),
            ),
            (
                "a && wtm; prcp",
                words(vec![
                    word("prcp", ["x"]).through("a"),
                    word("wt", ["--main"]).through("wtm"),
                    word("prcp", []),
                ]),
            ),
            ("e prcp", words(vec![word("prcp", [])])),
        ],
    );
}

#[test]
fn an_alias_that_names_itself_is_not_expanded_again() {
    let aliases = table([
        ("ls", "ls -G"),
        ("a", "b"),
        ("b", "a"),
        ("c", "echo $(c)"),
        ("d", "echo $(e)"),
        ("e", "d"),
    ]);
    check_with(
        &aliases,
        vec![
            ("ls x", words(vec![word("ls", ["-G", "x"]).through("ls")])),
            ("a", words(vec![word("a", []).through("a")])),
            ("b", words(vec![word("b", []).through("b")])),
            (
                "c",
                words(vec![
                    word("echo", ["$(c)"]).through("c"),
                    word("c", []).through("c"),
                ]),
            ),
            (
                "d",
                words(vec![
                    word("echo", ["$(e)"]).through("d"),
                    word("d", []).through("d"),
                ]),
            ),
        ],
    );
}

#[test]
fn a_value_that_ends_with_a_blank_expands_the_next_word() {
    let aliases = table([
        ("_", "sudo "),
        ("wtm", "wt --main"),
        ("s", "sudo "),
        ("n", "nice "),
        ("t", "sudo\t"),
    ]);
    check_with(
        &aliases,
        vec![
            ("_ prcp", words(vec![word("prcp", [])])),
            ("_ wtm", words(vec![word("wt", ["--main"]).through("wtm")])),
            (
                "_ wtm x",
                words(vec![word("wt", ["--main", "x"]).through("wtm")]),
            ),
            ("_ _ prcp", words(vec![word("_", ["prcp"])])),
            ("s n prcp x", words(vec![word("prcp", ["x"])])),
            (
                "s n wtm",
                words(vec![word("wt", ["--main"]).through("wtm")]),
            ),
            ("t wtm", words(vec![word("wt", ["--main"]).through("wtm")])),
            ("_ \"wtm\"", words(vec![word("wtm", [])])),
            (
                "wtm _ prcp",
                words(vec![word("wt", ["--main", "_", "prcp"]).through("wtm")]),
            ),
        ],
    );
}

#[test]
fn the_words_of_a_substitution_keep_the_origin_of_the_word_that_holds_it() {
    let aliases = table([("dirp", "eval $(dirc -paste)"), ("wtm", "wt --main")]);
    check_with(
        &aliases,
        vec![
            (
                "dirp",
                words(vec![
                    word("eval", ["$(dirc -paste)"]).through("dirp"),
                    word("dirc", ["-paste"]).through("dirp"),
                ]),
            ),
            (
                "echo $(wtm)",
                words(vec![
                    word("echo", ["$(wtm)"]),
                    word("wt", ["--main"]).through("wtm"),
                ]),
            ),
            (
                "echo \"a $(wtm x)\"",
                words(vec![
                    word("echo", ["\"a $(wtm x)\""]),
                    word("wt", ["--main", "x"]).through("wtm"),
                ]),
            ),
            (
                "echo `wtm`",
                words(vec![
                    word("echo", ["`wtm`"]),
                    word("wt", ["--main"]).through("wtm"),
                ]),
            ),
            (
                "x=$(wtm)",
                words(vec![word("wt", ["--main"]).through("wtm")]),
            ),
            (
                "dirp x",
                words(vec![
                    word("eval", ["$(dirc -paste)", "x"]).through("dirp"),
                    word("dirc", ["-paste"]).through("dirp"),
                ]),
            ),
            (
                "dirp $(prcp)",
                words(vec![
                    word("eval", ["$(dirc -paste)", "$(prcp)"]).through("dirp"),
                    word("dirc", ["-paste"]).through("dirp"),
                    word("prcp", []),
                ]),
            ),
        ],
    );
}

#[test]
fn a_word_that_is_not_at_command_position_or_not_plain_is_not_expanded() {
    let aliases = table([("wtm", "wt --main"), ("sudo", "sudo -n")]);
    check_with(
        &aliases,
        vec![
            ("\"wtm\"", words(vec![word("wtm", [])])),
            ("'wtm' x", words(vec![word("wtm", ["x"])])),
            ("\\wtm", words(vec![word("wtm", [])])),
            ("w\"t\"m", words(vec![word("wtm", [])])),
            ("echo wtm", words(vec![word("echo", ["wtm"])])),
            ("xargs wtm", words(vec![word("wtm", [])])),
            ("env wtm x", words(vec![word("wtm", ["x"])])),
            ("nohup wtm", words(vec![word("wtm", [])])),
            ("wtmx", words(vec![word("wtmx", [])])),
            ("f() { wtm; }", words(vec![])),
            ("sudo xargs wtm", words(vec![word("wtm", [])])),
        ],
    );
}

#[test]
fn an_alias_value_with_a_pipe_gives_a_word_for_each_command() {
    let aliases = table([("p", "prcp | prhash"), ("q", "cat x |")]);
    check_with(
        &aliases,
        vec![
            (
                "p x",
                words(vec![
                    word("prcp", []).through("p"),
                    word("prhash", ["x"]).through("p"),
                ]),
            ),
            (
                "p",
                words(vec![
                    word("prcp", []).through("p"),
                    word("prhash", []).through("p"),
                ]),
            ),
            (
                "q prcp",
                words(vec![word("cat", ["x"]).through("q"), word("prcp", [])]),
            ),
        ],
    );
}

#[test]
fn a_prefix_and_a_redirect_stay_outside_the_new_text() {
    let aliases = table([("wtm", "wt --main")]);
    check_with(
        &aliases,
        vec![
            (
                "FOO=1 wtm",
                words(vec![word("wt", ["--main"]).through("wtm")]),
            ),
            (
                "FOO=$(a) wtm",
                words(vec![word("a", []), word("wt", ["--main"]).through("wtm")]),
            ),
            (
                "wtm >out.txt",
                words(vec![word("wt", ["--main"]).through("wtm")]),
            ),
            (
                "wtm x >out.txt 2>&1",
                words(vec![word("wt", ["--main", "x"]).through("wtm")]),
            ),
            (
                "wtm >$(a)",
                words(vec![word("wt", ["--main"]).through("wtm"), word("a", [])]),
            ),
            (
                "wtm <(a) $(b)",
                words(vec![
                    word("wt", ["--main", "$(b)"]).through("wtm"),
                    word("b", []),
                    word("a", []),
                ]),
            ),
            (
                "wtm <<EOF\n$(a)\nEOF",
                words(vec![word("wt", ["--main"]).through("wtm"), word("a", [])]),
            ),
            (
                "wtm y=$(a) z",
                words(vec![
                    word("wt", ["--main", "y=$(a)", "z"]).through("wtm"),
                    word("a", []),
                ]),
            ),
        ],
    );
}

#[test]
fn an_alias_value_that_does_not_parse_gives_unparsed() {
    let aliases = table([("bad", "a |"), ("broken", "echo $("), ("ok", "prcp")]);
    check_with(
        &aliases,
        vec![
            ("bad", Parsed::Unparsed),
            ("broken x", Parsed::Unparsed),
            ("echo $(bad)", Parsed::Unparsed),
            ("ok", words(vec![word("prcp", []).through("ok")])),
        ],
    );
}

#[test]
fn multibyte_text_survives_the_alias_expansion() {
    let aliases = table([
        ("ch", "crap --here"),
        ("pj", "echo 日本語日本語日本語 |"),
        ("jp", "日本語 --x"),
        ("wtm", "wt --main"),
    ]);
    check_with(
        &aliases,
        vec![
            (
                "ch 日本語",
                words(vec![word("crap", ["--here", "日本語"]).through("ch")]),
            ),
            (
                "pj prcp",
                words(vec![
                    word("echo", ["日本語日本語日本語"]).through("pj"),
                    word("prcp", []),
                ]),
            ),
            (
                "jp 🎉 $(wtm)",
                words(vec![
                    word("日本語", ["--x", "🎉", "$(wtm)"]).through("jp"),
                    word("wt", ["--main"]).through("wtm"),
                ]),
            ),
            (
                "日本語日本語 | pj 🎉",
                words(vec![
                    word("日本語日本語", []),
                    word("echo", ["日本語日本語日本語"]).through("pj"),
                    word("🎉", []),
                ]),
            ),
        ],
    );
}

/// Makes `count` aliases where `a0` expands to `a1`, `a1` to `a2`, and the
/// last one to `prcp`.
fn alias_ladder(count: usize) -> Aliases {
    (0..count)
        .map(|index| {
            let value = if index + 1 == count {
                "prcp".to_owned()
            } else {
                format!("a{}", index + 1)
            };
            (format!("a{index}"), value)
        })
        .collect()
}

#[test]
fn each_alias_expansion_counts_as_one_level_of_the_depth_limit() {
    let Parsed::Words(inside) = command_words("a0", &alias_ladder(64)) else {
        panic!("64 expansions must parse");
    };
    assert_eq!(inside, vec![word("prcp", []).through("a0")]);
    assert_eq!(command_words("a0", &alias_ladder(65)), Parsed::Unparsed);
}

/// Makes an alias table with regular aliases, global aliases, and suffix
/// aliases. Each list holds pairs of a name (a suffix for a suffix alias) and
/// a value.
fn kinds<const R: usize, const G: usize, const S: usize>(
    regular: [(&str, &str); R],
    global: [(&str, &str); G],
    suffix: [(&str, &str); S],
) -> Aliases {
    let mut aliases = table(regular);
    for (name, value) in global {
        aliases.insert_global(name, value);
    }
    for (suffix, value) in suffix {
        aliases.insert_suffix(suffix, value);
    }
    aliases
}

/// zsh expands a global alias in every position, command position included,
/// and after a wrapper.
#[test]
fn a_global_alias_expands_in_every_position() {
    let aliases = kinds(
        [],
        [
            ("PH", "| prhash"),
            ("G", "prcp -v"),
            ("P", "prcp"),
            ("日本", "| prhash"),
        ],
        [],
    );
    check_with(
        &aliases,
        vec![
            (
                "echo hi PH",
                words(vec![word("echo", ["hi"]), word("prhash", []).through("PH")]),
            ),
            ("G x", words(vec![word("prcp", ["-v", "x"]).through("G")])),
            ("sudo P", words(vec![word("prcp", []).through("P")])),
            ("command P", words(vec![word("prcp", []).through("P")])),
            (
                "echo $(cat x PH)",
                words(vec![
                    word("echo", ["$(cat x PH)"]),
                    word("cat", ["x"]),
                    word("prhash", []).through("PH"),
                ]),
            ),
            (
                "echo 🎉 日本",
                words(vec![
                    word("echo", ["🎉"]),
                    word("prhash", []).through("日本"),
                ]),
            ),
            (
                "P;P",
                words(vec![
                    word("prcp", []).through("P"),
                    word("prcp", []).through("P"),
                ]),
            ),
        ],
    );
}

/// The value of a global alias takes the place of one word. It stays apart
/// from an operator next to that word, as in zsh. Thus `>PH` and `|PH` with a
/// value that starts with `|` do not parse, as in zsh, and do not become `>|`
/// and `||`.
#[test]
fn a_global_alias_in_a_redirect_target_expands_and_stays_one_word() {
    let aliases = kinds([], [("PH", "| prhash"), ("O", "out.txt")], []);
    check_with(
        &aliases,
        vec![
            ("echo hi > PH", Parsed::Unparsed),
            ("echo hi >PH", Parsed::Unparsed),
            ("echo hi|PH", Parsed::Unparsed),
            ("echo hi > O", words(vec![word("echo", ["hi"])])),
            ("echo hi >O", words(vec![word("echo", ["hi"])])),
        ],
    );
}

/// Only a plain word that is the whole name of a global alias expands. A
/// quoted word, an escaped word, a longer word, an assignment, and the body
/// of a here document do not.
#[test]
fn only_a_plain_word_that_is_the_whole_name_is_a_global_alias() {
    let aliases = kinds([], [("PH", "| prhash")], []);
    check_with(
        &aliases,
        vec![
            (
                "echo PH",
                words(vec![word("echo", []), word("prhash", []).through("PH")]),
            ),
            ("echo \"PH\"", words(vec![word("echo", ["PH"])])),
            ("echo 'PH'", words(vec![word("echo", ["PH"])])),
            ("echo \\PH", words(vec![word("echo", ["PH"])])),
            ("echo xPH", words(vec![word("echo", ["xPH"])])),
            ("X=PH prcp", words(vec![word("prcp", [])])),
            ("cat <<EOF\nPH\nEOF", words(vec![word("cat", [])])),
        ],
    );
}

/// A global alias that the person typed inside a substitution expands, also
/// when the same alias expands outside the substitution.
#[test]
fn a_typed_global_alias_inside_a_substitution_expands_next_to_one_outside() {
    let aliases = kinds([], [("PH", "| prhash")], []);
    check_with(
        &aliases,
        vec![(
            "cat PH $(cat PH)",
            words(vec![
                word("cat", []),
                word("prhash", ["$(cat PH)"]).through("PH"),
                word("cat", []),
                word("prhash", []).through("PH"),
            ]),
        )],
    );
}

/// A global alias in the value of a global alias or of a regular alias
/// expands. The words of the value have the origin of the word that the
/// person typed.
#[test]
fn a_global_alias_in_the_value_of_an_alias_expands() {
    let aliases = kinds(
        [("wtm", "wt PH")],
        [("A", "B x"), ("B", "prcp"), ("PH", "| prhash")],
        [],
    );
    check_with(
        &aliases,
        vec![
            ("A", words(vec![word("prcp", ["x"]).through("A")])),
            ("echo A", words(vec![word("echo", ["prcp", "x"])])),
            (
                "wtm",
                words(vec![
                    word("wt", []).through("wtm"),
                    word("prhash", []).through("wtm"),
                ]),
            ),
        ],
    );
}

/// A global alias in its own value is not expanded again, so the cycle ends.
/// A value that puts the alias into a substitution ends at the depth limit.
#[test]
fn a_global_alias_cycle_ends() {
    let aliases = kinds([], [("G", "G x"), ("S", "$(S)")], []);
    check_with(
        &aliases,
        vec![
            ("G", words(vec![word("G", ["x"]).through("G")])),
            ("echo G", words(vec![word("echo", ["G", "x"])])),
            ("S", Parsed::Unparsed),
        ],
    );
}

/// A later insert of the same name replaces the alias, and its kind with it.
#[test]
fn a_later_insert_replaces_the_kind_of_an_alias() {
    let mut aliases = Aliases::new();
    aliases.insert("G", "echo");
    aliases.insert_global("G", "| prhash");
    aliases.insert_global("H", "| prhash");
    aliases.insert("H", "prcp");
    check_with(
        &aliases,
        vec![
            (
                "x G",
                words(vec![word("x", []), word("prhash", []).through("G")]),
            ),
            ("x H", words(vec![word("x", ["H"])])),
            ("H", words(vec![word("prcp", []).through("H")])),
        ],
    );
}

/// A suffix alias expands a command word that ends with its suffix. The new
/// text is the value, then the word, then the rest of the command.
#[test]
fn a_suffix_alias_expands_the_command_word() {
    let aliases = kinds([], [], [("md", "glow"), ("gz", "prgz -d")]);
    check_with(
        &aliases,
        vec![
            (
                "README.md",
                words(vec![word("glow", ["README.md"]).through("*.md")]),
            ),
            (
                "./docs/a.md x",
                words(vec![word("glow", ["./docs/a.md", "x"]).through("*.md")]),
            ),
            (
                "a.tar.gz",
                words(vec![word("prgz", ["-d", "a.tar.gz"]).through("*.gz")]),
            ),
        ],
    );
}

/// A suffix alias needs a plain word in command position, with text before
/// the last dot. It does not apply after a wrapper.
#[test]
fn a_suffix_alias_needs_a_plain_command_word_with_a_stem() {
    let aliases = kinds([], [], [("md", "glow")]);
    check_with(
        &aliases,
        vec![
            ("x.md", words(vec![word("glow", ["x.md"]).through("*.md")])),
            (".md", words(vec![word(".md", [])])),
            ("\"q.md\"", words(vec![word("q.md", [])])),
            ("echo x.md", words(vec![word("echo", ["x.md"])])),
            ("sudo x.md", words(vec![word("x.md", [])])),
        ],
    );
}

/// The value of a suffix alias is read again, so a regular alias in it
/// expands. A regular alias wins over a suffix alias for the same word. A
/// suffix alias in its own value is not expanded again.
#[test]
fn a_suffix_alias_works_with_the_other_aliases() {
    let aliases = kinds(
        [("wtm", "wt --main"), ("x.md", "prcp")],
        [],
        [("txt", "wtm"), ("md", "glow"), ("rs", "y.rs")],
    );
    check_with(
        &aliases,
        vec![
            (
                "f.txt",
                words(vec![word("wt", ["--main", "f.txt"]).through("*.txt")]),
            ),
            ("x.md", words(vec![word("prcp", []).through("x.md")])),
            ("y.md", words(vec![word("glow", ["y.md"]).through("*.md")])),
            ("a.rs", words(vec![word("y.rs", ["a.rs"]).through("*.rs")])),
        ],
    );
}
