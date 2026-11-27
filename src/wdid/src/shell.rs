//! The aliases and the functions of the interactive shell.
//!
//! A person types an alias or a function name, and the shell runs a tool. The
//! history holds only the name that the person typed. Thus this module starts
//! the shell of the person, asks it for every alias and every function body,
//! and builds a [`Resolver`] that says which tools each function runs.
//!
//! [`script`] and [`parse_output`] are pure. [`read`] is the one function that
//! starts a process, and the one function that writes a file: the temporary
//! `.zshenv` that records the origin of each zsh alias.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use wait_timeout::ChildExt;

use crate::command_words::{command_words, AliasDefinition, Aliases, CommandWord, Parsed};
use crate::origin::{place, Origin, Places};
use crate::rules::{runs, Run};
use crate::starts::{
    Starts, Trigger, HOOK_ARRAY_SUFFIX, NOT_FOUND_HANDLER, SPECIAL_WIDGETS, TRAP_PREFIX, ZSH_HOOKS,
};
use crate::tool::{ToolName, ToolNames};

/// The file name of zsh.
const ZSH: &str = "zsh";

/// The file name of bash.
const BASH: &str = "bash";

/// The text that follows the marker on the line that starts the aliases.
const ALIASES_LABEL: &str = "ALIASES";

/// The text that follows the marker on the line that starts a function. The
/// name of the function follows this text.
const FUNCTION_LABEL: &str = "FUNCTION";

/// The text that follows the marker on the line that ends the output.
const END_LABEL: &str = "END";

/// The text that follows the marker on the line that gives the home directory
/// of the shell after its start. The path follows this text.
const HOME_LABEL: &str = "HOME";

/// The text that follows the marker on the line that gives the oh-my-zsh
/// directory (`$ZSH`). The path follows this text.
const ZSH_LABEL: &str = "ZSH";

/// The text that follows the marker on the line that gives the custom
/// directory of oh-my-zsh (`$ZSH_CUSTOM`). The path follows this text.
const ZSH_CUSTOM_LABEL: &str = "ZSH_CUSTOM";

/// The text that follows the marker on the line that starts the origin of a
/// regular or a global alias. The name of the alias follows this text.
const ALIAS_ORIGIN_LABEL: &str = "ALIAS_ORIGIN";

/// The text that follows the marker on the line that starts the origin of a
/// suffix alias. The suffix follows this text.
const SUFFIX_ORIGIN_LABEL: &str = "SUFFIX_ORIGIN";

/// The text that follows the marker on the line that gives the origin of the
/// alias or the function that the line before it names. The origin follows
/// this text: `file:line` for an alias, the file for a function, or nothing
/// when the shell keeps no origin.
const ORIGIN_LABEL: &str = "ORIGIN";

/// The text that follows the marker on the line that names a zsh hook and a
/// function that it runs. The hook, one space, and the function follow this
/// text.
const HOOK_LABEL: &str = "HOOK";

/// The text that follows the marker on the line that names a signal and its
/// zsh trap function. The signal, one space, and the function follow this
/// text.
const TRAP_LABEL: &str = "TRAP";

/// The text that follows the marker on the line that names the function that
/// zsh runs for a command that it does not find. The function follows this
/// text.
const NOT_FOUND_LABEL: &str = "NOT_FOUND";

/// The text that follows the marker on the line that starts the output of
/// `zle -lL`.
const WIDGETS_LABEL: &str = "WIDGETS";

/// The text that follows the marker on the line that starts the output of
/// `bindkey -L` for one keymap. The name of the keymap follows this text.
const KEYMAP_LABEL: &str = "KEYMAP";

/// The text that follows the marker on the line that names a special widget
/// and a widget that `add-zle-hook-widget` added to it. The special widget,
/// one space, and the widget follow this text.
const ZLE_HOOK_LABEL: &str = "ZLE_HOOK";

/// The text that follows the marker on the line that starts the output of
/// the bash `trap -p`.
const TRAPS_LABEL: &str = "TRAPS";

/// The text that follows the marker on the line that starts one element of
/// the bash `PROMPT_COMMAND`.
const PROMPT_COMMAND_LABEL: &str = "PROMPT_COMMAND";

/// The text that follows the marker on the line that starts the output of
/// the bash `bind -X`.
const KEY_COMMANDS_LABEL: &str = "KEY_COMMANDS";

/// The loop variable of the loop of the bash script over the elements of
/// `PROMPT_COMMAND`.
const CODE_VARIABLE: &str = "__wdid_code";

/// The loop variable of the outer loops of the zsh script that list the
/// starts: the hooks, the keymaps, and the special widgets.
const OUTER_VARIABLE: &str = "__wdid_outer";

/// The loop variable of the inner loops of the zsh script that list the
/// starts.
const NAME_VARIABLE: &str = "__wdid_name";

/// The variable of the zsh script that holds the name of the array of a hook.
const ARRAY_VARIABLE: &str = "__wdid_array";

/// The array of the zsh script that holds the widgets of a special widget.
const LIST_VARIABLE: &str = "__wdid_list";

/// The zsh table that the wrapper function `alias` fills: the `file:line` of
/// each regular and each global alias, by name.
pub(crate) const ALIAS_ORIGINS_TABLE: &str = "__wdid_alias_origins";

/// The zsh table that the wrapper function `alias` fills: the `file:line` of
/// each suffix alias, by suffix.
pub(crate) const SUFFIX_ORIGINS_TABLE: &str = "__wdid_suffix_origins";

/// The loop variable of the loops of the zsh script that print the origins of
/// the aliases.
const ORIGIN_LOOP_VARIABLE: &str = "__wdid_alias";

/// The variable of the bash script that holds the text of `declare -F`.
const BASH_ORIGIN_VARIABLE: &str = "__wdid_origin";

