//! The duplicates and the shadows among the definitions.
//!
//! [`find`] reads the entries of the report and the definitions of the shell.
//! It gives one [`Finding`] for each group of aliases with the same value, for
//! each alias and function that do the same thing, for each alias name that
//! the startup files define more than once, for each alias that hides a
//! function, and for each definition that hides a program. The function is
//! pure: it starts no process and reads no file.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use super::{Kind, Usage};
use crate::command_words::{simple_command, split_words, AliasDefinition, AliasKind};
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

impl Member {
    /// Copies the fields of `usage` that a finding shows.
    fn of(usage: &Usage) -> Self {
        Self {
            name: usage.name.clone(),
            kind: usage.kind,
            origin: usage.origin.clone(),
            uses: usage.uses,
            mine: usage.mine,
        }
    }
}

/// The last word of the body of a function that passes on its arguments.
const ALL_ARGUMENTS: &str = "\"$@\"";

/// The entries of the report, with the aliases that have an entry.
struct Entries<'a> {
    /// Each function entry, by name.
    functions: BTreeMap<&'a str, &'a Usage>,
    /// Each alias of the table that has an entry, with its entry, sorted by
    /// name, then by kind.
    aliases: Vec<(AliasDefinition, &'a Usage)>,
}

impl<'a> Entries<'a> {
    /// Matches each alias of `definitions` to its entry in `usages`.
    fn new(definitions: &Definitions, usages: &'a [Usage]) -> Self {
        let by_key: HashMap<(&str, Kind), &Usage> = usages
            .iter()
            .map(|usage| ((usage.name.as_str(), usage.kind), usage))
            .collect();
        let aliases = definitions
            .aliases
            .definitions()
            .into_iter()
            .filter_map(|alias| {
                let usage = *by_key.get(&(alias.name.as_str(), Kind::from(alias.kind)))?;
                Some((alias, usage))
            })
            .collect();
        let functions = usages
            .iter()
            .filter(|usage| usage.kind == Kind::Function)
            .map(|usage| (usage.name.as_str(), usage))
            .collect();
        Self { functions, aliases }
    }
}

/// Finds the groups of regular aliases, and the groups of global aliases,
/// that have the same words. A suffix alias takes no part, because each one
/// names different files.
fn same_expansion(entries: &Entries<'_>) -> Vec<Finding> {
    let mut groups: BTreeMap<(Kind, Vec<String>), Vec<&Usage>> = BTreeMap::new();
    for (alias, usage) in &entries.aliases {
        let kind = usage.kind;
        if kind == Kind::SuffixAlias {
            continue;
        }
        let Some(split) = split_words(&alias.value).filter(|split| !split.is_empty()) else {
            continue;
        };
        groups.entry((kind, split)).or_default().push(usage);
    }
    groups
        .into_iter()
        .filter(|(_, members)| members.len() >= 2)
        .map(|((_, shared), mut members)| {
            members.sort_by(|left, right| left.name.cmp(&right.name));
            Finding {
                kind: FindingKind::SameExpansion,
                definitions: members.into_iter().map(Member::of).collect(),
                words: shared,
                places: Vec::new(),
                program: None,
            }
        })
        .collect()
}

/// Gives the words that the body of a function runs before it passes on its
/// arguments, or `None` when the body does something else. The body must be
/// one simple command that ends with `"$@"` and has a word before it.
fn passed_on_words(body: &str) -> Option<Vec<String>> {
    let mut words = simple_command(body)?;
    if words.pop().as_deref() != Some(ALL_ARGUMENTS) || words.is_empty() {
        return None;
    }
    Some(words)
}

/// Finds each function that runs the words of one or more aliases and passes
/// on its arguments. A suffix alias takes no part.
fn alias_and_function(definitions: &Definitions, entries: &Entries<'_>) -> Vec<Finding> {
    let mut found = Vec::new();
    for function in entries.functions.values() {
        let Some(shared) = definitions
            .function_bodies
            .get(&function.name)
            .and_then(|body| passed_on_words(body))
        else {
            continue;
        };
        let mut matching: Vec<&Usage> = entries
            .aliases
            .iter()
            .filter(|(alias, _)| {
                alias.kind != AliasKind::Suffix
                    && split_words(&alias.value).as_deref() == Some(shared.as_slice())
            })
            .map(|(_, usage)| *usage)
            .collect();
        if matching.is_empty() {
            continue;
        }
        matching.sort_by(|left, right| left.name.cmp(&right.name));
        matching.push(function);
        found.push(Finding {
            kind: FindingKind::AliasAndFunction,
            definitions: matching.into_iter().map(Member::of).collect(),
            words: shared,
            places: Vec::new(),
            program: None,
        });
    }
    found
}

/// Gives `places` without each place that a later place repeats. The order
/// stays, and the last occurrence of a place stays.
fn last_occurrences(places: &[Option<Origin>]) -> Vec<Option<Origin>> {
    places
        .iter()
        .enumerate()
        .filter(|(index, place)| !places.iter().skip(index + 1).any(|later| later == *place))
        .map(|(_, place)| place.clone())
        .collect()
}

/// Finds each alias of any kind that the startup files define in two or more
/// places.
fn defined_more_than_once(definitions: &Definitions, entries: &Entries<'_>) -> Vec<Finding> {
    entries
        .aliases
        .iter()
        .filter_map(|(alias, usage)| {
            let places = last_occurrences(definitions.alias_places(alias));
            (places.len() >= 2).then(|| Finding {
                kind: FindingKind::DefinedMoreThanOnce,
                definitions: vec![Member::of(usage)],
                words: Vec::new(),
                places,
                program: None,
            })
        })
        .collect()
}

/// Says if the alias `alias` starts with its own name. zsh does not expand
/// such an alias again, thus its value runs the function or the program of
/// that name.
fn starts_with_own_name(alias: &AliasDefinition) -> bool {
    split_words(&alias.value)
        .and_then(|split| split.into_iter().next())
        .is_some_and(|first| first == alias.name)
}

/// Finds each regular or global alias that has the name of a function. An
/// alias that starts with its own name runs the function, and is no finding.
fn alias_hides_function(entries: &Entries<'_>) -> Vec<Finding> {
    entries
        .aliases
        .iter()
        .filter(|(alias, _)| alias.kind != AliasKind::Suffix && !starts_with_own_name(alias))
        .filter_map(|(alias, usage)| {
            let function = entries.functions.get(alias.name.as_str())?;
            Some(Finding {
                kind: FindingKind::AliasHidesFunction,
                definitions: vec![Member::of(usage), Member::of(function)],
                words: Vec::new(),
                places: Vec::new(),
                program: None,
            })
        })
        .collect()
}

/// Gives the findings among `usages`, sorted by kind, then by the name and the
/// kind of the first definition. Pure.
///
/// Only an entry of `usages` takes part, thus a private function takes part
/// only when the report lists it.
pub(super) fn find(definitions: &Definitions, usages: &[Usage]) -> Vec<Finding> {
    let entries = Entries::new(definitions, usages);
    let mut found = same_expansion(&entries);
    found.extend(alias_and_function(definitions, &entries));
    found.extend(defined_more_than_once(definitions, &entries));
    found.extend(alias_hides_function(&entries));
    found.sort_by(|left, right| sort_key(left).cmp(&sort_key(right)));
    found
}

/// Gives the key that orders the findings.
fn sort_key(finding: &Finding) -> (FindingKind, &str, Kind) {
    finding
        .definitions
        .first()
        .map_or((finding.kind, "", Kind::Alias), |member| {
            (finding.kind, member.name.as_str(), member.kind)
        })
}
