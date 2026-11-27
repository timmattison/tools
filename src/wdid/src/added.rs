//! The added date of a definition: the date of the oldest commit that adds a
//! line that defines its name to its origin file.
//!
//! [`defined_name`] reads one line of a shell file and gives the name that the
//! line defines. [`added_dates`] reads the history of one file, with its full
//! diff, and gives the oldest date for each name that an added line defines.
//! Both functions are pure.

use std::collections::BTreeMap;

use chrono::NaiveDate;

/// The family of a name that a line of a shell file defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Family {
    /// An alias of one of the three kinds.
    Alias,
    /// A function.
    Function,
}

/// A name that one line of a shell file defines.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Defined {
    /// The family of the definition.
    pub family: Family,
    /// The name. A suffix alias has its marker, `*.` and the suffix (`*.md`),
    /// as in [`crate::command_words::AliasDefinition::name`].
    pub name: String,
}

impl Defined {
    /// Makes the alias `name`.
    #[must_use]
    pub fn alias(name: impl Into<String>) -> Self {
        Self {
            family: Family::Alias,
            name: name.into(),
        }
    }

    /// Makes the function `name`.
    #[must_use]
    pub fn function(name: impl Into<String>) -> Self {
        Self {
            family: Family::Function,
            name: name.into(),
        }
    }
}

/// Gives the name that `line` defines, or `None`. Pure.
#[must_use]
pub fn defined_name(_line: &str) -> Option<Defined> {
    None
}

/// Gives the oldest date of the commits that add a line that defines each
/// name. Pure.
#[must_use]
pub fn added_dates(_log: &str) -> BTreeMap<Defined, NaiveDate> {
    BTreeMap::new()
}