/// The variable that names the directory of the zsh startup files.
const ZDOTDIR_VARIABLE: &str = "ZDOTDIR";

/// The variable that carries the `ZDOTDIR` of the environment to the
/// temporary `.zshenv`. [`read`] sets it only when the environment sets
/// `ZDOTDIR`, and the temporary `.zshenv` removes it.
const REAL_ZDOTDIR_VARIABLE: &str = "__WDID_REAL_ZDOTDIR";

/// The name of the first startup file that zsh reads from `ZDOTDIR`.
const ZSHENV_FILE: &str = ".zshenv";

/// The start of the name of each temporary `ZDOTDIR`. The rest of the name is
/// random, thus it is unique for each run.
const ZDOTDIR_PREFIX: &str = "wdid-zdotdir-";

/// The name of the loop variable in the script. It is unlikely to clash with a
/// name that the person uses.
const LOOP_VARIABLE: &str = "__wdid_function";

/// The pattern of the autoloaded zsh functions that the script loads.
///
/// An autoloaded function is only a stub until the shell loads it, and the
/// stub holds no tool. `autoload +X -m` loads the body and runs no code. The
/// pattern skips each name that starts with `_`. The completion system
/// registers those functions, nobody types them, and their bodies do not
/// parse by the hundred.
const AUTOLOAD_PATTERN: &str = "[^_]*";

/// The pattern of the autoloaded zsh functions whose file the script finds
/// without a load: the names that [`AUTOLOAD_PATTERN`] skips.
///
/// zsh gives no file for a stub until it finds the file. `autoload -Ur -m`
/// finds the file of each stub, loads no body, and keeps each defined
/// function as it is.
const PRIVATE_AUTOLOAD_PATTERN: &str = "_*";

/// The command word of every alias line.
const ALIAS_COMMAND: &str = "alias";

/// The character that starts an option word of `alias`.
const OPTION_START: char = '-';

/// The option of `alias` that makes a global alias.
const GLOBAL_OPTION: char = 'g';

/// The option of `alias` that makes a suffix alias.
const SUFFIX_OPTION: char = 's';

/// The argument that ends the options of `alias`.
const END_OF_OPTIONS: &str = "--";

/// The time that the readers of the pipes get to read what is left in the
/// pipes, after the shell ends.
///
/// A process that the shell started in the background can keep a pipe open
/// after the shell ends, for example `( sleep 10 & )` in `.zshrc`. The output
/// is complete when the shell ends, so after this time wdid takes the bytes
/// that the readers have, and it does not wait for that process.
const PIPE_GRACE: Duration = Duration::from_secs(2);

/// The size of one read from a pipe.
const READ_CHUNK: usize = 64 * 1024;

/// The default bound on the shell start.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// The shells that wdid can ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// zsh.
    Zsh,
    /// bash.
    Bash,
}

impl ShellKind {
    /// Gives the kind from the file name of the shell path: `zsh` or `bash`.
    /// Gives `None` for any other name.
    #[must_use]
    pub fn of(shell: &Path) -> Option<Self> {
        match shell.file_name()?.to_str()? {
            ZSH => Some(Self::Zsh),
            BASH => Some(Self::Bash),
            _ => None,
        }
    }
}

/// The marker that wraps the output.
///
/// The interactive start of a shell can print text of its own. The marker lets
/// [`parse_output`] ignore that text. A marker is unique for each run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker(String);

impl Marker {
    /// Makes a marker that is unique for each run: the process id and the
    /// nanoseconds since the Unix epoch.
    #[must_use]
    pub fn unique() -> Self {
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        Self(format!("__WDID_{}_{nanoseconds}__", std::process::id()))
    }

    /// Makes a marker from a text. The tests use this function.
    ///
    /// # Panics
    /// Panics when the text is empty or holds a character other than an ASCII
    /// letter, an ASCII digit, or `_`. [`script`] puts the marker into quoted
    /// shell text, and only these characters need no quoting.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        assert!(
            !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "a marker holds only ASCII letters, digits, and underscores: {text:?}"
        );
        Self(text)
    }

    /// Gives the text of the marker.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Gives the text of the `-c` argument for the shell.
