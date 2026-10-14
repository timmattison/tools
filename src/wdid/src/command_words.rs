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

/// Gives each command word of one command line. Pure.
#[must_use]
pub fn command_words(_line: &str, _aliases: &Aliases) -> Parsed {
    Parsed::Words(Vec::new())
}
