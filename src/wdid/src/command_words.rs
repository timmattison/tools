//! The command words of one command line.
//!
//! A person types a line such as `cat x | prhash` or `echo $(prcp x)`. Which
//! words of that line start a program? That question names a syntactic
//! category, so this module parses the line with a shell parser and walks the
//! tree. It does not search the text for names.
//!
//! A wrapper runs another command. The line `sudo -u root prcp x` runs `prcp`,
//! so the command word is `prcp` and its arguments are `x`. The walk skips a
//! wrapper, the options of the wrapper, and the words that the wrapper takes
//! for itself. It repeats that for a wrapper inside a wrapper. [`WRAPPERS`]
//! holds the names and the option tables. A word that holds an expansion ends
//! the options and has no literal text, so a wrapper that runs it gives no
//! word. A wrapper that is not in the table makes a count too low, never too
//! high.
//!
//! An alias expands as the shell expands it, at command position only.
//!
//! - The command word is an alias when its raw text equals the name of an
//!   alias. A quote or a backslash in the word stops the expansion. An alias
//!   that the walk already expanded on the path to the word is not expanded
//!   again, thus `ls='ls -G'` gives `ls` with the arguments `-G x` for `ls x`
//!   and an alias cycle ends.
//! - The new text is the value of the alias, then the raw text of each word of
//!   the suffix, with one space between them. The walk reads the new text as a
//!   program. The value can hold a pipe or a substitution, and then it gives
//!   more than one command word. The redirects and the process substitutions
//!   of the suffix stay outside the new text. The walk reads them after it.
//! - A value that ends with a blank also expands the next word, when that word
//!   is an alias. This repeats while the value just put in ends with a blank.
//!   The wrappers run after the expansion. A word after a wrapper is not
//!   expanded, except through this rule.
//! - The chain of expanded names goes into each substitution. An alias
//!   expansion counts as one level of nesting, as a substitution does.
//!
//! Each command word has an origin, which is [`CommandWord::alias`]. A word
//! that comes from the value of an alias has the origin of the word that the
//! value replaced. That is the origin of the replaced word when it has one,
//! and else the name of the alias. A word that comes from the suffix keeps its
//! own origin. A word in a substitution has the origin of the word that holds
//! the substitution. The walk keeps the origin of each part of the new text as
//! a range of character indexes, because the parser counts characters.
//!
//! This module owns the parser. No type of the parser is part of a public
//! signature, and no other module sees one.

use std::collections::HashMap;
use std::io::BufReader;
use std::ops::Range;

use brush_parser::ast;
use brush_parser::word::{self, TildeExpr, WordPiece, WordPieceWithSource};
use brush_parser::{Parser, ParserOptions};

/// The alias table: the name of each alias and its value, after quote removal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Aliases {
    values: HashMap<String, String>,
}

impl Aliases {
    /// Makes an empty alias table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the alias `name` with the value `value`. An alias with the same
    /// name is replaced.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.values.insert(name.into(), value.into());
    }

    /// Gives the value of the alias `name`, or `None` when there is no such alias.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Gives the number of aliases.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// True when the table holds no alias.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl FromIterator<(String, String)> for Aliases {
    fn from_iter<T: IntoIterator<Item = (String, String)>>(iter: T) -> Self {
        Self {
            values: iter.into_iter().collect(),
        }
    }
}

/// One word in command position, with its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandWord {
    /// The command name, after quote removal.
    pub word: String,
    /// The arguments, after quote removal. An argument that holds an
    /// expansion (a parameter, a command substitution, an arithmetic
    /// expansion) keeps its raw text.
    pub args: Vec<String>,
    /// The alias that the person typed, when this command word came from the
    /// value of an alias. `None` when the person typed the word.
    pub alias: Option<String>,
}

impl CommandWord {
    /// Makes a word that the person typed. For example
    /// `CommandWord::new("prcp", ["a", "b"])`.
    #[must_use]
    pub fn new<I, S>(word: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            word: word.into(),
            args: args.into_iter().map(Into::into).collect(),
            alias: None,
        }
    }

    /// Gives the same word, from the alias `alias`.
    #[must_use]
    pub fn through(self, alias: impl Into<String>) -> Self {
        Self {
            alias: Some(alias.into()),
            ..self
        }
    }
}

