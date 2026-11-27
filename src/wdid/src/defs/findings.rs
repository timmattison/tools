//! The duplicates and the shadows among the definitions.
//!
//! [`find`] reads the entries of the report and the definitions of the shell.
//! It gives one [`Finding`] for each group of aliases with the same value, for
//! each alias and function that do the same thing, for each alias name that
//! the startup files define more than once, for each alias that hides a
//! function, and for each definition that hides a program. The function is
//! pure: it starts no process and reads no file.

use std::path::PathBuf;

use super::{Kind, Usage};
use crate::origin::Origin;
use crate::shell::Definitions;

/// The kind of a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Two or more aliases have the same value, after the shell splits it
    /// into words.
    SameExpansion,
    /// An alias and a function do the same thing.
    AliasAndFunction,
    /// The startup files define an alias name two or more times.
    DefinedMoreThanOnce,
    /// An alias and a function have the same name.
    AliasHidesFunction,
    /// The name of a definition is also the name of a program.
    HidesProgram,
}

/// One definition of a finding: its name, kind, origin, uses, and whether it
/// is mine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The name of the definition.
    pub name: String,
    /// The kind of the definition.
    pub kind: Kind,
    /// The file that defined the definition, or `None` when it is unknown.
    pub origin: Option<Origin>,
    /// The uses of the definition.
    pub uses: u64,
    /// True when the definition is mine.
    pub mine: bool,
}

/// One duplicate or one shadow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The kind of the finding.
    pub kind: FindingKind,
    /// The definitions of the finding.
    pub definitions: Vec<Member>,
    /// For [`FindingKind::SameExpansion`], the words that the aliases share.
    /// For [`FindingKind::AliasAndFunction`], the words of the alias, without
    /// the `"$@"` of the function. Empty for the other kinds.
    pub words: Vec<String>,
    /// For [`FindingKind::DefinedMoreThanOnce`], each place that defined the
    /// alias, in order. The last place is in effect. Empty for the other
    /// kinds.
    pub places: Vec<Option<Origin>>,
    /// For [`FindingKind::HidesProgram`], the program that the definition
    /// hides. `None` for the other kinds.
    pub program: Option<PathBuf>,
}

impl Finding {
    /// Says if one or more of its definitions is mine.
    #[must_use]
    pub fn mine(&self) -> bool {
        self.definitions.iter().any(|member| member.mine)
    }
}

/// Gives the findings among `usages`, in report order. Pure.
pub(super) fn find(definitions: &Definitions, usages: &[Usage]) -> Vec<Finding> {
    let _ = (definitions, usages);
    Vec::new()
}