///
/// The zsh script lists the regular and the global aliases with `alias -L`,
/// and the suffix aliases with `alias -sL`, because `alias -L` does not list
/// them. Bash has no global or suffix alias. Before the listing, the zsh
/// script removes the wrapper function `alias` that [`read`] defines, so the
/// listing holds no such function and the builtin lists the aliases.
///
/// The zsh script loads the autoloaded functions first, so a function that
/// the shell did not load yet counts through its body. This runs no code. The
/// script skips the functions whose name starts with `_` (see
/// [`AUTOLOAD_PATTERN`]), and it finds only their files. Bash has no
/// autoload.
///
/// Each script gives the origin of each function, and the home directory. The
/// zsh script also gives the origin of each alias that the wrapper function
/// recorded, and the oh-my-zsh directories. Bash keeps no origin for an alias.
/// Each script also gives the code that the shell runs without a typed name.
/// The zsh script gives its hooks, its trap functions, its handler of a
/// command that it does not find, its widgets and their keys, and the widgets
/// of `add-zle-hook-widget` (see [`zsh_starts`]). The bash script gives the
/// code of each trap, each element of `PROMPT_COMMAND`, and each `bind -x`
/// key. A bash older than 4.3 has no `bind -X`, thus it gives no key.
///
/// The bash script turns off the `DEBUG` trap before `extdebug`, because
/// `extdebug` skips the next command when that trap fails. It lists the traps
/// before that.
#[must_use]
pub fn script(kind: ShellKind, marker: &Marker) -> String {
    let marker = marker.as_str();
    match kind {
        ShellKind::Zsh => format!(
            "print -r -- '{marker} {ALIASES_LABEL}'\n\
             if (( ${{+functions[{ALIAS_COMMAND}]}} )); then unfunction {ALIAS_COMMAND}; fi\n\
             zmodload zsh/parameter 2>/dev/null || :\n\
             alias -L\n\
             alias -sL\n\
             print -r -- \"{marker} {HOME_LABEL} ${{HOME-}}\"\n\
             print -r -- \"{marker} {ZSH_LABEL} ${{ZSH-}}\"\n\
             print -r -- \"{marker} {ZSH_CUSTOM_LABEL} ${{ZSH_CUSTOM-}}\"\n\
             {alias_origins}\
             {suffix_origins}\
             autoload +X -m '{AUTOLOAD_PATTERN}' 2>/dev/null || :\n\
             autoload -Ur -m '{PRIVATE_AUTOLOAD_PATTERN}' >/dev/null 2>&1 || :\n\
             {starts}\
             for {LOOP_VARIABLE} in ${{(ok)functions}}; do\n\
             \x20 print -r -- \"{marker} {FUNCTION_LABEL} ${LOOP_VARIABLE}\"\n\
             \x20 print -r -- \"{marker} {ORIGIN_LABEL} ${{functions_source[${LOOP_VARIABLE}]-}}\"\n\
             \x20 print -r -- \"${{functions[${LOOP_VARIABLE}]}}\"\n\
             done\n\
             print -r -- '{marker} {END_LABEL}'\n",
            alias_origins = zsh_origin_loop(marker, ALIAS_ORIGINS_TABLE, ALIAS_ORIGIN_LABEL),
            suffix_origins = zsh_origin_loop(marker, SUFFIX_ORIGINS_TABLE, SUFFIX_ORIGIN_LABEL),
            starts = zsh_starts(marker),
        ),
        ShellKind::Bash => format!(
            "printf '%s\\n' '{marker} {ALIASES_LABEL}'\n\
             alias -p\n\
             printf '%s\\n' \"{marker} {HOME_LABEL} ${{HOME-}}\"\n\
             printf '%s\\n' '{marker} {TRAPS_LABEL}'\n\
             trap -p\n\
             for {CODE_VARIABLE} in ${{PROMPT_COMMAND+\"${{PROMPT_COMMAND[@]}}\"}}; do\n\
             \x20 printf '%s\\n' '{marker} {PROMPT_COMMAND_LABEL}'\n\
             \x20 printf '%s\\n' \"${CODE_VARIABLE}\"\n\
             done\n\
             printf '%s\\n' '{marker} {KEY_COMMANDS_LABEL}'\n\
             bind -X 2>/dev/null || :\n\
             trap - DEBUG\n\
             shopt -s extdebug\n\
             for {LOOP_VARIABLE} in $(compgen -A function); do\n\
             \x20 printf '%s\\n' \"{marker} {FUNCTION_LABEL} ${LOOP_VARIABLE}\"\n\
             \x20 {BASH_ORIGIN_VARIABLE}=$(declare -F -- \"${LOOP_VARIABLE}\")\n\
             \x20 {BASH_ORIGIN_VARIABLE}=${{{BASH_ORIGIN_VARIABLE}#\"${LOOP_VARIABLE}\" }}\n\
             \x20 printf '%s\\n' \"{marker} {ORIGIN_LABEL} ${{{BASH_ORIGIN_VARIABLE}#* }}\"\n\
             \x20 declare -f -- \"${LOOP_VARIABLE}\" | tail -n +2\n\
             done\n\
             printf '%s\\n' '{marker} {END_LABEL}'\n"
        ),
    }
}

/// Gives the zsh lines that print the starts: a line for each hook function
/// and each name in a hook array, a line for each trap function, a line for
/// the handler of a command that zsh does not find, the output of `zle -lL`,
/// the output of `bindkey -L` for each keymap, and a line for each widget that
/// `add-zle-hook-widget` added to a special widget.
///
/// A hook array that is not set gives no line, also under `NO_UNSET`. A
/// command that can fail ends with `|| :`, so `ERR_EXIT` does not stop the
/// script.
fn zsh_starts(marker: &str) -> String {
    let hooks = ZSH_HOOKS.join(" ");
    let specials = SPECIAL_WIDGETS.join(" ");
    format!(
        "for {OUTER_VARIABLE} in {hooks}; do\n\
         \x20 if (( ${{+functions[${OUTER_VARIABLE}]}} )); then print -r -- \"{marker} {HOOK_LABEL} ${OUTER_VARIABLE} ${OUTER_VARIABLE}\"; fi\n\
         \x20 {ARRAY_VARIABLE}=${{{OUTER_VARIABLE}}}{HOOK_ARRAY_SUFFIX}\n\
         \x20 if (( ${{(P)+{ARRAY_VARIABLE}}} )); then\n\
         \x20   for {NAME_VARIABLE} in \"${{(@P){ARRAY_VARIABLE}}}\"; do print -r -- \"{marker} {HOOK_LABEL} ${OUTER_VARIABLE} ${NAME_VARIABLE}\"; done\n\
         \x20 fi\n\
         done\n\
         for {NAME_VARIABLE} in \"${{signals[@]}}\"; do\n\
         \x20 if (( ${{+functions[{TRAP_PREFIX}${NAME_VARIABLE}]}} )); then print -r -- \"{marker} {TRAP_LABEL} ${NAME_VARIABLE} {TRAP_PREFIX}${NAME_VARIABLE}\"; fi\n\
         done\n\
         if (( ${{+functions[{NOT_FOUND_HANDLER}]}} )); then print -r -- '{marker} {NOT_FOUND_LABEL} {NOT_FOUND_HANDLER}'; fi\n\
         print -r -- '{marker} {WIDGETS_LABEL}'\n\
         zle -lL 2>/dev/null || :\n\
         for {OUTER_VARIABLE} in ${{(f)\"$(bindkey -l 2>/dev/null)\"}}; do\n\
         \x20 print -r -- \"{marker} {KEYMAP_LABEL} ${OUTER_VARIABLE}\"\n\
         \x20 bindkey -M \"${OUTER_VARIABLE}\" -L 2>/dev/null || :\n\
         done\n\
         for {OUTER_VARIABLE} in {specials}; do\n\
         \x20 {LIST_VARIABLE}=()\n\
         \x20 zstyle -a \"${OUTER_VARIABLE}\" widgets {LIST_VARIABLE} 2>/dev/null || :\n\
         \x20 for {NAME_VARIABLE} in \"${{{LIST_VARIABLE}[@]}}\"; do print -r -- \"{marker} {ZLE_HOOK_LABEL} ${OUTER_VARIABLE} ${{{NAME_VARIABLE}#*:}}\"; done\n\
         done\n"
    )
}

