//! Building the plan by running `claude`.
//!
//! `wn` answers a plan somebody already wrote, and the reader who has none has
//! a repository full of open issues instead. The plan is one `claude` run
//! away, and `wn` already knows the repository, so the tool that answers "what
//! is next" answers it from an empty clipboard as well.
//!
//! The run is the fourth input, and it is the quietest of the four. An
//! argument was typed on purpose, a pipe was built on purpose, and the
//! clipboard was neither. A run that costs money and a minute of waiting is
//! quieter still, so it answers only when the other three did not.

use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use thiserror::Error;

/// The variable that turns the run off.
///
/// It has the shape [`crate::input::NO_CLIPBOARD_ENV`] has: any value with a
/// character in it turns the run off, and an empty value leaves it on.
pub const NO_CLAUDE_ENV: &str = "WN_NO_CLAUDE";

/// The variable that names the seconds a run may take.
pub const TIMEOUT_ENV: &str = "WN_PLAN_TIMEOUT";

/// The seconds a run may take when the environment names none.
///
/// `inscribe` waits 120 seconds for a commit message. A plan of a whole
/// backlog reads every open issue and every open pull request of the
/// repository, which is a longer run, so this one waits ten minutes.
const DEFAULT_TIMEOUT_SECONDS: u64 = 600;

/// The prompt the run is handed.
///
/// A constant rather than a literal at the spawn, because a test asserts it. A
/// rename of the skill must become a build that stops, and not a run that
/// quietly asks for something else.
pub const PROMPT: &str = "/plan-parallel-work --json";

/// The name of the binary, as `PATH` carries it.
const CLAUDE: &str = "claude";

/// The tools the run is allowed to reach for.
///
/// The skill runs its `gather.ts` script, which shells out to `gh` and to
/// `git`, and it reads the repository to place each issue in a zone. A run
/// under `--print` has no terminal to answer a permission prompt with, so a
/// tool it needs and cannot reach hangs the run until the timeout and then
/// reports nothing.
///
/// The list names those tools and stops there. `--dangerously-skip-permissions`
/// would answer every prompt of every tool, and a tool that reaches for the
/// bypass on behalf of its reader has made a decision that is not its to make.
const ALLOWED_TOOLS: &str = "";

/// The arguments the run is given. The prompt goes on standard input.
const ARGUMENTS: [&str; 0] = [];

/// How often a waiting run is asked whether it is finished.
const POLL: Duration = Duration::from_millis(100);

/// The words the spinner writes while the run works.
const WORKING: &str = "plan-parallel-work: reading the backlog…";

/// Whether `value`, the value of [`NO_CLAUDE_ENV`], turns the run off.
///
/// Takes the value as an argument rather than reading the environment, so a
/// test of it touches no process-global state. This mirrors
/// [`crate::input::clipboard_is_off`].
///
/// A value of nothing but whitespace leaves the run on. An exported but empty
/// variable is a common accident, and it is not the same statement as
/// `WN_NO_CLAUDE=1`.
#[must_use]
pub fn claude_is_off(value: Option<&str>) -> bool {
    value.is_some_and(|named| !named.trim().is_empty())
}

/// How long a run may take, as `value`, the value of [`TIMEOUT_ENV`], names it.
///
/// An absent value gives [`DEFAULT_TIMEOUT_SECONDS`], and so does a value of
/// nothing but whitespace.
///
/// # Errors
///
/// Gives [`BuildError::BadTimeout`] for a value that is not a number of
/// seconds, and for a zero. A reader who wrote `WN_PLAN_TIMEOUT=10m` and got
/// the default back would learn nothing about why the run still took ten
/// minutes, and a zero is a confusing way to spell [`NO_CLAUDE_ENV`].
pub fn seconds(value: Option<&str>) -> Result<Duration, BuildError> {
    let Some(named) = value.map(str::trim).filter(|named| !named.is_empty()) else {
        return Ok(Duration::from_secs(DEFAULT_TIMEOUT_SECONDS));
    };
    match named.parse::<u64>() {
        Ok(0) | Err(_) => Err(BuildError::BadTimeout {
            value: named.to_string(),
        }),
        Ok(read) => Ok(Duration::from_secs(read)),
    }
}

