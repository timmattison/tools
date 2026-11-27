//! A `git` on `PATH` that logs each call and can pause before a subcommand.
//!
//! `gitscratch` starts git through [`childenv::fixture_command`], which resolves
//! `git` through the `PATH` of the process and sheds every `GIT_*` variable.
//! So `GIT_TRACE` cannot log the calls. The one way to see which git processes
//! a replay starts is a `git` that comes first on `PATH`.
//!
//! The test process cannot change its own `PATH`, because the environment is
//! process-wide and the tests of one binary run on many threads. A test runs
//! the code under test in a child process instead, with
//! [`GitShim::path_variable`] as the `PATH` of that child. See
//! [`crate::testing::run_child_half`].

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;

use super::executable::write_executable;

/// How long the shim pauses before a subcommand of `pause_before`.
///
/// It is longer than `PROGRESS_DELAY` (500 ms, in `console.rs`), so a display
/// that waits for its delay before it shows has time to show.
pub const PAUSE: Duration = Duration::from_secs(1);

/// The byte that ends one argument in the log.
const ARGUMENT_END: char = '\u{1f}';

/// The byte that ends one call in the log. It is not a newline, so an argument
/// that holds a newline cannot break the parse.
const CALL_END: char = '\u{1e}';

/// A fake `git` that logs its arguments and then runs the real one.
pub struct GitShim {
    dir: TempDir,
}

/// One call of the shim, as the arguments git received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCall {
    /// The arguments, without the program name.
    pub arguments: Vec<String>,
}

impl GitShim {
    /// Write the shim script, and pause for [`PAUSE`] before each subcommand
    /// named in `pause_before`. An empty slice never pauses.
    ///
    /// The script is written with [`write_executable`], because a test runs it.
    /// On Linux, a script that this process held open for writing gives
    /// `ETXTBSY` when another thread forks before the run.
    ///
    /// # Panics
    ///
    /// Panics when no executable `git` is on the `PATH` of this process, or
    /// when the script cannot be written.
    #[must_use]
    pub fn new(pause_before: &[&str]) -> Self {
        let dir = TempDir::new().expect("create the shim directory");
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).expect("create the bin directory of the shim");
        let script = bin.join("git");
        let log = dir.path().join("calls.log");
        let real_git = real_git();

        let patterns: Vec<String> = pause_before
            .iter()
            .map(|name| shellquote::shell_quote(name))
            .collect();
        let pause = if patterns.is_empty() {
            String::new()
        } else {
            format!(
                "case \"$(subcommand \"$@\")\" in\n    {}) sleep {} ;;\nesac\n",
                patterns.join("|"),
                PAUSE.as_secs()
            )
        };
        let contents = format!(
            "#!/bin/sh\n\
             log={log}\n\
             printf '%s\\037' \"$@\" >> \"$log\"\n\
             printf '\\036' >> \"$log\"\n\
             subcommand() {{\n    \
                 while [ \"$#\" -ge 1 ]; do\n        \
                     case \"$1\" in\n            \
                         -c) [ \"$#\" -ge 2 ] || break; shift 2 ;;\n            \
                         --*) shift ;;\n            \
                         *) break ;;\n        \
                     esac\n    \
                 done\n    \
                 printf '%s' \"$1\"\n\
             }}\n\
             {pause}\
             exec {real} \"$@\"\n",
            log = shellquote::shell_quote(&log.to_string_lossy()),
            real = shellquote::shell_quote(&real_git.to_string_lossy()),
        );
        write_executable(&script, contents).expect("write the git shim");
        Self { dir }
    }

    /// The `PATH` that puts the shim first, then each entry of the `PATH` of
    /// this process.
    ///
    /// # Panics
    ///
    /// Panics when an entry of `PATH` holds a separator and cannot be joined.
    #[must_use]
    pub fn path_variable(&self) -> OsString {
        let mut entries = vec![self.dir.path().join("bin")];
        if let Some(path) = std::env::var_os("PATH") {
            entries.extend(std::env::split_paths(&path));
        }
        std::env::join_paths(entries).expect("join the PATH of the shim")
    }

    /// Each call the shim logged so far, in order. No log yet means no call.
    #[must_use]
    pub fn calls(&self) -> Vec<GitCall> {
        let log = std::fs::read(self.dir.path().join("calls.log")).unwrap_or_default();
        parse_calls(&String::from_utf8_lossy(&log))
    }
}