/// Gives the zsh lines that print each entry of the origin table `table`: a
/// line with `label` and the name, then a line with the origin. The lines do
/// nothing when the table does not exist, for example in a shell that started
/// without the wrapper function `alias`.
fn zsh_origin_loop(marker: &str, table: &str, label: &str) -> String {
    format!(
        "if (( ${{+{table}}} )); then\n\
         \x20 for {ORIGIN_LOOP_VARIABLE} in ${{(ok){table}}}; do\n\
         \x20   print -r -- \"{marker} {label} ${ORIGIN_LOOP_VARIABLE}\"\n\
         \x20   print -r -- \"{marker} {ORIGIN_LABEL} ${{{table}[${ORIGIN_LOOP_VARIABLE}]}}\"\n\
         \x20 done\n\
         fi\n"
    )
}

/// The aliases and the functions that the shell defines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Definitions {
    /// The alias table.
    pub aliases: Aliases,
    /// The command words of the body of each function that parsed, by name.
    pub functions: BTreeMap<String, Vec<CommandWord>>,
    /// The first line of the body of each function, parsed or not, by name.
    /// It is the first line whose text is not blank and is not `{` alone,
    /// without the blanks at its start and at its end. It is empty when the
    /// body has no such line.
    pub function_lines: BTreeMap<String, String>,
    /// The body of each function, parsed or not, by name, as the shell gave
    /// it.
    pub function_bodies: BTreeMap<String, String>,
    /// The names of the functions whose body did not parse, sorted.
    pub unparsed_functions: Vec<String>,
    /// The alias lines that did not parse, in output order.
    pub unparsed_aliases: Vec<String>,
    /// The origin of each regular and each global alias, by name. An alias
    /// that this map does not hold has an unknown origin.
    pub alias_origins: BTreeMap<String, Origin>,
    /// The origin of each suffix alias, by its suffix (`md` for `*.md`).
    pub suffix_origins: BTreeMap<String, Origin>,
    /// The origin of each function, by name. A function that this map does
    /// not hold has an unknown origin.
    pub function_origins: BTreeMap<String, Origin>,
    /// Each place that defined each regular and each global alias, by name, in
    /// the order of the definitions. `None` stands for a place that is not an
    /// absolute path, for example `(eval):1`.
    pub alias_places: BTreeMap<String, Vec<Option<Origin>>>,
    /// Each place that defined each suffix alias, by its suffix (`md` for
    /// `*.md`), in the order of the definitions. `None` is as for
    /// [`Definitions::alias_places`].
    pub suffix_places: BTreeMap<String, Vec<Option<Origin>>>,
    /// The directories that the shell gave after its start.
    pub places: Places,
    /// The code that the shell runs without a typed name.
    pub starts: Starts,
}

impl Definitions {
    /// Gives the origin of `alias`, or `None` when it is unknown. A suffix
    /// alias has its origin by its suffix, and a regular or a global alias by
    /// its name.
    #[must_use]
    pub fn alias_origin(&self, alias: &AliasDefinition) -> Option<&Origin> {
        match alias.suffix() {
            Some(suffix) => self.suffix_origins.get(suffix),
            None => self.alias_origins.get(&alias.name),
        }
    }

    /// Gives each place that defined `alias`, in the order of the definitions.
    /// A suffix alias has its places by its suffix, and a regular or a global
    /// alias by its name. The slice is empty when the shell gave no place.
    #[must_use]
    pub fn alias_places(&self, alias: &AliasDefinition) -> &[Option<Origin>] {
        let places = match alias.suffix() {
            Some(suffix) => self.suffix_places.get(suffix),
            None => self.alias_places.get(&alias.name),
        };
        places.map_or(&[], Vec::as_slice)
    }

    /// Gives the origin of the function `name`, or `None` when it is unknown.
    #[must_use]
    pub fn function_origin(&self, name: &str) -> Option<&Origin> {
        self.function_origins.get(name)
    }
}