/// The places `claude` stands, in the order they are tried.
///
/// `home` is the value of `HOME`, which the caller reads. A machine that names
/// no home has no home directory to look under, so the two paths that name one
/// are left out rather than written as `/.local/bin/claude`.
#[must_use]
pub fn candidate_paths(home: Option<&str>) -> Vec<String> {
    let mut paths = vec![CLAUDE.to_string()];
    if let Some(home) = home.map(str::trim).filter(|home| !home.is_empty()) {
        let home = home.trim_end_matches('/');
        paths.push(format!("{home}/.local/bin/{CLAUDE}"));
        paths.push(format!("{home}/.claude/local/{CLAUDE}"));
    }
    paths.push(format!("/usr/local/bin/{CLAUDE}"));
    paths
}

/// The `claude` at `path`, when it answers `--version`.
///
/// The probe every run uses. [`find`] takes it as an argument so a test of the
/// order of the paths spawns nothing at all: a test that ran this probe would
/// answer differently on a machine that has `claude` and on one that does not,
/// which is a test of the machine rather than of the code.
#[must_use]
pub fn answers_version(path: &str) -> bool {
    std::process::Command::new(path)
        .arg("--version")
        .output()
        .is_ok_and(|answer| answer.status.success())
}

/// The first path of `paths` that `answers` names as a working `claude`.
///
/// # Errors
///
/// Gives [`BuildError::NotInstalled`] when no path answers. The message names
/// every path it looked in, and it names [`NO_CLAUDE_ENV`] for a reader who
/// wants no run at all.
pub fn find(paths: &[String], answers: &dyn Fn(&str) -> bool) -> Result<String, BuildError> {
    paths
        .iter()
        .find(|path| answers(path))
        .map(ToString::to_string)
        .ok_or_else(|| BuildError::NotInstalled {
            looked_in: paths.to_vec(),
        })
}

