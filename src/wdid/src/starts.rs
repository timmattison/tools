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

/// The byte that starts each escape in the command that `bind -X` writes.
const ESCAPE_START: u8 = b'\\';

/// The byte of the quote in the command that `bind -X` writes.
const QUOTE_BYTE: u8 = b'"';

/// The text after [`ESCAPE_START`] that gives a byte with its high bit:
/// `\M-x`.
const META_PREFIX: &[u8] = b"M-";

/// The high bit, which `\M-` adds to the byte after it.
const META_BIT: u8 = 0x80;

/// The text after [`ESCAPE_START`] that gives a control character: `\C-x`.
const CONTROL_PREFIX: &[u8] = b"C-";

/// The bits of a byte that its control character keeps.
const CONTROL_MASK: u8 = 0x1f;

/// The character after `\C-` that gives DEL.
const DELETE_LETTER: u8 = b'?';

/// The byte DEL.
const DELETE: u8 = 0x7f;

/// The character after [`ESCAPE_START`] that gives the escape character:
/// `\e`.
const ESCAPE_LETTER: u8 = b'e';

/// The escape character.
const ESCAPE: u8 = 0x1b;

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

/// Gives back the command `text` that `bind -X` wrote. readline writes each
/// byte of the command in one of these forms, and the decode reverses each
/// form:
///
/// - `\\` and `\"`: a backslash and a quote.
/// - `\e`: the escape character.
/// - `\C-?`: DEL, and `\C-x`: the control character of `x`, for example
///   `\C-j` for a newline.
/// - `\M-` before the form of the byte without its high bit, for example
///   `\M-C\M-)` for the two bytes of `é`.
///
/// Each other byte stays, and so does a backslash before a different
/// character. The bytes give UTF-8 text, and a byte that is not UTF-8 gives
/// U+FFFD.
fn decode_key_command(text: &str) -> String {
    let mut rest = text.as_bytes();
    let mut bytes = Vec::with_capacity(rest.len());
    while let Some((byte, after)) = decode_byte(rest) {
        bytes.push(byte);
        rest = after;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Gives the first byte that the form at the start of `input` stands for, and
/// the input after that form. See [`decode_key_command`] for the forms. Gives
/// `None` when `input` is empty.
fn decode_byte(input: &[u8]) -> Option<(u8, &[u8])> {
    let (&first, rest) = input.split_first()?;
    if first != ESCAPE_START {
        return Some((first, rest));
    }
    if let Some((byte, after)) = rest.strip_prefix(META_PREFIX).and_then(decode_byte) {
        return Some((byte | META_BIT, after));
    }
    if let Some((&letter, after)) = rest
        .strip_prefix(CONTROL_PREFIX)
        .and_then(<[u8]>::split_first)
    {
        let byte = if letter == DELETE_LETTER {
            DELETE
        } else {
            letter & CONTROL_MASK
        };
        return Some((byte, after));
    }
    match rest.split_first() {
        Some((&ESCAPE_LETTER, after)) => Some((ESCAPE, after)),
        Some((&next, after)) if next == ESCAPE_START || next == QUOTE_BYTE => Some((next, after)),
        _ => Some((first, rest)),
    }
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
    /// command gets back each byte that readline wrote as an escape (see
    /// [`decode_key_command`]). A line of a different form changes nothing.
    pub(crate) fn read_key_command_line(&mut self, line: &str) {
        let Some((key, rest)) = double_quoted(line) else {
            return;
        };
        let rest = rest.strip_prefix(KEY_END).unwrap_or(rest).trim_start();
        let Some((command, _)) = double_quoted(rest) else {
            return;
        };
        self.add_code(Trigger::KeyCommand(key.to_owned()), &decode_key_command(command));
    }
}