/// The reasons that the aliases and the functions cannot be read.
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    /// The variable `SHELL` does not exist.
    #[error("SHELL is not set, so wdid cannot read the aliases and functions")]
    NotSet,
    /// The shell is not zsh and not bash.
    #[error("wdid reads aliases and functions from zsh and bash only, and SHELL is {shell}")]
    NotSupported {
        /// The value of `SHELL`.
        shell: String,
    },
    /// The system cannot start the shell.
    #[error("cannot start {shell}: {source}")]
    Spawn {
        /// The shell program.
        shell: String,
        /// The error of the system.
        source: std::io::Error,
    },
    /// The shell ran for too long.
    #[error("the shell did not stop in {seconds} seconds")]
    TimedOut {
        /// The bound, in seconds.
        seconds: u64,
    },
    /// The shell ended with a non-zero status.
    #[error("the shell failed ({status}): {stderr}")]
    Failed {
        /// The text of the exit status.
        status: String,
        /// The standard error of the shell, trimmed.
        stderr: String,
    },
    /// The output has no start marker or no end marker.
    #[error("the shell output does not hold the start and end markers of wdid")]
    NoMarkers,
    /// wdid cannot make the temporary `ZDOTDIR` that records the origin of
    /// each alias.
    #[error("cannot make the temporary ZDOTDIR for zsh: {source}")]
    Zdotdir {
        /// The error of the system.
        source: std::io::Error,
    },
}

