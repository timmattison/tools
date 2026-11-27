//! The definitions that the shell runs without a typed name.
//!
//! zsh runs a hook function before each prompt, a trap function at a signal,
//! and the function of a widget at a key. bash runs `PROMPT_COMMAND`, the code
//! of a trap, and the command of a `bind -x` key. The history holds no row for
//! such a run. Thus [`crate::shell`] asks the shell for each such start, and
//! [`Starts`] holds what the shell gives. [`crate::defs`] then finds each
//! definition that a start reaches.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::command_words::{command_words, Aliases, CommandWord, Parsed};

/// The zsh hooks. zsh runs the function of each name, and each name in the
/// array of the name with [`HOOK_ARRAY_SUFFIX`].
pub const ZSH_HOOKS: [&str; 6] = [
    "precmd",
    "preexec",
    "chpwd",
    "periodic",
    "zshaddhistory",
    "zshexit",
];

/// The text that follows the name of a zsh hook in the name of its array,
/// for example `precmd_functions`.
pub const HOOK_ARRAY_SUFFIX: &str = "_functions";

/// The start of the name of each zsh trap function, for example `TRAPINT`.
/// The name of a signal follows it.
pub const TRAP_PREFIX: &str = "TRAP";

/// The function that zsh runs for a command that it does not find.
pub const NOT_FOUND_HANDLER: &str = "command_not_found_handler";

/// The special widgets of zle. zle runs each of them at an event of its own,
/// and `add-zle-hook-widget` adds widgets to each of them.
pub const SPECIAL_WIDGETS: [&str; 7] = [
    "zle-isearch-exit",
    "zle-isearch-update",
    "zle-line-pre-redraw",
    "zle-line-init",
    "zle-line-finish",
    "zle-history-line-set",
    "zle-keymap-select",
];

/// The command of each line that `zle -lL` writes.
const ZLE_COMMAND: &str = "zle";

/// The option of `zle` that defines a widget: `zle -N widget [function]`.
const NEW_WIDGET_OPTION: &str = "-N";

/// The option of `zle` that defines a completion widget:
/// `zle -C widget completion-widget function`.
const COMPLETION_WIDGET_OPTION: &str = "-C";

/// The command of each line that `bindkey -L` writes.
const BINDKEY_COMMAND: &str = "bindkey";

/// The option of `bindkey` that binds a key to a string, not to a widget.
const STRING_BINDING_OPTION: &str = "-s";

/// The command of each line that `trap -p` writes.
const TRAP_COMMAND: &str = "trap";

/// The word that ends the options of `trap`.
const END_OF_OPTIONS: &str = "--";

/// The start of the name of a signal in the output of the bash `trap -p`, for
/// example `SIGINT`.
const SIGNAL_PREFIX: &str = "SIG";

/// The quote around the key and around the command in the output of
/// `bind -X`.
const DOUBLE_QUOTE: char = '"';

/// The character that escapes the next character in the output of `bind -X`.
const BACKSLASH: char = '\\';

/// The character after the key in the output of `bind -X` of a bash older
/// than 5.3.
const KEY_END: char = ':';

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

/// The text that divides the keys of one widget in the text of a trigger.
const KEY_SEPARATOR: &str = ", ";

/// Gives the text of the key `key` in the text of a trigger: the key, or the
/// key in double quotes when it is empty or holds a blank, so that a reader
/// sees the space key.
fn shown_key(key: &str) -> String {
    if key.is_empty() || key.chars().any(char::is_whitespace) {
        format!("{DOUBLE_QUOTE}{key}{DOUBLE_QUOTE}")
    } else {
        key.to_owned()
    }
}

impl fmt::Display for Trigger {
    /// Writes the text of the trigger that a report shows, for example
    /// `precmd hook`, `INT trap`, or `widget atuin-search, key ^R`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hook(hook) => write!(formatter, "{hook} hook"),
            Self::Trap(signal) => write!(formatter, "{signal} trap"),
            Self::CommandNotFound => formatter.write_str("command not found"),
            Self::Widget { widget, keys } => {
                write!(formatter, "widget {widget}")?;
                let label = if keys.len() == 1 { "key" } else { "keys" };
                let shown: Vec<String> = keys.iter().map(|key| shown_key(key)).collect();
                if !shown.is_empty() {
                    write!(formatter, ", {label} {}", shown.join(KEY_SEPARATOR))?;
                }
                Ok(())
            }
            Self::SpecialWidget(widget) => write!(formatter, "widget {widget}"),
            Self::PromptCommand => formatter.write_str("PROMPT_COMMAND"),
            Self::KeyCommand(key) => write!(formatter, "bind -x, key {}", shown_key(key)),
        }
    }
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

/// Gives the one command word of `line`, when the line is exactly one call
/// of `command`. The parse uses no alias, because the shell wrote the line.
fn one_call(line: &str, command: &str) -> Option<CommandWord> {
    match command_words(line, &Aliases::new()) {
        Parsed::Words(words) => match <[CommandWord; 1]>::try_from(words) {
            Ok([word]) if word.word == command => Some(word),
            _ => None,
        },
        Parsed::Unparsed => None,
    }
}

