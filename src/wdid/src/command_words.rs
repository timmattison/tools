//! The command words of one command line.
//!
//! A person types a line such as `cat x | prhash` or `echo $(prcp x)`. Which
//! words of that line start a program? That question names a syntactic
//! category, so this module parses the line with a shell parser and walks the
//! tree. It does not search the text for names.
//!
//! This module owns the parser. No type of the parser is part of a public
//! signature, and no other module sees one.

use std::collections::HashMap;
use std::io::BufReader;

use brush_parser::ast;
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
}

/// The state of one walk over one line.
struct Walker {
    /// The options of the parser. The same value serves every parse.
    options: ParserOptions,
    /// The command words found so far, in source order.
    words: Vec<CommandWord>,
}

impl Walker {
    /// Makes a walker with no word.
    fn new() -> Self {
        Self {
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
            ast::Command::Compound(compound, _redirects) => self.walk_compound(compound, context),
            ast::Command::Function(_) | ast::Command::ExtendedTest(..) => Ok(()),
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
            Compound::ForClause(clause) => self.walk_list(&clause.body.list, context),
            Compound::CaseClause(clause) => {
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
    /// the suffix items. A command with no command word adds no word.
    #[allow(
        clippy::unnecessary_wraps,
        reason = "the walk of the substitutions in the words arrives in the next step"
    )]
    fn walk_simple(
        &mut self,
        simple: &ast::SimpleCommand,
        _context: &TextContext<'_>,
    ) -> Result<(), Refused> {
        let args = simple
            .suffix
            .iter()
            .flat_map(|suffix| &suffix.0)
            .filter_map(|item| match item {
                ast::CommandPrefixOrSuffixItem::Word(word)
                | ast::CommandPrefixOrSuffixItem::AssignmentWord(_, word) => {
                    Some(word.value.clone())
                }
                ast::CommandPrefixOrSuffixItem::IoRedirect(_)
                | ast::CommandPrefixOrSuffixItem::ProcessSubstitution(..) => None,
            });
        if let Some(command) = &simple.word_or_name {
            self.words
                .push(CommandWord::new(command.value.clone(), args));
        }
        Ok(())
    }
}

/// Gives each command word of one command line. Pure.
///
/// The alias table is accepted, and the walk does not use it yet.
#[must_use]
pub fn command_words(line: &str, _aliases: &Aliases) -> Parsed {
    let mut walker = Walker::new();
    match walker.walk_text(&TextContext { text: line }) {
        Ok(()) => Parsed::Words(walker.words),
        Err(Refused) => Parsed::Unparsed,
    }
}