/// The result of the parse of one command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// The line parsed. The words are in source order.
    Words(Vec<CommandWord>),
    /// The parser refused the line, or a part of it. No word is known.
    Unparsed,
}

/// The most levels of nesting that the walk reads. A substitution inside a
/// substitution adds one level. A line that nests deeper is refused.
const MAX_DEPTH: usize = 64;

/// The parser refused a part of the line. The walk stops at once, and the whole
/// line is [`Parsed::Unparsed`]. No part is skipped.
struct Refused;

/// One text that the walk reads: the line, or the text inside a substitution.
///
/// Every step of the walk takes the context of the text that it reads. Thus a
/// step can state where it is in the nesting.
struct TextContext<'t> {
    /// The text that the parser reads.
    text: &'t str,
    /// The number of substitutions and alias expansions that hold this text.
    /// The line has depth 0.
    depth: usize,
    /// The names of the aliases that the walk already expanded on the path to
    /// this text. An alias in the chain is not expanded again.
    chain: Vec<String>,
    /// The origin of each part of the text, as a range of character indexes.
    /// The parser counts characters, not bytes.
    parts: Vec<(Range<usize>, Option<String>)>,
    /// The origin of a position that is in no part.
    default_origin: Option<String>,
}

impl<'t> TextContext<'t> {
    /// Makes the context of the line that the person typed.
    const fn line(text: &'t str) -> Self {
        Self {
            text,
            depth: 0,
            chain: Vec::new(),
            parts: Vec::new(),
            default_origin: None,
        }
    }

    /// Makes the context of the text inside a substitution of this text. The
    /// substitution keeps the chain. It has no part, and every word in it has
    /// the origin `origin`, which is the origin of the word that holds it.
    /// Gives `Refused` when the nesting is too deep.
    fn inside<'u>(&self, text: &'u str, origin: Option<&str>) -> Result<TextContext<'u>, Refused> {
        self.nested(text, Vec::new(), Vec::new(), origin.map(str::to_owned))
    }

    /// Makes the context of the text that an alias expansion builds. The text
    /// has the parts `parts`. The chain gets the alias names `names`. A word
    /// that is in no part has the origin of this context.
    /// Gives `Refused` when the nesting is too deep.
    fn expanded<'u>(
        &self,
        text: &'u str,
        parts: Vec<(Range<usize>, Option<String>)>,
        names: Vec<String>,
    ) -> Result<TextContext<'u>, Refused> {
        self.nested(text, parts, names, self.default_origin.clone())
    }

    /// Makes the context of a text that one more level holds. The chain of the
    /// new context is the chain of this one plus `names`.
    fn nested<'u>(
        &self,
        text: &'u str,
        parts: Vec<(Range<usize>, Option<String>)>,
        names: Vec<String>,
        default_origin: Option<String>,
    ) -> Result<TextContext<'u>, Refused> {
        let depth = self.depth + 1;
        if depth > MAX_DEPTH {
            return Err(Refused);
        }
        let mut chain = self.chain.clone();
        chain.extend(names);
        Ok(TextContext {
            text,
            depth,
            chain,
            parts,
            default_origin,
        })
    }

    /// Gives the origin of the word that starts at the character index
    /// `index`: the origin of the part that holds the index, or else the
    /// default origin. A word without a position has the default origin.
    fn origin_at(&self, index: Option<usize>) -> Option<String> {
        index
            .and_then(|index| {
                self.parts
                    .iter()
                    .find(|(range, _)| range.contains(&index))
                    .map(|(_, origin)| origin.clone())
            })
            .unwrap_or_else(|| self.default_origin.clone())
    }

    /// True when the walk already expanded the alias `name` on the path to
    /// this text.
    fn in_chain(&self, name: &str) -> bool {
        self.chain.iter().any(|expanded| expanded == name)
    }
}

/// The text that an alias expansion builds, with the origin of each part.
#[derive(Default)]
struct ExpandedText {
    /// The text so far.
    text: String,
    /// The number of characters in `text`.
    characters: usize,
    /// The origin of each part, as a range of character indexes.
    parts: Vec<(Range<usize>, Option<String>)>,
}