/// The paths of a refusal, one to a line and indented under it.
fn looked_in_lines(paths: &[String]) -> String {
    paths
        .iter()
        .map(|path| {
            // The bare name is the one entry that is no path at all. A reader
            // who sees it beside three absolute paths has to guess where it
            // was looked for, so the line says so.
            if path.contains('/') {
                format!("  {path}")
            } else {
                format!("  {path} (on PATH)")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The refusal a run that failed earns, out of what it wrote on standard error.
///
/// A run that could not log in is the one failure with an answer the reader
/// can act on, so it is the one failure that is told apart. Every other one
/// carries what `claude` said, because this tool cannot know what that is.
fn refusal_of(said: &str) -> BuildError {
    BuildError::Failed {
        said: String::new(),
    }
}

/// Why no plan came back.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BuildError {
    /// The value of [`TIMEOUT_ENV`] is not a number of seconds.
    #[error(
        "{TIMEOUT_ENV} names {value:?}, and it names a number of seconds, one and up: \
         {TIMEOUT_ENV}={DEFAULT_TIMEOUT_SECONDS}"
    )]
    BadTimeout {
        /// The value the environment named, with the space around it dropped.
        value: String,
    },
    /// No path holds a `claude` that answers `--version`.
    #[error(
        "claude is not installed, and wn builds a plan by running it.\n\nIt looked in:\n{}\n\n\
         Install it from https://claude.ai/code, or set {NO_CLAUDE_ENV} to any value to turn the \
         run off.",
        looked_in_lines(.looked_in)
    )]
    NotInstalled {
        /// Every path that was tried, in the order they were tried.
        looked_in: Vec<String>,
    },
    /// The run took longer than it was given.
    #[error("PLACEHOLDER {seconds}")]
    TimedOut {
        /// The seconds it was given.
        seconds: u64,
    },
    /// `claude` has no account to run under.
    #[error("PLACEHOLDER")]
    NotAuthenticated,
    /// The run failed for a reason only `claude` knows.
    #[error("PLACEHOLDER {said}")]
    Failed {
        /// What the run wrote on standard error, with the space around it
        /// dropped.
        said: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_with_a_character_in_it_turns_the_run_off() {
        assert!(!claude_is_off(None));
        assert!(!claude_is_off(Some("")));
        assert!(!claude_is_off(Some("   ")));
        assert!(claude_is_off(Some("1")));
        assert!(claude_is_off(Some("no")));
    }

    #[test]
    fn an_environment_that_names_no_timeout_waits_ten_minutes() {
        assert_eq!(seconds(None), Ok(Duration::from_secs(600)));
        assert_eq!(seconds(Some("")), Ok(Duration::from_secs(600)));
        assert_eq!(seconds(Some("  \t ")), Ok(Duration::from_secs(600)));
    }

    #[test]
    fn the_named_number_of_seconds_is_the_timeout() {
        assert_eq!(seconds(Some("30")), Ok(Duration::from_secs(30)));
        assert_eq!(seconds(Some(" 90 ")), Ok(Duration::from_secs(90)));
    }

    #[test]
    fn a_timeout_that_is_not_a_number_of_seconds_is_a_refusal() {
        let refused = seconds(Some("10m")).expect_err("10m is not a number of seconds");
        assert_eq!(
            refused,
            BuildError::BadTimeout {
                value: "10m".to_string()
            }
        );
        assert_eq!(
            refused.to_string(),
            "WN_PLAN_TIMEOUT names \"10m\", and it names a number of seconds, one and up: \
             WN_PLAN_TIMEOUT=600"
        );
    }

    #[test]
    fn a_timeout_of_zero_is_a_refusal() {
        // A run that may take no time at all is killed the moment it starts,
        // which is a confusing way to spell WN_NO_CLAUDE.
        assert_eq!(
            seconds(Some("0")),
            Err(BuildError::BadTimeout {
                value: "0".to_string()
            })
        );
    }

    /// The paths of a machine that has `claude` under its home directory.
    fn paths() -> Vec<String> {
        candidate_paths(Some("/Users/x"))
    }

    #[test]
    fn the_first_path_that_answers_is_the_one() {
        let paths = paths();
        let found = find(&paths, &|path| path == "/Users/x/.claude/local/claude")
            .expect("one path answers");
        assert_eq!(found, "/Users/x/.claude/local/claude");
    }

    #[test]
    fn a_path_earlier_in_the_list_wins() {
        let paths = paths();
        let found = find(&paths, &|_| true).expect("every path answers");
        assert_eq!(found, "claude");
    }

    #[test]
    fn no_path_is_tried_after_the_one_that_answered() {
        let paths = paths();
        let tried = std::cell::RefCell::new(Vec::new());
        let found = find(&paths, &|path| {
            tried.borrow_mut().push(path.to_string());
            path == "/Users/x/.local/bin/claude"
        })
        .expect("one path answers");
        assert_eq!(found, "/Users/x/.local/bin/claude");
        assert_eq!(
            tried.into_inner(),
            vec![
                "claude".to_string(),
                "/Users/x/.local/bin/claude".to_string()
            ]
        );
    }

    #[test]
    fn a_machine_with_no_claude_names_every_path_and_the_variable() {
        let paths = paths();
        let refused = find(&paths, &|_| false).expect_err("no path answers");
        assert_eq!(
            refused,
            BuildError::NotInstalled {
                looked_in: paths.clone()
            }
        );
        let message = refused.to_string();
        for path in &paths {
            assert!(message.contains(path.as_str()), "{message}");
        }
        // The bare name is the one entry that is no path at all, so the
        // message says where it was looked for.
        assert!(message.contains("claude (on PATH)"), "{message}");
        assert!(message.contains(NO_CLAUDE_ENV), "{message}");
        assert!(message.contains("https://claude.ai/code"), "{message}");
    }

    #[test]
    fn the_run_names_the_tools_the_skill_needs_and_never_the_bypass() {
        // A run under --print has no terminal to answer a permission prompt
        // with, so a tool the skill needs and cannot reach hangs the run. The
        // bypass flag answers every prompt of every tool, and that decision is
        // the reader\'s to make and not this tool\'s.
        assert!(ARGUMENTS.contains(&"--print"), "{ARGUMENTS:?}");
        assert!(ARGUMENTS.contains(&"--allowed-tools"), "{ARGUMENTS:?}");
        assert!(ARGUMENTS.contains(&ALLOWED_TOOLS), "{ARGUMENTS:?}");
        assert!(
            !ARGUMENTS.contains(&"--dangerously-skip-permissions"),
            "{ARGUMENTS:?}"
        );
        // The gather script of the skill is a program, and Bash is what runs
        // it.
        assert!(ALLOWED_TOOLS.contains("Bash"), "{ALLOWED_TOOLS}");
    }

    #[test]
    fn a_run_that_could_not_log_in_names_claude_login() {
        for said in [
            "Invalid API key · Please run /login",
            "Error: not authenticated",
            "You must log in first",
        ] {
            let refused = refusal_of(said);
            assert_eq!(refused, BuildError::NotAuthenticated, "{said}");
            assert!(refused.to_string().contains("claude login"), "{refused}");
        }
    }

    #[test]
    fn every_other_failure_carries_what_claude_said() {
        let refused = refusal_of("  the model is overloaded.\n");
        assert_eq!(
            refused,
            BuildError::Failed {
                said: "the model is overloaded.".to_string()
            }
        );
        let message = refused.to_string();
        assert!(message.contains("claude"), "{message}");
        assert!(message.contains("the model is overloaded."), "{message}");
    }

    #[test]
    fn a_failure_that_said_nothing_still_names_claude() {
        let refused = refusal_of("   \n ");
        assert_eq!(
            refused,
            BuildError::Failed {
                said: String::new()
            }
        );
        assert!(refused.to_string().contains("claude"), "{refused}");
    }

    #[test]
    fn a_run_that_outlived_its_deadline_names_the_seconds_and_the_variable() {
        let refused = BuildError::TimedOut { seconds: 600 };
        let message = refused.to_string();
        assert!(message.contains("600"), "{message}");
        assert!(message.contains(TIMEOUT_ENV), "{message}");
    }

    #[test]
    fn the_prompt_names_the_skill_and_the_json_mode() {
        // A rename of the skill must become a build that stops here, and not a
        // run that quietly asks for something else.
        assert!(PROMPT.contains("plan-parallel-work"), "{PROMPT}");
        assert!(PROMPT.contains("--json"), "{PROMPT}");
    }

    #[test]
    fn the_four_places_claude_stands_are_tried_in_order() {
        assert_eq!(
            candidate_paths(Some("/Users/x")),
            vec![
                "claude".to_string(),
                "/Users/x/.local/bin/claude".to_string(),
                "/Users/x/.claude/local/claude".to_string(),
                "/usr/local/bin/claude".to_string(),
            ]
        );
    }

    #[test]
    fn a_machine_that_names_no_home_leaves_the_two_paths_under_one_out() {
        for home in [None, Some(""), Some("  ")] {
            assert_eq!(
                candidate_paths(home),
                vec!["claude".to_string(), "/usr/local/bin/claude".to_string()],
                "{home:?}"
            );
        }
    }

    #[test]
    fn a_home_that_ends_with_a_slash_does_not_double_it() {
        assert_eq!(
            candidate_paths(Some("/Users/x/")),
            vec![
                "claude".to_string(),
                "/Users/x/.local/bin/claude".to_string(),
                "/Users/x/.claude/local/claude".to_string(),
                "/usr/local/bin/claude".to_string(),
            ]
        );
    }
}