/// One line of the output that starts with the marker, without the marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Record<'a> {
    /// The end of the output.
    End,
    /// The start of a function, with its name.
    Function(&'a str),
    /// The start of the origin of a regular or a global alias, with its name.
    AliasOrigin(&'a str),
    /// The start of the origin of a suffix alias, with its suffix.
    SuffixOrigin(&'a str),
    /// The origin of the definition that the record before it started.
    Origin(&'a str),
    /// The home directory.
    Home(&'a str),
    /// The oh-my-zsh directory.
    Zsh(&'a str),
    /// The custom directory of oh-my-zsh.
    ZshCustom(&'a str),
    /// A zsh hook and a function that it runs, divided by one space.
    Hook(&'a str),
    /// A signal and its zsh trap function, divided by one space.
    Trap(&'a str),
    /// The function that zsh runs for a command that it does not find.
    NotFound(&'a str),
    /// The start of the output of `zle -lL`.
    Widgets,
    /// The start of the output of `bindkey -L` for one keymap.
    Keymap,
    /// A special widget and a widget that `add-zle-hook-widget` added to it,
    /// divided by one space.
    ZleHook(&'a str),
    /// The start of the output of the bash `trap -p`.
    Traps,
    /// The start of one element of the bash `PROMPT_COMMAND`.
    PromptCommand,
    /// The start of the output of the bash `bind -X`.
    KeyCommands,
}

/// Reads `line` as a record of `marker`. Gives `None` for a line that is not
/// a record: the marker, one space, a label, then one space and the value.
/// The end record and the start of each listing that holds lines have no
/// value.
fn record<'a>(line: &'a str, marker: &str) -> Option<Record<'a>> {
    let rest = line.strip_prefix(marker)?.strip_prefix(' ')?;
    let Some((label, value)) = rest.split_once(' ') else {
        return match rest {
            END_LABEL => Some(Record::End),
            WIDGETS_LABEL => Some(Record::Widgets),
            TRAPS_LABEL => Some(Record::Traps),
            PROMPT_COMMAND_LABEL => Some(Record::PromptCommand),
            KEY_COMMANDS_LABEL => Some(Record::KeyCommands),
            _ => None,
        };
    };
    match label {
        HOOK_LABEL => Some(Record::Hook(value)),
        TRAP_LABEL => Some(Record::Trap(value)),
        NOT_FOUND_LABEL => Some(Record::NotFound(value)),
        KEYMAP_LABEL => Some(Record::Keymap),
        ZLE_HOOK_LABEL => Some(Record::ZleHook(value)),
        FUNCTION_LABEL => Some(Record::Function(value)),
        ALIAS_ORIGIN_LABEL => Some(Record::AliasOrigin(value)),
        SUFFIX_ORIGIN_LABEL => Some(Record::SuffixOrigin(value)),
        ORIGIN_LABEL => Some(Record::Origin(value)),
        HOME_LABEL => Some(Record::Home(value)),
        ZSH_LABEL => Some(Record::Zsh(value)),
        ZSH_CUSTOM_LABEL => Some(Record::ZshCustom(value)),
        _ => None,
    }
}

/// The part of the output that the lines after the last start record belong
/// to.
enum Section<'a> {
    /// The alias listing, before the first start record.
    Aliases,
    /// The origin of an alias. `suffix` is true for a suffix alias.
    AliasOrigin {
        /// The name of a regular or a global alias, or the suffix of a suffix
        /// alias.
        name: &'a str,
        /// True for a suffix alias.
        suffix: bool,
    },
    /// The output of `zle -lL`: one `zle` command for each widget.
    Widgets,
    /// The output of `bindkey -L` for one keymap: one `bindkey` command for
    /// each key.
    Keys,
    /// The output of `trap -p` and its lines so far. The code of one trap can
    /// hold more than one line.
    Traps {
        /// The lines so far.
        lines: Vec<&'a str>,
    },
    /// One element of `PROMPT_COMMAND` and its lines so far.
    PromptCommand {
        /// The lines so far.
        lines: Vec<&'a str>,
    },
    /// The output of `bind -X`: the key and the command of each `bind -x`
    /// binding.
    KeyCommands,
    /// A function and the lines of its body so far.
    Function {
        /// The name of the function.
        name: &'a str,
        /// The lines of the body so far.
        body: Vec<&'a str>,
    },
}

impl<'a> Section<'a> {
    /// Adds a line that is not a record. The alias listing reads it as an
    /// alias line, and a function adds it to its body. A listing of the
    /// widgets, the keys, or the key commands reads it as one of them. The
    /// traps and an element of `PROMPT_COMMAND` add it to their lines. The
    /// origin of an alias has no more lines, thus it ignores the line.
    fn push(&mut self, line: &'a str, definitions: &mut Definitions) {
        match self {
            Self::Aliases => read_alias_line(line, definitions),
            Self::AliasOrigin { .. } => {}
            Self::Widgets => definitions.starts.read_widget_line(line),
            Self::Keys => definitions.starts.read_key_line(line),
            Self::KeyCommands => definitions.starts.read_key_command_line(line),
            Self::Traps { lines } | Self::PromptCommand { lines } => lines.push(line),
            Self::Function { body, .. } => body.push(line),
        }
    }

    /// Keeps the origin `text` for the definition of this section. An origin
    /// that is not known, and an origin in the alias listing, change nothing.
    fn set_origin(&self, text: &str, definitions: &mut Definitions) {
        match self {
            Self::Aliases
            | Self::Widgets
            | Self::Keys
            | Self::Traps { .. }
            | Self::PromptCommand { .. }
            | Self::KeyCommands => {}
            Self::AliasOrigin { name, suffix } => {
                if let Some(origin) = Origin::from_trace(text) {
                    let table = if *suffix {
                        &mut definitions.suffix_origins
                    } else {
                        &mut definitions.alias_origins
                    };
                    table.insert((*name).to_owned(), origin);
                }
            }
            Self::Function { name, .. } => {
                if let Some(origin) = Origin::from_file(text) {
                    definitions
                        .function_origins
                        .insert((*name).to_owned(), origin);
                }
            }
        }
    }

    /// Ends the section. A function reads its body. The traps and an element
    /// of `PROMPT_COMMAND` read their lines as one text.
    fn finish(self, definitions: &mut Definitions) {
        match self {
            Self::Function { name, body } => read_function(name, &body.join("\n"), definitions),
            Self::Traps { lines } => definitions.starts.read_traps(&lines.join("\n")),
            Self::PromptCommand { lines } => definitions
                .starts
                .add_code(Trigger::PromptCommand, &lines.join("\n")),
            Self::Aliases
            | Self::AliasOrigin { .. }
            | Self::Widgets
            | Self::Keys
            | Self::KeyCommands => {}
        }
    }
}

/// Parses the standard output of the script. Pure.
///
/// # Errors
/// Gives [`ShellError::NoMarkers`] when the output has no start marker line, or
/// no end marker line after it.
pub fn parse_output(stdout: &str, marker: &Marker) -> Result<Definitions, ShellError> {
    let marker = marker.as_str();
    let aliases_line = format!("{marker} {ALIASES_LABEL}");
    let mut lines = stdout.lines().skip_while(|line| *line != aliases_line);
    lines.next().ok_or(ShellError::NoMarkers)?;
    let mut definitions = Definitions::default();
    let mut section = Section::Aliases;
    let mut ended = false;
    for line in lines {
        let Some(found) = record(line, marker) else {
            section.push(line, &mut definitions);
            continue;
        };
        let next = match found {
            Record::End => {
                ended = true;
                break;
            }
            Record::Function(name) => Section::Function {
                name,
                body: Vec::new(),
            },
            Record::AliasOrigin(name) => Section::AliasOrigin {
                name,
                suffix: false,
            },
            Record::SuffixOrigin(name) => Section::AliasOrigin { name, suffix: true },
            Record::Origin(text) => {
                section.set_origin(text, &mut definitions);
                continue;
            }
            Record::Home(text) => {
                definitions.places.home = place(text);
                continue;
            }
            Record::Zsh(text) => {
                definitions.places.zsh = place(text);
                continue;
            }
            Record::ZshCustom(text) => {
                definitions.places.zsh_custom = place(text);
                continue;
            }
            Record::Hook(text) => {
                if let Some((hook, function)) = text.split_once(' ') {
                    let trigger = Trigger::Hook(hook.to_owned());
                    definitions.starts.add_function(trigger, function);
                }
                continue;
            }
            Record::Trap(text) => {
                if let Some((signal, function)) = text.split_once(' ') {
                    let trigger = Trigger::Trap(signal.to_owned());
                    definitions.starts.add_function(trigger, function);
                }
                continue;
            }
            Record::NotFound(function) => {
                definitions
                    .starts
                    .add_function(Trigger::CommandNotFound, function);
                continue;
            }
            Record::ZleHook(text) => {
                if let Some((special, widget)) = text.split_once(' ') {
                    definitions.starts.add_hook_widget(special, widget);
                }
                continue;
            }
            Record::Widgets => Section::Widgets,
            Record::Keymap => Section::Keys,
            Record::Traps => Section::Traps { lines: Vec::new() },
            Record::PromptCommand => Section::PromptCommand { lines: Vec::new() },
            Record::KeyCommands => Section::KeyCommands,
        };
        std::mem::replace(&mut section, next).finish(&mut definitions);
    }
    if !ended {
        return Err(ShellError::NoMarkers);
    }
    section.finish(&mut definitions);
    definitions.unparsed_functions.sort();
    Ok(definitions)
}

/// Gives the first line of a function body whose text is not blank and is not
/// `{` alone, without the blanks at its start and at its end. Gives an empty
/// text when the body has no such line. A bash body starts with a line `{`.
fn first_line(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && *line != "{")
        .unwrap_or_default()
        .to_owned()
}

/// Reads the body of one function into `definitions`. It keeps the body and
/// its first line for each function, also for a body that does not parse.
///
/// The shell expanded the aliases of the body when it read the definition,
/// the global and the suffix aliases included. Thus the parse of the body uses
/// an empty alias table.
fn read_function(name: &str, body: &str, definitions: &mut Definitions) {
    definitions
        .function_lines
        .insert(name.to_owned(), first_line(body));
    definitions
        .function_bodies
        .insert(name.to_owned(), body.to_owned());
    match command_words(body, &Aliases::new()) {
        Parsed::Words(words) => {
            definitions.functions.insert(name.to_owned(), words);
        }
        Parsed::Unparsed => definitions.unparsed_functions.push(name.to_owned()),
    }
}

/// Reads one line of the alias listing into `definitions`.
///
/// A line that is not exactly one `alias` command goes to
/// `unparsed_aliases`. An option word that holds `g` makes the aliases of the
/// line global. An option word that holds `s` makes them suffix aliases, and
/// the name before `=` is then the suffix. A line with both options goes to
/// `unparsed_aliases`, because one alias cannot be of both kinds.
fn read_alias_line(line: &str, definitions: &mut Definitions) {
    if line.trim().is_empty() {
        return;
    }
    let args = match command_words(line, &Aliases::new()) {
        Parsed::Words(words) => match <[CommandWord; 1]>::try_from(words) {
            Ok([word]) if word.word == ALIAS_COMMAND => word.args,
            _ => Vec::new(),
        },
        Parsed::Unparsed => Vec::new(),
    };
    if args.is_empty() {
        definitions.unparsed_aliases.push(line.to_owned());
        return;
    }
    let mut definitions_in_line = Vec::new();
    let mut options_ended = false;
    let mut global = false;
    let mut suffix = false;
    for arg in &args {
        if options_ended || arg.contains('=') {
            if let Some((name, value)) = arg.split_once('=') {
                definitions_in_line.push((name, value));
            }
        } else if arg == END_OF_OPTIONS {
            options_ended = true;
        } else if let Some(options) = arg.strip_prefix(OPTION_START) {
            global |= options.contains(GLOBAL_OPTION);
            suffix |= options.contains(SUFFIX_OPTION);
        }
    }
    if global && suffix {
        definitions.unparsed_aliases.push(line.to_owned());
        return;
    }
    let aliases = &mut definitions.aliases;
    for (name, value) in definitions_in_line {
        if global {
            aliases.insert_global(name, value);
        } else if suffix {
            aliases.insert_suffix(name, value);
        } else {
            aliases.insert(name, value);
        }
    }
}

/// Gives the text of the temporary `.zshenv` of [`read`].
///
/// zsh keeps no origin for an alias. Thus the file defines a function named
/// `alias`, before each startup file of the person runs. For each `name=value`
/// argument, the function records `${funcfiletrace[1]}` (`file:line`, the
/// place of the call) by the name, and then it runs `builtin alias "$@"`. An
/// argument after the options and `--` is a definition, and the option `s`
/// makes a suffix alias, which zsh keeps in a namespace of its own.
///
/// Then the file puts `ZDOTDIR` back: the value of the environment, or no
/// value when the environment has none. It sources the real `.zshenv` from
/// `${ZDOTDIR-$HOME}`, the directory where zsh looks for it. zsh then reads
/// the other startup files from the real directory, also when the real
/// `.zshenv` changes `ZDOTDIR`.
fn zshenv() -> String {
    format!(
        "typeset -gA {ALIAS_ORIGINS_TABLE} {SUFFIX_ORIGINS_TABLE}\n\
         function {ALIAS_COMMAND} {{\n\
         \x20 emulate -L zsh\n\
         \x20 local __wdid_word\n\
         \x20 local -i __wdid_suffix=0 __wdid_options=1\n\
         \x20 for __wdid_word in \"$@\"; do\n\
         \x20   if (( __wdid_options )); then\n\
         \x20     case $__wdid_word in\n\
         \x20       (--|-) __wdid_options=0; continue ;;\n\
         \x20       ([-+]*) [[ $__wdid_word == *{SUFFIX_OPTION}* ]] && __wdid_suffix=1; continue ;;\n\
         \x20       (*) __wdid_options=0 ;;\n\
         \x20     esac\n\
         \x20   fi\n\
         \x20   [[ $__wdid_word == *=* ]] || continue\n\
         \x20   if (( __wdid_suffix )); then\n\
         \x20     {SUFFIX_ORIGINS_TABLE}[${{__wdid_word%%=*}}]=${{funcfiletrace[1]}}\n\
         \x20   else\n\
         \x20     {ALIAS_ORIGINS_TABLE}[${{__wdid_word%%=*}}]=${{funcfiletrace[1]}}\n\
         \x20   fi\n\
         \x20 done\n\
         \x20 builtin {ALIAS_COMMAND} \"$@\"\n\
         }}\n\
         if (( ${{+{REAL_ZDOTDIR_VARIABLE}}} )); then\n\
         \x20 export {ZDOTDIR_VARIABLE}=${REAL_ZDOTDIR_VARIABLE}\n\
         \x20 unset {REAL_ZDOTDIR_VARIABLE}\n\
         else\n\
         \x20 unset {ZDOTDIR_VARIABLE}\n\
         fi\n\
         if [[ -r ${{{ZDOTDIR_VARIABLE}-$HOME}}/{ZSHENV_FILE} ]]; then\n\
         \x20 source ${{{ZDOTDIR_VARIABLE}-$HOME}}/{ZSHENV_FILE}\n\
         fi\n"
    )
}

/// Gives the value of the variable `name` in the environment of the child
/// that `command` starts: the value that `command` sets or removes, else the
/// value of this process.
fn child_variable(command: &Command, name: &str) -> Option<OsString> {
    match command.get_envs().find(|(key, _)| *key == OsStr::new(name)) {
        Some((_, value)) => value.map(OsStr::to_os_string),
        None => std::env::var_os(name),
    }
}

/// Points zsh at a new temporary `ZDOTDIR` that holds the file of
/// [`zshenv`], and gives that directory. The directory removes itself when
/// the caller drops it.
///
/// # Errors
/// Gives [`ShellError::Zdotdir`] when the directory or its file cannot be
/// made.
fn wrap_zsh(command: &mut Command) -> Result<tempfile::TempDir, ShellError> {
    let zdotdir_error = |source| ShellError::Zdotdir { source };
    let directory = tempfile::Builder::new()
        .prefix(ZDOTDIR_PREFIX)
        .tempdir()
        .map_err(zdotdir_error)?;
    std::fs::write(directory.path().join(ZSHENV_FILE), zshenv()).map_err(zdotdir_error)?;
    match child_variable(command, ZDOTDIR_VARIABLE) {
        Some(real) => command.env(REAL_ZDOTDIR_VARIABLE, real),
        None => command.env_remove(REAL_ZDOTDIR_VARIABLE),
    };
    command.env(ZDOTDIR_VARIABLE, directory.path());
    Ok(directory)
}

/// Runs `command -i -c <script>` and parses the output.
///
/// The caller makes the `Command`, thus a test can set `HOME` and `ZDOTDIR`.
///
/// For zsh, `read` first makes a temporary `ZDOTDIR` with a unique name, and
/// its `.zshenv` records the origin of each alias (see [`zshenv`]). The
/// directory is gone when `read` returns, after an error and after a timeout
/// too.
///
/// # Errors
/// Gives [`ShellError::Zdotdir`] when the temporary `ZDOTDIR` cannot be made,
/// [`ShellError::Spawn`] when the shell does not start,
/// [`ShellError::TimedOut`] when it runs longer than `timeout`,
/// [`ShellError::Failed`] for a non-zero status, and [`ShellError::NoMarkers`]
/// from [`parse_output`].
pub fn read(
    mut command: Command,
    kind: ShellKind,
    timeout: Duration,
) -> Result<Definitions, ShellError> {
    let _zdotdir = match kind {
        ShellKind::Zsh => Some(wrap_zsh(&mut command)?),
        ShellKind::Bash => None,
    };
    let shell = command.get_program().to_string_lossy().into_owned();
    let marker = Marker::unique();
    let mut child = command
        .args(["-i", "-c"])
        .arg(script(kind, &marker))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| ShellError::Spawn {
            shell: shell.clone(),
            source,
        })?;
    let stdout = collect(child.stdout.take());
    let stderr = collect(child.stderr.take());
    let timed_out = ShellError::TimedOut {
        seconds: timeout.as_secs(),
    };
    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(timed_out);
        }
        Err(source) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ShellError::Spawn { shell, source });
        }
    };
    let deadline = Instant::now() + PIPE_GRACE;
    let stdout = stdout.take(deadline);
    let stderr = stderr.take(deadline);
    if !status.success() {
        return Err(ShellError::Failed {
            status: status.to_string(),
            stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
        });
    }
    parse_output(&String::from_utf8_lossy(&stdout), &marker)
}

/// The bytes that a reader thread got from one pipe, and the signal that the
/// pipe closed.
struct Collected {
    /// The bytes so far. The reader adds each chunk as it arrives, so the
    /// bytes stay available when the pipe never closes.
    bytes: Arc<Mutex<Vec<u8>>>,
    /// Gets a message when the pipe closes.
    closed: mpsc::Receiver<()>,
}

impl Collected {
    /// Waits until the pipe closes or until `deadline`, then gives the bytes
    /// that the reader got.
    fn take(self, deadline: Instant) -> Vec<u8> {
        let _ = self
            .closed
            .recv_timeout(deadline.saturating_duration_since(Instant::now()));
        let mut bytes = self.bytes.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *bytes)
    }
}