impl ExpandedText {
    /// Adds `piece` after one space, and states that it has the origin
    /// `origin`. The first piece has no space before it.
    fn push(&mut self, piece: &str, origin: Option<String>) {
        if !self.text.is_empty() {
            self.text.push(SEPARATOR);
            self.characters += 1;
        }
        let start = self.characters;
        self.text.push_str(piece);
        self.characters += piece.chars().count();
        self.parts.push((start..self.characters, origin));
    }
}

/// The text that joins the pieces of an expanded alias.
const SEPARATOR: char = ' ';

/// The characters that make an alias expand the next word too, when the value
/// of the alias ends with one of them.
const BLANKS: [char; 2] = [' ', '\t'];

/// The backslash that starts an escape sequence.
const BACKSLASH: char = '\\';

/// The newline. A backslash before it joins two lines, so it gives no character.
const NEWLINE: char = '\n';

/// The text that stands for the home directory of the current user.
const HOME_TILDE: &str = "~";

/// One word of the line, read once: its raw text and its pieces.
struct ReadWord {
    /// The text as the person typed it, quotes included.
    raw: String,
    /// The character index where the word starts in its text, when the parser
    /// states it.
    start: Option<usize>,
    /// The pieces that the word parser found.
    pieces: Vec<WordPieceWithSource>,
}

impl ReadWord {
    /// Gives the word after quote removal, or `None` when the word holds an
    /// expansion. A word with an expansion has no literal text.
    fn literal(&self) -> Option<String> {
        literal_of(&self.pieces)
    }

    /// Gives the literal text of the word, or its raw text when it has none.
    /// This is the form of an argument.
    fn argument(&self) -> String {
        self.literal().unwrap_or_else(|| self.raw.clone())
    }
}

/// Removes the quotes from the pieces of a word and joins them. Gives `None`
/// when one piece is not plain text (a parameter, a substitution, an arithmetic
/// expansion, an ANSI-C string, or a tilde that is not `~` or `~user`).
fn literal_of(pieces: &[WordPieceWithSource]) -> Option<String> {
    let mut text = String::new();
    for WordPieceWithSource { piece, .. } in pieces {
        match piece {
            WordPiece::Text(plain) | WordPiece::SingleQuotedText(plain) => text.push_str(plain),
            WordPiece::EscapeSequence(escape) => text.push_str(unescape(escape)),
            WordPiece::DoubleQuotedSequence(inner) => text.push_str(&literal_of(inner)?),
            WordPiece::TildeExpansion(TildeExpr::Home) => text.push_str(HOME_TILDE),
            WordPiece::TildeExpansion(TildeExpr::UserHome(user)) => {
                text.push_str(HOME_TILDE);
                text.push_str(user);
            }
            _ => return None,
        }
    }
    Some(text)
}

/// Removes the backslash from an escape sequence such as `\ `. The parser
/// keeps the backslash in the piece. A backslash before a newline is a line
/// join, so it gives nothing.
fn unescape(escape: &str) -> &str {
    let rest = escape.strip_prefix(BACKSLASH).unwrap_or(escape);
    if rest.starts_with(NEWLINE) {
        ""
    } else {
        rest
    }
}

/// The word that ends the options of a command.
const END_OF_OPTIONS: &str = "--";

/// The character that starts an option.
const OPTION_START: char = '-';

/// The prefix of a long option.
const LONG_OPTION_PREFIX: &str = "--";

/// The character that joins the name of a long option to its value, and the
/// name of a variable to its value.
const ASSIGNMENT: char = '=';

/// The short options of `command` that look a name up and run nothing.
const LOOKUP_OPTIONS: [char; 2] = ['v', 'V'];

/// The rules to skip one wrapper: how to find the word that the wrapper runs.
struct Wrapper {
    /// The name of the wrapper.
    name: &'static str,
    /// The short options that take an argument.
    short_with_argument: &'static str,
    /// The long options that take an argument.
    long_with_argument: &'static [&'static str],
    /// True when a lone `-` is an option of the wrapper.
    lone_dash_is_option: bool,
    /// True when the wrapper skips the `NAME=value` words after its options.
    skips_assignments: bool,
    /// The number of words that follow the options and the assignments, and
    /// that the wrapper takes for itself. For example the duration of `timeout`.
    positional: usize,
    /// True when `-v` or `-V` among the options makes the wrapper look a name
    /// up. The line then runs nothing.
    lookup_options: bool,
}

