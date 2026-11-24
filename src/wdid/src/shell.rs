//! The aliases and the functions of the interactive shell.
//!
//! A person types an alias or a function name, and the shell runs a tool. The
//! history holds only the name that the person typed. Thus this module starts
//! the shell of the person, asks it for every alias and every function body,
//! and builds a [`Resolver`] that says which tools each function runs.
//!
//! [`script`] and [`parse_output`] are pure. [`read`] is the one function that
//! starts a process.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use wait_timeout::ChildExt;

use crate::command_words::{command_words, Aliases, CommandWord, Parsed};
use crate::rules::{runs, Run};
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
/// them. Bash has no global or suffix alias.
///
/// The zsh script loads the autoloaded functions first, so a function that
/// the shell did not load yet counts through its body. This runs no code. The
/// script skips the functions whose name starts with `_` (see
/// [`AUTOLOAD_PATTERN`]). Bash has no autoload, so its script does not change.
#[must_use]
pub fn script(kind: ShellKind, marker: &Marker) -> String {
    let marker = marker.as_str();
    match kind {
        ShellKind::Zsh => format!(
            "print -r -- '{marker} {ALIASES_LABEL}'\n\
             alias -L\n\
             alias -sL\n\
             autoload +X -m '{AUTOLOAD_PATTERN}' 2>/dev/null || :\n\
             for {LOOP_VARIABLE} in ${{(ok)functions}}; do\n\
             \x20 print -r -- \"{marker} {FUNCTION_LABEL} ${LOOP_VARIABLE}\"\n\
             \x20 print -r -- \"${{functions[${LOOP_VARIABLE}]}}\"\n\
             done\n\
             print -r -- '{marker} {END_LABEL}'\n"
        ),
        ShellKind::Bash => format!(
            "printf '%s\\n' '{marker} {ALIASES_LABEL}'\n\
             alias -p\n\
             for {LOOP_VARIABLE} in $(compgen -A function); do\n\
             \x20 printf '%s\\n' \"{marker} {FUNCTION_LABEL} ${LOOP_VARIABLE}\"\n\
             \x20 declare -f -- \"${LOOP_VARIABLE}\" | tail -n +2\n\
             done\n\
             printf '%s\\n' '{marker} {END_LABEL}'\n"
        ),
    }
}

/// The aliases and the functions that the shell defines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Definitions {
    /// The alias table.
    pub aliases: Aliases,
    /// The command words of the body of each function that parsed, by name.
    pub functions: BTreeMap<String, Vec<CommandWord>>,
    /// The names of the functions whose body did not parse, sorted.
    pub unparsed_functions: Vec<String>,
    /// The alias lines that did not parse, in output order.
    pub unparsed_aliases: Vec<String>,
}

/// The reasons that the aliases and the functions cannot be read.
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    /// The variable `SHELL` does not exist.
    #[error("SHELL is not set, so wdid cannot read the aliases and functions (wdid --no-shell skips the aliases and functions)")]
    NotSet,
    /// The shell is not zsh and not bash.
    #[error("wdid reads aliases and functions from zsh and bash only, and SHELL is {shell} (wdid --no-shell skips the aliases and functions)")]
    NotSupported {
        /// The value of `SHELL`.
        shell: String,
    },
    /// The system cannot start the shell.
    #[error("cannot start {shell}: {source} (wdid --no-shell skips the aliases and functions)")]
    Spawn {
        /// The shell program.
        shell: String,
        /// The error of the system.
        source: std::io::Error,
    },
    /// The shell ran for too long.
    #[error("the shell did not stop in {seconds} seconds (wdid --no-shell skips the aliases and functions)")]
    TimedOut {
        /// The bound, in seconds.
        seconds: u64,
    },
    /// The shell ended with a non-zero status.
    #[error(
        "the shell failed ({status}): {stderr} (wdid --no-shell skips the aliases and functions)"
    )]
    Failed {
        /// The text of the exit status.
        status: String,
        /// The standard error of the shell, trimmed.
        stderr: String,
    },
    /// The output has no start marker or no end marker.
    #[error("the shell output does not hold the start and end markers of wdid (wdid --no-shell skips the aliases and functions)")]
    NoMarkers,
}

/// Parses the standard output of the script. Pure.
///
/// # Errors
/// Gives [`ShellError::NoMarkers`] when the output has no start marker line, or
/// no end marker line after it.
pub fn parse_output(stdout: &str, marker: &Marker) -> Result<Definitions, ShellError> {
    let marker = marker.as_str();
    let aliases_line = format!("{marker} {ALIASES_LABEL}");
    let end_line = format!("{marker} {END_LABEL}");
    let function_prefix = format!("{marker} {FUNCTION_LABEL} ");
    let mut lines = stdout.lines().skip_while(|line| *line != aliases_line);
    lines.next().ok_or(ShellError::NoMarkers)?;
    let mut definitions = Definitions::default();
    let mut ended = false;
    let mut function: Option<(&str, Vec<&str>)> = None;
    for line in lines {
        let starts_function = line.strip_prefix(function_prefix.as_str());
        if starts_function.is_some() || line == end_line {
            if let Some((name, body)) = function.take() {
                read_function(name, &body.join("\n"), &mut definitions);
            }
        }
        if line == end_line {
            ended = true;
            break;
        }
        match (starts_function, function.as_mut()) {
            (Some(name), _) => function = Some((name, Vec::new())),
            (None, Some((_, body))) => body.push(line),
            (None, None) => read_alias_line(line, &mut definitions),
        }
    }
    if ended {
        definitions.unparsed_functions.sort();
        Ok(definitions)
    } else {
        Err(ShellError::NoMarkers)
    }
}

/// Reads the body of one function into `definitions`.
///
/// The shell expanded the aliases of the body when it read the definition,
/// the global and the suffix aliases included. Thus the parse of the body uses
/// an empty alias table.
fn read_function(name: &str, body: &str, definitions: &mut Definitions) {
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

/// Runs `command -i -c <script>` and parses the output.
///
/// The caller makes the `Command`, thus a test can set `HOME` and `ZDOTDIR`.
///
/// # Errors
/// Gives [`ShellError::Spawn`] when the shell does not start,
/// [`ShellError::TimedOut`] when it runs longer than `timeout`,
/// [`ShellError::Failed`] for a non-zero status, and [`ShellError::NoMarkers`]
/// from [`parse_output`].
pub fn read(
    mut command: Command,
    kind: ShellKind,
    timeout: Duration,
) -> Result<Definitions, ShellError> {
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
