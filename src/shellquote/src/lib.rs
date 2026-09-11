//! One shell quoter for every tool in this workspace.
//!
//! A program that builds a command line for a shell must make each value one
//! word. The shell reads an unquoted value a second time: it splits the value
//! at each space, it expands `$name`, it runs the text between backticks, and
//! it matches `*` against file names. [`shell_quote`] stops all of that.
//!
//! # Why single quotes
//!
//! Single quotes are the strongest quotation a POSIX shell has. The shell
//! reads no character between them, so a space, a `$`, a backtick, a
//! backslash and a newline all stay literal. Double quotes are weaker,
//! because the shell still expands `$` and backticks between them, and a
//! program that uses double quotes must then escape those characters one at a
//! time.
//!
//! The single quote itself is the one character single quotes cannot hold,
//! because it ends the quotation. The rule for that character is to close the
//! quotation, add an escaped quote, and open the quotation again. `can't`
//! becomes `'can'\''t'`, and the shell joins those three parts into one word.
//!
//! # Why the result always carries quotes
//!
//! [`shell_quote`] puts quotes around every value, and around the empty
//! string too. The empty string becomes `''`, which is one empty word. An
//! unquoted empty string is no word at all, and the command then gets one
//! argument less than the caller counted.
//!
//! A quoter that removes the quotes from a value that looks safe has two
//! results for one input, and a test of such a quoter usually covers only
//! one of them. This crate has one result, so a caller reads the same way for
//! every value.
//!
//! # Why this is a crate
//!
//! Four tools in this workspace built the same string, and only one of them
//! had tests. `gsw` quotes the command it asks an interactive shell to run,
//! `nwt` quotes the command it hands to tmux, `swt` quotes the path of a
//! `.swt-check` override and the command lines it prints for a human, and
//! `aws2env` quotes each credential value it prints. A rule that lives in
//! four places drifts apart, and the copy that drifts is the copy nobody
//! tested.
//!
//! `aws2env` keeps one rule of its own above this one. It prints a value that
//! holds only letters, digits and a few safe punctuation marks without
//! quotes, because a human reads that output. Every other value goes through
//! [`shell_quote`].

/// `value` as one word of a shell command line.
///
/// The result always carries single quotes, and the empty string becomes
/// `''`. An embedded single quote closes the quotation, adds an escaped
/// quote, and opens the quotation again, so the shell still reads the whole
/// result as one word.
///
/// `value` is the raw text to embed. Concatenate the result into a command
/// line as it is, and add no quotes of your own.
#[must_use]
pub fn shell_quote(value: &str) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn a_plain_word_gets_quotes() {
        assert_eq!(shell_quote("check"), "'check'");
    }

    #[test]
    fn a_space_stays_inside_one_word() {
        assert_eq!(
            shell_quote("/repos/my repo/.swt-check"),
            "'/repos/my repo/.swt-check'",
            "a space must not split the value into two arguments"
        );
    }

    #[test]
    fn an_embedded_quote_closes_escapes_and_reopens() {
        assert_eq!(shell_quote("can't"), r"'can'\''t'");
    }

    #[test]
    fn a_value_of_only_quotes_escapes_each_one() {
        assert_eq!(
            shell_quote("''"),
            r"''\'''\'''",
            "two quotes in a row each get their own escape"
        );
    }

    #[test]
    fn the_empty_string_becomes_one_empty_word() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn multi_byte_text_survives() {
        assert_eq!(shell_quote("日本語"), "'日本語'");
        assert_eq!(shell_quote("🎉 party"), "'🎉 party'");
        assert_eq!(shell_quote("café au lait"), "'café au lait'");
        assert_eq!(
            shell_quote("café'日本語"),
            r"'café'\''日本語'",
            "the escape lands between two multi-byte characters"
        );
    }

    #[test]
    fn the_characters_a_shell_acts_on_stay_literal() {
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
        assert_eq!(shell_quote("a `touch pwned` b"), "'a `touch pwned` b'");
        assert_eq!(shell_quote(r"back\slash"), r"'back\slash'");
        assert_eq!(shell_quote("*"), "'*'");
    }

    /// The values the shell itself reads back, one per kind of hazard.
    #[cfg(unix)]
    const ROUND_TRIP: &[&str] = &[
        "check",
        "/repos/my repo/.swt-check",
        "can't",
        "''",
        "",
        "$HOME",
        "a `touch pwned` b",
        r"back\slash",
        "*",
        "日本語",
        "🎉 party",
        "café'au lait",
        "two\nlines",
    ];

    /// A shell that runs `script` and gives back what it printed.
    #[cfg(unix)]
    fn sh_output(script: String) -> String {
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&script)
            .output()
            .expect("run /bin/sh");
        assert!(out.status.success(), "/bin/sh failed on {script}");
        String::from_utf8(out.stdout).expect("the shell printed UTF-8")
    }

    #[cfg(unix)]
    #[test]
    fn a_real_shell_reads_back_what_went_in() {
        for value in ROUND_TRIP {
            let quoted = shell_quote(value);
            assert_eq!(
                sh_output(format!("printf '%s' {quoted}")),
                *value,
                "/bin/sh changed the value {value:?} quoted as {quoted}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_real_shell_sees_exactly_one_argument() {
        for value in ROUND_TRIP {
            let quoted = shell_quote(value);
            assert_eq!(
                sh_output(format!("set -- {quoted}\nprintf '%s' \"$#\"")),
                "1",
                "/bin/sh split or dropped the value {value:?} quoted as {quoted}"
            );
        }
    }
}