/// A wrapper with no option that takes an argument. The wrapper tables below
/// change the fields that differ.
const PLAIN: Wrapper = Wrapper {
    name: "",
    short_with_argument: "",
    long_with_argument: &[],
    lone_dash_is_option: false,
    skips_assignments: false,
    positional: 0,
    lookup_options: false,
};

/// Every wrapper that the walk knows, with its option tables.
const WRAPPERS: &[Wrapper] = &[
    Wrapper {
        name: "sudo",
        short_with_argument: "ugCDprtTUR",
        long_with_argument: &[
            "user",
            "group",
            "close-from",
            "chdir",
            "prompt",
            "role",
            "type",
            "command-timeout",
            "other-user",
            "chroot",
        ],
        skips_assignments: true,
        ..PLAIN
    },
    Wrapper {
        name: "doas",
        short_with_argument: "uCa",
        ..PLAIN
    },
    Wrapper {
        name: "time",
        short_with_argument: "of",
        long_with_argument: &["output", "format"],
        ..PLAIN
    },
    Wrapper {
        name: "nice",
        short_with_argument: "n",
        long_with_argument: &["adjustment"],
        ..PLAIN
    },
    Wrapper {
        name: "nohup",
        ..PLAIN
    },
    Wrapper {
        name: "env",
        short_with_argument: "uCPS",
        long_with_argument: &["unset", "chdir", "split-string"],
        lone_dash_is_option: true,
        skips_assignments: true,
        ..PLAIN
    },
    Wrapper {
        name: "command",
        lookup_options: true,
        ..PLAIN
    },
    Wrapper {
        name: "exec",
        short_with_argument: "a",
        ..PLAIN
    },
    Wrapper {
        name: "noglob",
        ..PLAIN
    },
    Wrapper {
        name: "builtin",
        ..PLAIN
    },
    Wrapper {
        name: "caffeinate",
        short_with_argument: "tw",
        ..PLAIN
    },
    Wrapper {
        name: "xargs",
        short_with_argument: "EIJLnPRSsad",
        long_with_argument: &[
            "arg-file",
            "delimiter",
            "max-lines",
            "max-args",
            "max-procs",
            "max-chars",
            "process-slot-var",
        ],
        ..PLAIN
    },
    Wrapper {
        name: "watch",
        short_with_argument: "n",
        long_with_argument: &["interval"],
        ..PLAIN
    },
    Wrapper {
        name: "timeout",
        short_with_argument: "sk",
        long_with_argument: &["signal", "kill-after"],
        positional: 1,
        ..PLAIN
    },
    Wrapper {
        name: "gtimeout",
        short_with_argument: "sk",
        long_with_argument: &["signal", "kill-after"],
        positional: 1,
        ..PLAIN
    },
];

/// What one word is, when a wrapper reads it as an option.
enum OptionForm {
    /// The word is not an option. The options end before it.
    NotAnOption,
    /// The word is `--`. It is the last word of the options.
    EndOfOptions,
    /// The word is an option that takes no next word.
    Alone,
    /// The word is an option, and the next word is its argument.
    WithNextWord,
}

/// What a wrapper learns from one word, when it reads the word as an option.
struct ReadOption {
    /// The form of the word.
    form: OptionForm,
    /// True when the word holds a `-v` or `-V` option.
    lookup: bool,
}

impl Wrapper {
    /// Reads `text` as an option of this wrapper, in the getopt form.
    fn read_option(&self, text: &str) -> ReadOption {
        let mut read = ReadOption {
            form: OptionForm::NotAnOption,
            lookup: false,
        };
        if text == END_OF_OPTIONS {
            read.form = OptionForm::EndOfOptions;
        } else if let Some(name) = text.strip_prefix(LONG_OPTION_PREFIX) {
            read.form = if name.contains(ASSIGNMENT) || !self.long_with_argument.contains(&name) {
                OptionForm::Alone
            } else {
                OptionForm::WithNextWord
            };
        } else if let Some(cluster) = text.strip_prefix(OPTION_START) {
            if cluster.is_empty() {
                if self.lone_dash_is_option {
                    read.form = OptionForm::Alone;
                }
                return read;
            }
            read.form = OptionForm::Alone;
            let mut characters = cluster.chars();
            while let Some(character) = characters.next() {
                if self.lookup_options && LOOKUP_OPTIONS.contains(&character) {
                    read.lookup = true;
                }
                if self.short_with_argument.contains(character) {
                    if characters.as_str().is_empty() {
                        read.form = OptionForm::WithNextWord;
                    }
                    break;
                }
            }
        }
        read
    }

