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

use std::time::Duration;

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
