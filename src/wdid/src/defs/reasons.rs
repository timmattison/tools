//! The reasons that the shell runs a definition without a typed name.
//!
//! [`find`] starts at each trigger that [`Definitions::starts`] holds: a
//! function that a hook or a trap runs, the function of each widget, and the
//! code of each bash start. Each function that such a function calls, to any
//! depth, runs through it. The function is pure.

use std::collections::{BTreeSet, HashMap};

use super::{callees, Kind, Names, Reason};
use crate::command_words::parse_line;
use crate::shell::Definitions;
use crate::starts::{Starts, Trigger, SPECIAL_WIDGETS};

/// The reasons of each definition, by name and kind.
type Found = HashMap<(String, Kind), BTreeSet<Reason>>;

/// Gives the reasons of each definition that a start of `definitions`
/// reaches, by name and kind. A private function gets its reasons too. A
/// name that is not a function or an alias gets none.
pub(super) fn find(definitions: &Definitions, names: &Names<'_>) -> Found {
    let mut finder = Finder {
        definitions,
        names,
        found: HashMap::new(),
    };
    let starts = &definitions.starts;
    for (trigger, function) in &starts.functions {
        finder.function_runs(function, trigger, None);
    }
    for (widget, function) in &starts.widgets {
        let triggers = widget_triggers(starts, widget);
        if triggers.is_empty() && names.functions.contains(function.as_str()) {
            finder.add(function, Kind::Function, Reason::WidgetWithoutKey);
        }
        for trigger in &triggers {
            finder.function_runs(function, trigger, None);
        }
    }
    for (trigger, code) in &starts.code {
        finder.code_runs(trigger, code);
    }
    finder.found
}

/// Gives each trigger that runs the widget `widget`: the widget itself when it
/// is a special widget, the widget with its keys when a key starts it, and
/// each special widget that `add-zle-hook-widget` added it to. Gives none when
/// no key starts the widget.
fn widget_triggers(starts: &Starts, widget: &str) -> Vec<Trigger> {
    let mut triggers = Vec::new();
    if SPECIAL_WIDGETS.contains(&widget) {
        triggers.push(Trigger::SpecialWidget(widget.to_owned()));
    }
    if let Some(keys) = starts.keys.get(widget).filter(|keys| !keys.is_empty()) {
        triggers.push(Trigger::Widget {
            widget: widget.to_owned(),
            keys: keys.iter().cloned().collect(),
        });
    }
    for (special, widgets) in &starts.hook_widgets {
        if widgets.iter().any(|added| added == widget) {
            triggers.push(Trigger::SpecialWidget(special.clone()));
        }
    }
    triggers
}

/// The walk of [`find`].
struct Finder<'a> {
    definitions: &'a Definitions,
    names: &'a Names<'a>,
    found: Found,
}

impl Finder<'_> {
    /// Adds `reason` to the definition `name` of the kind `kind`.
    fn add(&mut self, name: &str, kind: Kind, reason: Reason) {
        self.found
            .entry((name.to_owned(), kind))
            .or_default()
            .insert(reason);
    }

    /// Adds the reason that `trigger` runs the function `name`, and the reason
    /// that `trigger` runs each function that `name` calls, to any depth.
    /// `through` names the definition that `trigger` runs, when `trigger`
    /// reaches `name` through it. A callee runs through `through`, else
    /// through `name`. A name that is not a function changes nothing.
    fn function_runs(&mut self, name: &str, trigger: &Trigger, through: Option<&str>) {
        if !self.names.functions.contains(name) {
            return;
        }
        let reason = |through: Option<&str>| Reason::Runs {
            trigger: trigger.clone(),
            through: through.map(str::to_owned),
        };
        self.add(name, Kind::Function, reason(through));
        let via = through.unwrap_or(name);
        for callee in callees(self.definitions, self.names, name) {
            self.add(callee, Kind::Function, reason(Some(via)));
        }
    }

    /// Adds the reasons that `trigger` runs each alias and each function that
    /// the code `code` names, as a typed line names them. Code that does not
    /// parse runs nothing.
    fn code_runs(&mut self, trigger: &Trigger, code: &str) {
        let Some(line) = parse_line(code, &self.definitions.aliases) else {
            return;
        };
        for alias_use in &line.aliases {
            if let Some(kind) = self.names.alias_kinds.get(&alias_use.name).copied() {
                let reason = Reason::Runs {
                    trigger: trigger.clone(),
                    through: alias_use.through.clone(),
                };
                self.add(&alias_use.name, kind, reason);
            }
        }
        for word in &line.words {
            self.function_runs(&word.word, trigger, word.alias.as_deref());
        }
    }
}