    /// Gives the index of the word that follows this wrapper and its options.
    /// `start` is the index of the first word after the wrapper. Gives `None`
    /// when the wrapper looks a name up and runs nothing.
    fn skip(&self, words: &[&ReadWord], start: usize) -> Option<usize> {
        let mut index = start;
        let mut lookup = false;
        while let Some(text) = words.get(index).and_then(|word| word.literal()) {
            let read = self.read_option(&text);
            lookup |= read.lookup;
            match read.form {
                OptionForm::NotAnOption => break,
                OptionForm::EndOfOptions => {
                    index += 1;
                    break;
                }
                OptionForm::Alone => index += 1,
                OptionForm::WithNextWord => index += 2,
            }
        }
        if self.skips_assignments {
            while words
                .get(index)
                .is_some_and(|word| is_assignment(&word.argument()))
            {
                index += 1;
            }
        }
        (!lookup).then_some(index + self.positional)
    }
}

/// True when `text` has the form `NAME=value`. The name starts with a letter or
/// `_`, and holds only ASCII letters, digits, and `_`.
fn is_assignment(text: &str) -> bool {
    let Some((name, _value)) = text.split_once(ASSIGNMENT) else {
        return false;
    };
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Gives the index of the word that a command really runs. `words` holds the
/// command word and then the words after it, in order. The first word is the
/// candidate. A wrapper hands the candidate to the word after its options, and
/// the loop repeats for a wrapper inside a wrapper. Gives `None` when nothing
/// runs: no word is left, the candidate holds an expansion, or the wrapper
/// looks a name up.
fn unwrap_command(words: &[&ReadWord]) -> Option<usize> {
    let mut index = 0;
    loop {
        let literal = words.get(index)?.literal()?;
        let Some(wrapper) = WRAPPERS.iter().find(|wrapper| wrapper.name == literal) else {
            return Some(index);
        };
        index = wrapper.skip(words, index + 1)?;
    }
}

/// Gives the character index where `word` starts, when the parser states it.
fn start_of(word: &ast::Word) -> Option<usize> {
    word.loc.as_ref().map(|span| span.start.index)
}

/// The state of one walk over one line.
struct Walker<'a> {
    /// The aliases that the walk expands.
    aliases: &'a Aliases,
    /// The options of the parser. The same value serves every parse.
    options: ParserOptions,
    /// The command words found so far, in source order.
    words: Vec<CommandWord>,
}

impl<'a> Walker<'a> {
    /// Makes a walker with no word.
    fn new(aliases: &'a Aliases) -> Self {
        Self {
            aliases,
            options: ParserOptions::default(),
            words: Vec::new(),
        }
    }

    /// Parses the text of `context` as a program and walks it.
    fn walk_text(&mut self, context: &TextContext<'_>) -> Result<(), Refused> {
        let program = self.parse_program(context.text)?;
        self.walk_program(&program, context)
    }

    /// Parses `text` as a program.
    fn parse_program(&self, text: &str) -> Result<ast::Program, Refused> {
        Parser::new(BufReader::new(text.as_bytes()), &self.options)
            .parse_program()
            .map_err(|_| Refused)
    }

    /// Walks each complete command of a program.
    fn walk_program(
        &mut self,
        program: &ast::Program,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        for list in &program.complete_commands {
            self.walk_list(list, context)?;
        }
        Ok(())
    }

    /// Walks each item of a list, in source order.
    fn walk_list(
        &mut self,
        list: &ast::CompoundList,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        for ast::CompoundListItem(and_or, _separator) in &list.0 {
            self.walk_and_or(and_or, context)?;
        }
        Ok(())
    }

    /// Walks the first pipeline of an and-or list, and then each pipeline that
    /// follows an `&&` or a `||`.
    fn walk_and_or(
        &mut self,
        and_or: &ast::AndOrList,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        self.walk_pipeline(&and_or.first, context)?;
        for additional in &and_or.additional {
            let (ast::AndOr::And(pipeline) | ast::AndOr::Or(pipeline)) = additional;
            self.walk_pipeline(pipeline, context)?;
        }
        Ok(())
    }

