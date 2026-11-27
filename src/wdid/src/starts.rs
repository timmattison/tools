//! The definitions that the shell runs without a typed name.
//!
//! zsh runs a hook function before each prompt, a trap function at a signal,
//! and the function of a widget at a key. bash runs `PROMPT_COMMAND`, the code
//! of a trap, and the command of a `bind -x` key. The history holds no row for
//! such a run. Thus [`crate::shell`] asks the shell for each such start, and
//! [`Starts`] holds what the shell gives. [`crate::defs`] then finds each
//! definition that a start reaches.

use std::collections::{BTreeMap, BTreeSet};

/// One event of the shell that runs code without a typed name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Trigger {
    /// A zsh hook, by its name: the function `precmd`, or a name in the
    /// array `precmd_functions`, which `add-zsh-hook` fills.
    Hook(String),
    /// A trap, by the name of its signal without `SIG`: a zsh function
    /// `TRAPINT`, or the code of a bash `trap`.
    Trap(String),
    /// The zsh function `command_not_found_handler`.
    CommandNotFound,
    /// A zle widget, by its name, with each key that starts it, sorted. A key
    /// in more than one keymap is one key.
    Widget {
        /// The name of the widget.
        widget: String,
        /// The keys, as `bindkey -L` writes them, without the quotes.
        keys: Vec<String>,
    },
    /// A special widget, by its name, which zle runs at an event of its own,
    /// for example `zle-line-init`. `add-zle-hook-widget` adds widgets to it.
    SpecialWidget(String),
    /// The bash variable `PROMPT_COMMAND`.
    PromptCommand,
    /// A bash `bind -x` key, by its key sequence.
    KeyCommand(String),
}

/// What the shell gives about the code that it runs without a typed name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Starts {
    /// Each function that a trigger runs by its name, with the trigger: a
    /// zsh hook, a zsh trap function, and the handler of a command that zsh
    /// does not find. A name that is not a function changes nothing.
    pub functions: Vec<(Trigger, String)>,
    /// The function of each zle widget that a function implements, by the
    /// name of the widget.
    pub widgets: BTreeMap<String, String>,
    /// The keys of each zle widget in all keymaps, by the name of the widget.
    pub keys: BTreeMap<String, BTreeSet<String>>,
    /// The widgets that `add-zle-hook-widget` adds to each special widget, by
    /// the name of the special widget.
    pub hook_widgets: BTreeMap<String, Vec<String>>,
    /// Each text of code that a trigger runs, with the trigger: the bash
    /// `PROMPT_COMMAND`, the code of a bash trap, and the command of a bash
    /// `bind -x` key.
    pub code: Vec<(Trigger, String)>,
}