/// Reads a pipe to its end on its own thread. The output of the shell is
/// larger than one pipe buffer, thus a read after the wait would block the
/// shell forever.
fn collect<R: Read + Send + 'static>(pipe: Option<R>) -> Collected {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let (sender, closed) = mpsc::channel();
    let shared = Arc::clone(&bytes);
    thread::spawn(move || {
        if let Some(mut pipe) = pipe {
            let mut chunk = vec![0_u8; READ_CHUNK];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => shared
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .extend_from_slice(&chunk[..count]),
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        }
        let _ = sender.send(());
    });
    Collected { bytes, closed }
}

/// The tools that each function resolves to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolver {
    tools: BTreeMap<String, BTreeSet<ToolName>>,
}

impl Resolver {
    /// Resolves every function of `definitions` to the tools that it uses.
    #[must_use]
    pub fn new(definitions: &Definitions, tools: &ToolNames) -> Self {
        let tools_by_function = definitions
            .functions
            .keys()
            .map(|name| (name.clone(), reachable_tools(name, definitions, tools)))
            .collect();
        Self {
            tools: tools_by_function,
        }
    }

    /// Gives the tools of the function `name`. Gives `None` when `name` is not
    /// a function. Gives `Some` of a possibly empty set when it is.
    #[must_use]
    pub fn tools_of(&self, name: &str) -> Option<&BTreeSet<ToolName>> {
        self.tools.get(name)
    }
}

/// Gives every tool that the function `start` uses, to any depth.
///
/// The walk visits each function once, thus a cycle of functions ends. A word
/// can be a tool and a function, for example the function `crap` that runs
/// `command crap`. Such a word adds the tool, and the walk enters the function
/// too. A development run does not count as a use.
fn reachable_tools(
    start: &str,
    definitions: &Definitions,
    tools: &ToolNames,
) -> BTreeSet<ToolName> {
    let mut found = BTreeSet::new();
    let mut visited: BTreeSet<&str> = BTreeSet::new();
    let mut pending = vec![start];
    while let Some(name) = pending.pop() {
        let Some((name, words)) = definitions.functions.get_key_value(name) else {
            continue;
        };
        if !visited.insert(name.as_str()) {
            continue;
        }
        for word in words {
            for run in runs(word, tools) {
                if let Run::Use(tool) = run {
                    found.insert(tool);
                }
            }
            pending.push(word.word.as_str());
        }
    }
    found
}