    /// Walks each command of a pipeline. The `time` and `!` prefixes change
    /// nothing, because the commands stay the same.
    fn walk_pipeline(
        &mut self,
        pipeline: &ast::Pipeline,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        for command in &pipeline.seq {
            self.walk_command(command, context)?;
        }
        Ok(())
    }

    /// Walks one command. A function definition is not a run, so it adds nothing.
    fn walk_command(
        &mut self,
        command: &ast::Command,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        match command {
            ast::Command::Simple(simple) => self.walk_simple(simple, context),
            ast::Command::Compound(compound, redirects) => {
                self.walk_compound(compound, context)?;
                self.walk_redirect_list(redirects.as_ref(), context)
            }
            ast::Command::ExtendedTest(test, redirects) => {
                self.walk_test_expression(&test.expr, context)?;
                self.walk_redirect_list(redirects.as_ref(), context)
            }
            ast::Command::Function(_) => Ok(()),
        }
    }

    /// Walks a compound command. The condition and the body of each clause run.
    /// The patterns of a `case` and the expressions of an arithmetic command do
    /// not start programs.
    fn walk_compound(
        &mut self,
        compound: &ast::CompoundCommand,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        use ast::CompoundCommand as Compound;
        match compound {
            Compound::Arithmetic(_) => Ok(()),
            Compound::ArithmeticForClause(clause) => self.walk_list(&clause.body.list, context),
            Compound::BraceGroup(group) => self.walk_list(&group.list, context),
            Compound::Subshell(subshell) => self.walk_list(&subshell.list, context),
            Compound::ForClause(clause) => {
                for value in clause.values.iter().flatten() {
                    self.walk_word(value, context)?;
                }
                self.walk_list(&clause.body.list, context)
            }
            Compound::CaseClause(clause) => {
                self.walk_word(&clause.value, context)?;
                for item in &clause.cases {
                    if let Some(list) = &item.cmd {
                        self.walk_list(list, context)?;
                    }
                }
                Ok(())
            }
            Compound::IfClause(clause) => {
                self.walk_list(&clause.condition, context)?;
                self.walk_list(&clause.then, context)?;
                for other in clause.elses.iter().flatten() {
                    if let Some(condition) = &other.condition {
                        self.walk_list(condition, context)?;
                    }
                    self.walk_list(&other.body, context)?;
                }
                Ok(())
            }
            Compound::WhileClause(clause) | Compound::UntilClause(clause) => {
                self.walk_list(&clause.0, context)?;
                self.walk_list(&clause.1.list, context)
            }
            Compound::Coprocess(coprocess) => self.walk_command(&coprocess.body, context),
        }
    }

    /// Walks a simple command: the prefix items, then the command word, then
    /// the suffix items. A command with no command word adds no word. A command
    /// word that holds an expansion has no literal, so it adds no word either.
    /// An alias at the command word expands first. A wrapper hands the command
    /// word to the word that it runs.
    fn walk_simple(
        &mut self,
        simple: &ast::SimpleCommand,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        let command = simple
            .word_or_name
            .as_ref()
            .map(|word| self.read_word(word))
            .transpose()?;
        let suffix = self.read_items(simple.suffix.as_ref().map(|suffix| suffix.0.as_slice()))?;
        for item in simple.prefix.iter().flat_map(|prefix| &prefix.0) {
            self.walk_item(item, None, context)?;
        }
        if let Some(command) = &command {
            if let Some(value) = self.alias_value(&command.raw, context, &[]) {
                return self.walk_alias(command, value, &suffix, context);
            }
            let sequence: Vec<&ReadWord> = std::iter::once(command)
                .chain(suffix.iter().filter_map(|(_, read)| read.as_ref()))
                .collect();
            if let Some(index) = unwrap_command(&sequence) {
                if let (Some(runs), Some(rest)) = (sequence.get(index), sequence.get(index + 1..)) {
                    if let Some(literal) = runs.literal() {
                        let args = rest.iter().map(|word| word.argument());
                        let mut word = CommandWord::new(literal, args);
                        word.alias = context.origin_at(runs.start);
                        self.words.push(word);
                    }
                }
            }
            self.walk_read_word(command, context)?;
        }
        for (item, read) in &suffix {
            self.walk_item(item, read.as_ref(), context)?;
        }
        Ok(())
    }