/// Reads one text in double quotes at the start of `text`. Gives the text
/// between the quotes, with each backslash and the character after it as they
/// are, and the rest of `text` after the closing quote. Gives `None` when
/// `text` does not start with a quote or has no closing quote.
fn double_quoted(text: &str) -> Option<(&str, &str)> {
    let inner = text.strip_prefix(DOUBLE_QUOTE)?;
    let mut escaped = false;
    for (index, character) in inner.char_indices() {
        if escaped {
            escaped = false;
        } else if character == BACKSLASH {
            escaped = true;
        } else if character == DOUBLE_QUOTE {
            let rest = inner.get(index + DOUBLE_QUOTE.len_utf8()..)?;
            return Some((inner.get(..index)?, rest));
        }
    }
    None
}

/// Removes one level of backslashes from `text`: a backslash before a
/// backslash or a quote goes away. A different backslash stays.
fn unescape(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != BACKSLASH {
            result.push(character);
            continue;
        }
        match characters.next() {
            Some(next) if next == BACKSLASH || next == DOUBLE_QUOTE => result.push(next),
            Some(next) => {
                result.push(BACKSLASH);
                result.push(next);
            }
            None => result.push(BACKSLASH),
        }
    }
    result
}

impl Starts {
    /// Keeps the function `name` that `trigger` runs. An empty name changes
    /// nothing.
    pub(crate) fn add_function(&mut self, trigger: Trigger, name: &str) {
        if !name.is_empty() {
            self.functions.push((trigger, name.to_owned()));
        }
    }

    /// Keeps the widget `widget` that `add-zle-hook-widget` adds to the
    /// special widget `special`. An empty name changes nothing.
    pub(crate) fn add_hook_widget(&mut self, special: &str, widget: &str) {
        if !special.is_empty() && !widget.is_empty() {
            self.hook_widgets
                .entry(special.to_owned())
                .or_default()
                .push(widget.to_owned());
        }
    }

    /// Reads one line of `zle -lL`. `zle -N widget` makes the function of the
    /// same name the function of the widget, and `zle -N widget function`
    /// names the function. `zle -C widget completion-widget function` names
    /// the function after the completion widget. A line of a different form
    /// changes nothing.
    pub(crate) fn read_widget_line(&mut self, line: &str) {
        let Some(call) = one_call(line, ZLE_COMMAND) else {
            return;
        };
        let (widget, function) = match call.args.as_slice() {
            [option, widget] if option == NEW_WIDGET_OPTION => (widget, widget),
            [option, widget, function] if option == NEW_WIDGET_OPTION => (widget, function),
            [option, widget, _, function] if option == COMPLETION_WIDGET_OPTION => {
                (widget, function)
            }
            _ => return,
        };
        self.widgets.insert(widget.clone(), function.clone());
    }

    /// Reads one line of `bindkey -M keymap -L`. The last word is the widget
    /// and the word before it is the key, without its quotes. The words before
    /// them are options. A line with `-s` binds a key to a string, thus it
    /// starts no widget. A line of a different form changes nothing.
    pub(crate) fn read_key_line(&mut self, line: &str) {
        let Some(call) = one_call(line, BINDKEY_COMMAND) else {
            return;
        };
        let [options @ .., key, widget] = call.args.as_slice() else {
            return;
        };
        if options.iter().any(|option| option == STRING_BINDING_OPTION) {
            return;
        }
        self.keys
            .entry(widget.clone())
            .or_default()
            .insert(key.clone());
    }

    /// Keeps the code `text` that `trigger` runs. A text that holds only
    /// blanks changes nothing.
    pub(crate) fn add_code(&mut self, trigger: Trigger, text: &str) {
        if !text.trim().is_empty() {
            self.code.push((trigger, text.to_owned()));
        }
    }

    /// Reads the output of the bash `trap -p`: one `trap -- 'code' SIGNAL`
    /// command for each signal. The code of each signal is one text, and the
    /// name of the signal loses its `SIG`. A text that does not parse changes
    /// nothing.
    pub(crate) fn read_traps(&mut self, text: &str) {
        let Parsed::Words(words) = command_words(text, &Aliases::new()) else {
            return;
        };
        for word in words.into_iter().filter(|word| word.word == TRAP_COMMAND) {
            let args = match word.args.split_first() {
                Some((first, rest)) if first == END_OF_OPTIONS => rest,
                _ => word.args.as_slice(),
            };
            let Some((code, signals)) = args.split_first() else {
                continue;
            };
            for signal in signals {
                let name = signal.strip_prefix(SIGNAL_PREFIX).unwrap_or(signal);
                self.add_code(Trigger::Trap(name.to_owned()), code);
            }
        }
    }

    /// Reads one line of the bash `bind -X`: the key and the command, each in
    /// double quotes. bash 5.3 divides them with a space, and an older bash
    /// with a colon and a space. The key stays as bash writes it, and the
    /// command loses one level of backslashes. A line of a different form
    /// changes nothing.
    pub(crate) fn read_key_command_line(&mut self, line: &str) {
        let Some((key, rest)) = double_quoted(line) else {
            return;
        };
        let rest = rest.strip_prefix(KEY_END).unwrap_or(rest).trim_start();
        let Some((command, _)) = double_quoted(rest) else {
            return;
        };
        self.add_code(Trigger::KeyCommand(key.to_owned()), &unescape(command));
    }
}
