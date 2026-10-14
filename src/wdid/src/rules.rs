//! The count rules for one command word.
//!
//! The function rule is not here. The tally applies that one, because it needs
//! the shell definitions.

use crate::command_words::CommandWord;
use crate::tool::{ToolName, ToolNames};

/// The separator of the components of a path.
const PATH_SEPARATOR: char = '/';

/// The directory that cargo builds into. A path with this component is a
/// development build.
const TARGET_DIRECTORY: &str = "target";

/// The directory that holds installed binaries. A path whose parent has this
/// name is a use of an installed tool.
const BIN_DIRECTORY: &str = "bin";

/// The command word of cargo.
const CARGO: &str = "cargo";

/// The subcommands of cargo that run a binary.
const RUN_SUBCOMMANDS: [&str; 2] = ["run", "r"];

/// The word that ends the options of cargo. Later words go to the program.
const END_OF_OPTIONS: &str = "--";

/// The option that names a binary target.
const BIN_OPTION: &str = "--bin";

/// The long option that names a package.
const PACKAGE_OPTION: &str = "--package";

/// The short option that names a package.
const PACKAGE_SHORT_OPTION: &str = "-p";

/// The character that joins a long option to its value.
const VALUE_SEPARATOR: char = '=';

/// The first character of an option.
const OPTION_PREFIX: char = '-';

/// The first character of a toolchain word, for example `+nightly`.
const TOOLCHAIN_PREFIX: char = '+';

/// One run that a command word makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    /// A use of the tool.
    Use(ToolName),
    /// A run of a development build of the tool.
    Development(ToolName),
}

/// Gives the runs that one command word makes. Pure.
///
/// The result has at most one entry for each tool.
#[must_use]
pub fn runs(word: &CommandWord, tools: &ToolNames) -> Vec<Run> {
    if let Some(tool) = tools.get(&word.word) {
        return vec![Run::Use(tool.clone())];
    }
    if let Some((directory, name)) = word.word.rsplit_once(PATH_SEPARATOR) {
        return path_run(directory, name, tools).into_iter().collect();
    }
    if word.word == CARGO {
        return cargo_runs(&word.args, tools);
    }
    Vec::new()
}

/// Gives the development runs of a `cargo run` command. `args` are the
/// arguments of cargo. Any other subcommand gives no run.
///
/// The values of `--bin` win over the values of the package options. The
/// result holds each tool once, in the order of its first appearance.
fn cargo_runs(args: &[String], tools: &ToolNames) -> Vec<Run> {
    let mut rest = args
        .iter()
        .skip_while(|arg| arg.starts_with([OPTION_PREFIX, TOOLCHAIN_PREFIX]));
    match rest.next() {
        Some(subcommand) if RUN_SUBCOMMANDS.contains(&subcommand.as_str()) => {}
        _ => return Vec::new(),
    }
    let mut binaries: Vec<&str> = Vec::new();
    let mut packages: Vec<&str> = Vec::new();
    let mut words = rest
        .map(String::as_str)
        .take_while(|arg| *arg != END_OF_OPTIONS);
    while let Some(arg) = words.next() {
        if arg == BIN_OPTION {
            binaries.extend(words.next());
        } else if let Some(value) = long_option_value(arg, BIN_OPTION) {
            binaries.push(value);
        } else if arg == PACKAGE_OPTION || arg == PACKAGE_SHORT_OPTION {
            packages.extend(words.next());
        } else if let Some(value) = long_option_value(arg, PACKAGE_OPTION) {
            packages.push(value);
        } else if let Some(value) = arg
            .strip_prefix(PACKAGE_SHORT_OPTION)
            .filter(|value| !value.starts_with(OPTION_PREFIX))
        {
            packages.push(value);
        }
    }
    let names = if binaries.is_empty() {
        packages
    } else {
        binaries
    };
    let mut found: Vec<Run> = Vec::new();
    for tool in names.into_iter().filter_map(|name| tools.get(name)) {
        let run = Run::Development(tool.clone());
        if !found.contains(&run) {
            found.push(run);
        }
    }
    found
}

/// Gives the value of `arg` when it has the form `<option>=<value>`.
fn long_option_value<'a>(arg: &'a str, option: &str) -> Option<&'a str> {
    arg.strip_prefix(option)?.strip_prefix(VALUE_SEPARATOR)
}

/// Gives the run of a command word that holds a path. `directory` is the text
/// before the last separator. `name` is the text after it.
///
/// A path under a [`TARGET_DIRECTORY`] is a development run. A path whose
/// parent directory is a [`BIN_DIRECTORY`] is a use: `~/.cargo/bin/prcp`,
/// `/usr/local/bin/prcp`. Any other path gives no run. Each crate directory of
/// the workspace has the name of its tool, and zsh changes into a directory
/// that a person types alone (`autocd`). Thus `src/occ` is a `cd`, and a rule
/// that took every path counted 32 crate directories of a real history as
/// uses.
fn path_run(directory: &str, name: &str, tools: &ToolNames) -> Option<Run> {
    let tool = tools.get(name)?.clone();
    let mut components = directory.split(PATH_SEPARATOR);
    if components
        .clone()
        .any(|component| component == TARGET_DIRECTORY)
    {
        Some(Run::Development(tool))
    } else if components.next_back() == Some(BIN_DIRECTORY) {
        Some(Run::Use(tool))
    } else {
        None
    }
}