    /// Gives the value of the alias `name`, when the walk expands it here. An
    /// alias is not expanded when its name is in the chain of `context`, or
    /// among the `names` that the current expansion added.
    fn alias_value(
        &self,
        name: &str,
        context: &TextContext<'_>,
        names: &[String],
    ) -> Option<&'a str> {
        if context.in_chain(name) || names.iter().any(|expanded| expanded == name) {
            return None;
        }
        self.aliases.get(name)
    }

    /// Expands the alias that is the command word `command`, and walks the
    /// result. The new text is the value, then the raw text of each word of the
    /// suffix. A value that ends with a blank also expands the next word, when
    /// that word is an alias. The redirects and the process substitutions of the
    /// suffix stay outside the new text, and the walk reads them after it.
    fn walk_alias(
        &mut self,
        command: &ReadWord,
        value: &str,
        suffix: &[(&ast::CommandPrefixOrSuffixItem, Option<ReadWord>)],
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        let mut names = vec![command.raw.clone()];
        let mut expanded = ExpandedText::default();
        let replaced_origin = |name: &str, start: Option<usize>| {
            context.origin_at(start).or_else(|| Some(name.to_owned()))
        };
        expanded.push(value, replaced_origin(&command.raw, command.start));
        let mut rest = suffix
            .iter()
            .filter_map(|(_, read)| read.as_ref())
            .peekable();
        let mut last = value;
        while last.ends_with(BLANKS) {
            let Some(next) = rest.peek() else { break };
            let Some(next_value) = self.alias_value(&next.raw, context, &names) else {
                break;
            };
            expanded.push(next_value, replaced_origin(&next.raw, next.start));
            names.push(next.raw.clone());
            last = next_value;
            rest.next();
        }
        for word in rest {
            expanded.push(&word.raw, context.origin_at(word.start));
        }
        let ExpandedText { text, parts, .. } = expanded;
        self.walk_text(&context.expanded(&text, parts, names)?)?;
        for (item, read) in suffix {
            if read.is_none() {
                self.walk_item(item, None, context)?;
            }
        }
        Ok(())
    }

    /// Reads the word of each item. A redirect and a process substitution hold
    /// no word of the command, so they give `None`.
    fn read_items<'i>(
        &self,
        items: Option<&'i [ast::CommandPrefixOrSuffixItem]>,
    ) -> Result<Vec<(&'i ast::CommandPrefixOrSuffixItem, Option<ReadWord>)>, Refused> {
        items
            .unwrap_or_default()
            .iter()
            .map(|item| {
                let read = match item {
                    ast::CommandPrefixOrSuffixItem::Word(word)
                    | ast::CommandPrefixOrSuffixItem::AssignmentWord(_, word) => {
                        Some(self.read_word(word)?)
                    }
                    ast::CommandPrefixOrSuffixItem::IoRedirect(_)
                    | ast::CommandPrefixOrSuffixItem::ProcessSubstitution(..) => None,
                };
                Ok((item, read))
            })
            .collect()
    }

    /// Walks the substitutions of one prefix or suffix item. `read` is the word
    /// of the item when the caller read it already.
    fn walk_item(
        &mut self,
        item: &ast::CommandPrefixOrSuffixItem,
        read: Option<&ReadWord>,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        match item {
            ast::CommandPrefixOrSuffixItem::IoRedirect(redirect) => {
                self.walk_redirect(redirect, context)
            }
            ast::CommandPrefixOrSuffixItem::Word(word)
            | ast::CommandPrefixOrSuffixItem::AssignmentWord(_, word) => match read {
                Some(read) => self.walk_read_word(read, context),
                None => self.walk_word(word, context),
            },
            ast::CommandPrefixOrSuffixItem::ProcessSubstitution(_, subshell) => {
                self.walk_list(&subshell.list, context)
            }
        }
    }

    /// Walks the redirects of a compound command, when there are any.
    fn walk_redirect_list(
        &mut self,
        redirects: Option<&ast::RedirectList>,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        for redirect in redirects.iter().flat_map(|list| &list.0) {
            self.walk_redirect(redirect, context)?;
        }
        Ok(())
    }

    /// Walks the substitutions of one redirect. The body of a here document
    /// whose delimiter is quoted is plain text, so nothing in it runs.
    fn walk_redirect(
        &mut self,
        redirect: &ast::IoRedirect,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        match redirect {
            ast::IoRedirect::File(_, _, target) => match target {
                ast::IoFileRedirectTarget::Filename(word)
                | ast::IoFileRedirectTarget::Duplicate(word) => self.walk_word(word, context),
                ast::IoFileRedirectTarget::Fd(_) => Ok(()),
                ast::IoFileRedirectTarget::ProcessSubstitution(_, subshell) => {
                    self.walk_list(&subshell.list, context)
                }
            },
            ast::IoRedirect::HereDocument(_, document) => {
                if !document.requires_expansion {
                    return Ok(());
                }
                let pieces =
                    word::parse_heredoc(&document.doc.value, &self.options).map_err(|_| Refused)?;
                let origin = context.origin_at(start_of(&document.doc));
                self.walk_pieces(&pieces, origin.as_deref(), context)
            }
            ast::IoRedirect::HereString(_, word) | ast::IoRedirect::OutputAndError(word, _) => {
                self.walk_word(word, context)
            }
        }
    }

    /// Walks the substitutions in every word of an expression of `[[ ... ]]`.
    fn walk_test_expression(
        &mut self,
        expression: &ast::ExtendedTestExpr,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        use ast::ExtendedTestExpr as Expression;
        match expression {
            Expression::And(left, right) | Expression::Or(left, right) => {
                self.walk_test_expression(left, context)?;
                self.walk_test_expression(right, context)
            }
            Expression::Not(inner) | Expression::Parenthesized(inner) => {
                self.walk_test_expression(inner, context)
            }
            Expression::UnaryTest(_, word) => self.walk_word(word, context),
            Expression::BinaryTest(_, left, right) => {
                self.walk_word(left, context)?;
                self.walk_word(right, context)
            }
        }
    }

    /// Walks the substitutions in one word. Gives `Refused` when the parser
    /// cannot read the word.
    fn walk_word(&mut self, word: &ast::Word, context: &TextContext<'_>) -> Result<(), Refused> {
        let read = self.read_word(word)?;
        self.walk_read_word(&read, context)
    }

    /// Walks the substitutions in a word that the caller read already. They
    /// have the origin of the word.
    fn walk_read_word(
        &mut self,
        read: &ReadWord,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        let origin = context.origin_at(read.start);
        self.walk_pieces(&read.pieces, origin.as_deref(), context)
    }

    /// Parses one word into pieces. Gives `Refused` when the parser cannot read it.
    fn read_word(&self, word: &ast::Word) -> Result<ReadWord, Refused> {
        let pieces = word::parse(&word.value, &self.options).map_err(|_| Refused)?;
        Ok(ReadWord {
            raw: word.value.clone(),
            start: start_of(word),
            pieces,
        })
    }

    /// Walks the pieces of a word. A command substitution holds a program, so
    /// the walk reads it at one level deeper. A double-quoted sequence holds
    /// more pieces. The other pieces start no program. The words in a
    /// substitution have the origin `origin`.
    fn walk_pieces(
        &mut self,
        pieces: &[WordPieceWithSource],
        origin: Option<&str>,
        context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        for WordPieceWithSource { piece, .. } in pieces {
            match piece {
                WordPiece::CommandSubstitution(text)
                | WordPiece::BackquotedCommandSubstitution(text) => {
                    self.walk_text(&context.inside(text, origin)?)?;
                }
                WordPiece::DoubleQuotedSequence(inner)
                | WordPiece::GettextDoubleQuotedSequence(inner) => {
                    self.walk_pieces(inner, origin, context)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Gives each command word of one command line. Pure.
///
/// `aliases` holds the aliases of the shell. See the module documentation for
/// the rules of the expansion and of the origin of each word.
#[must_use]
pub fn command_words(line: &str, aliases: &Aliases) -> Parsed {
    let mut walker = Walker::new(aliases);
    match walker.walk_text(&TextContext::line(line)) {
        Ok(()) => Parsed::Words(walker.words),
        Err(Refused) => Parsed::Unparsed,
    }
}