impl GitCall {
    /// The git subcommand of this call. It skips each leading `-c <value>` pair
    /// and each leading `--flag`, the way gitscratch puts its safety
    /// configuration and `--literal-pathspecs` before the subcommand.
    #[must_use]
    pub fn subcommand(&self) -> Option<&str> {
        let mut rest = self.arguments.as_slice();
        while let Some(first) = rest.first() {
            if first == "-c" && rest.len() >= 2 {
                rest = &rest[2..];
            } else if first.starts_with("--") {
                rest = &rest[1..];
            } else {
                break;
            }
        }
        rest.first().map(String::as_str)
    }

    /// True when `run` appears as consecutive arguments of this call.
    #[must_use]
    pub fn has_arguments(&self, run: &[&str]) -> bool {
        !run.is_empty()
            && self
                .arguments
                .windows(run.len())
                .any(|window| window.iter().map(String::as_str).eq(run.iter().copied()))
    }
}

/// Split a log into calls and each call into arguments.
fn parse_calls(log: &str) -> Vec<GitCall> {
    let mut records: Vec<&str> = log.split(CALL_END).collect();
    if records.last().is_some_and(|last| last.is_empty()) {
        records.pop();
    }
    records
        .into_iter()
        .map(|record| {
            let mut arguments: Vec<String> =
                record.split(ARGUMENT_END).map(str::to_owned).collect();
            if arguments.last().is_some_and(String::is_empty) {
                arguments.pop();
            }
            GitCall { arguments }
        })
        .collect()
}

/// The first `git` on the `PATH` of this process that is a file with an
/// execute bit. The shim directory is never on that `PATH`, so the shim cannot
/// find itself.
fn real_git() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH is set, so a git can be found");
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|candidate| is_executable_file(candidate))
        .expect("no executable `git` on PATH, so the git shim has nothing to run")
}

/// True for a regular file with at least one execute bit.
fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(arguments: &[&str]) -> GitCall {
        GitCall {
            arguments: arguments.iter().map(|&a| a.to_owned()).collect(),
        }
    }

    /// A log of two calls parses to two calls, and an argument that holds a
    /// newline and a space stays one argument.
    #[test]
    fn the_log_parses_into_calls_and_arguments() {
        let log = "status\u{1f}-s\u{1f}\u{1e}commit\u{1f}-m\u{1f}two\nlines here\u{1f}\u{1e}";
        assert_eq!(
            parse_calls(log),
            [
                call(&["status", "-s"]),
                call(&["commit", "-m", "two\nlines here"])
            ]
        );
    }

    /// An empty log is no call.
    #[test]
    fn an_empty_log_holds_no_call() {
        assert!(parse_calls("").is_empty());
    }

    /// The subcommand follows each leading `-c` pair, and a lone `-c` ends the
    /// walk instead of looping.
    #[test]
    fn the_subcommand_skips_leading_configuration_pairs() {
        assert_eq!(
            call(&["-c", "a=b", "-c", "c=d", "rebase", "--continue"]).subcommand(),
            Some("rebase")
        );
        assert_eq!(
            call(&["-c", "a=b", "--literal-pathspecs", "rev-parse"]).subcommand(),
            Some("rev-parse")
        );
        assert_eq!(call(&["-c"]).subcommand(), Some("-c"));
        assert_eq!(call(&[]).subcommand(), None);
    }

    /// A run of arguments matches only as consecutive arguments.
    #[test]
    fn a_run_of_arguments_matches_only_when_consecutive() {
        let rev_parse = call(&["rev-parse", "--git-path", "rebase-merge"]);
        assert!(rev_parse.has_arguments(&["--git-path", "rebase-merge"]));
        assert!(!rev_parse.has_arguments(&["rev-parse", "rebase-merge"]));
        assert!(!rev_parse.has_arguments(&[]));
    }

    /// The shim logs a call and runs the real git, and it pauses only before
    /// a subcommand it was told about.
    #[test]
    fn the_shim_logs_a_call_and_runs_the_real_git() {
        let shim = GitShim::new(&["never-a-subcommand"]);
        let output = childenv::fixture_command(shim.dir.path().join("bin/git"))
            .args(["-c", "a=b", "--literal-pathspecs", "--version"])
            .output()
            .expect("run the shim");
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).starts_with("git version"));
        assert_eq!(
            shim.calls(),
            [call(&["-c", "a=b", "--literal-pathspecs", "--version"])]
        );
    }
}
