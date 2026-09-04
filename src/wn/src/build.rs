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

use crate::chain::Snippet;
use crate::envelope::Envelope;

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

/// The seconds a run may take at the most.
///
/// A run of `claude` takes minutes, and a year is longer than a reader
/// waits, so a larger value names no run that a person starts. `Instant`
/// holds a moment and not every number of seconds after now, so a value near
/// the top of `u64` is a deadline the clock cannot hold.
const MAX_TIMEOUT_SECONDS: u64 = 31_536_000;

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
/// The subagent tool stands under both of its names. The skill dispatches one
/// when the backlog holds eight open issues or more, and that tool is named
/// `Agent` in a current `claude` and `Task` in an older one. `wn` names the
/// versions of `claude` it finds and not the one it was built beside, so it
/// carries both: a name the run does not know is read past, and a name it
/// needs and does not carry is a prompt no run under `--print` can answer.
///
/// The list names those tools and stops there. `--dangerously-skip-permissions`
/// would answer every prompt of every tool, and a tool that reaches for the
/// bypass on behalf of its reader has made a decision that is not its to make.
const ALLOWED_TOOLS: &str = "Bash Read Glob Grep Agent Task TodoWrite Skill";

/// The arguments the run is given. The prompt goes on standard input.
///
/// `--output-format json` is what makes the run say what it cost. Standard
/// output then carries one JSON envelope, and the plan is the `result` field
/// of it, which [`Envelope`] takes back out.
const ARGUMENTS: [&str; 5] = [
    "--print",
    "--output-format",
    "json",
    "--allowed-tools",
    ALLOWED_TOOLS,
];

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
///
/// Gives [`BuildError::TimeoutTooFar`] for a value above
/// [`MAX_TIMEOUT_SECONDS`]. Such a value names no run that a person starts,
/// and the largest of them build a deadline the clock cannot hold.
///
/// It reads no clock, so it gives the same answer on every machine and at
/// every moment.
pub fn seconds(value: Option<&str>) -> Result<Duration, BuildError> {
    let Some(named) = value.map(str::trim).filter(|named| !named.is_empty()) else {
        return Ok(Duration::from_secs(DEFAULT_TIMEOUT_SECONDS));
    };
    match named.parse::<u64>() {
        Ok(0) | Err(_) => Err(BuildError::BadTimeout {
            value: named.to_string(),
        }),
        Ok(read) if read > MAX_TIMEOUT_SECONDS => Err(BuildError::TimeoutTooFar {
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

/// The plan a run of `claude` builds, as the document it printed.
///
/// `paths` and `answers` are what [`find`] takes, and `timeout` is the value
/// of [`TIMEOUT_ENV`]. All three arrive as arguments rather than as reads of
/// the machine and of the environment, so the caller owns every input of the
/// run and a test of the pieces under it touches neither.
///
/// The run inherits the directory `wn` was started in, because the skill asks
/// `gh` and `git` about the repository of that directory.
///
/// A spinner runs on standard error while the run works, and the report of
/// what the run cost stands there after it, so a pipe still gets the document
/// alone.
///
/// # Errors
///
/// Gives [`BuildError::BadTimeout`] for a timeout that names no seconds,
/// [`BuildError::TimeoutTooFar`] for a timeout longer than a run waits,
/// [`BuildError::NotInstalled`] when no path holds a `claude`,
/// [`BuildError::TimedOut`] for a run that outlived its deadline,
/// [`BuildError::NotAuthenticated`] for a `claude` with no account,
/// [`BuildError::BadEnvelope`] for a run that printed no envelope, and
/// [`BuildError::Failed`] for every other failure.
///
/// The two refusals of the timeout stand before the run starts, because the
/// timeout is read before a path is found and before a child is spawned.
pub fn plan(
    paths: &[String],
    answers: &dyn Fn(&str) -> bool,
    timeout: Option<&str>,
) -> Result<String, BuildError> {
    let waited = seconds(timeout)?;
    let path = find(paths, answers)?;

    let spinner = spinner();
    let printed = ask(&path, waited);
    spinner.finish_and_clear();

    let envelope = Envelope::read(&printed?)?;
    // The report stands after the spinner is cleared, and on the pipe the
    // spinner drew on. The document goes to standard output, and a reader who
    // pipes that output must get the document alone.
    if let Some(report) = envelope.report(None) {
        eprintln!("{report}");
    }
    Ok(envelope.document().to_string())
}

/// The spinner that stands while the run works.
///
/// It draws on standard error. The document goes to standard output, and a
/// reader who pipes that output must get the document alone.
fn spinner() -> ProgressBar {
    let spinner = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
    if let Ok(style) = ProgressStyle::with_template("{spinner:.cyan} {msg}") {
        spinner.set_style(style.tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]));
    }
    spinner.set_message(WORKING);
    spinner.enable_steady_tick(POLL);
    spinner
}

/// Hand `path` the prompt and read the document it prints.
///
/// # Errors
///
/// Gives [`BuildError::TimedOut`] when the run outlives `waited`, and the
/// refusals of [`refusal_of`] for a run that ended with a failure. Such a
/// refusal carries the reason [`reason_of`] picks out of the two pipes and
/// out of the write of the prompt.
fn ask(path: &str, waited: Duration) -> Result<String, BuildError> {
    let mut child = Command::new(path)
        .args(ARGUMENTS)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|cause| BuildError::Failed {
            said: cause.to_string(),
        })?;

    // Both pipes are drained on threads of their own, and they are taken
    // before the prompt is written. A pipe nobody reads fills up, and a child
    // that writes into a full pipe blocks there until the deadline. A write
    // that fails must find the two readers in place as well, because what the
    // run said is the reason to report and the error of the pipe is not.
    let printed = child.stdout.take().map(read_whole);
    let complained = child.stderr.take().map(read_whole);

    // The pipe is closed as soon as the prompt is in it. A `claude` that reads
    // standard input waits for the end of it, so a pipe left open is a run
    // that never starts and then dies at the deadline.
    //
    // A write that fails ends nothing here. A `claude` that exits before it
    // reads the prompt closes the read end, and the write then fails with a
    // broken pipe — which names the pipe and never the reason the run went. So
    // the error is kept as one candidate reason, and the child is waited for
    // as every other child is: no run is left behind, and the deadline stands.
    let mut pipe = None;
    if let Some(mut mouth) = child.stdin.take() {
        if let Err(cause) = mouth.write_all(PROMPT.as_bytes()) {
            pipe = Some(cause.to_string());
        }
    }

    let status = wait_for(&mut child, waited)?;
    let printed = printed.map_or_else(String::new, joined);
    let complained = complained.map_or_else(String::new, joined);

    if status.success() {
        // A run that answered with a plan says the write of the prompt got
        // there. An error of that pipe is then worth nothing, so it is dropped.
        Ok(printed)
    } else {
        Err(refusal_of(&reason_of(
            &complained,
            &printed,
            pipe.as_deref(),
        )))
    }
}

/// Read the whole of `pipe` on a thread of its own.
fn read_whole<R>(mut pipe: R) -> thread::JoinHandle<String>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut read = Vec::new();
        // A pipe that ends early is a run that ended early, and the status of
        // the run is what says so. What was read up to that point is kept.
        let _ = pipe.read_to_end(&mut read);
        String::from_utf8_lossy(&read).into_owned()
    })
}

/// What the thread `reader` read, or nothing when the thread went down.
fn joined(reader: thread::JoinHandle<String>) -> String {
    reader.join().unwrap_or_default()
}

/// Wait for `child`, and kill it when it outlives `waited`.
///
/// # Errors
///
/// Gives [`BuildError::TimedOut`] for a run that outlived its deadline and
/// for a `waited` the clock cannot hold, and [`BuildError::Failed`] when the
/// state of the child could not be read.
fn wait_for(child: &mut Child, waited: Duration) -> Result<ExitStatus, BuildError> {
    // `seconds` refuses a value above MAX_TIMEOUT_SECONDS, and this fallback
    // stands under it. This function takes the duration and never the value
    // that named it, and the clock is read twice: once here for the deadline
    // and once for each look at the child. A duration at the edge of what the
    // clock holds must kill the child and give a refusal, and it must never
    // panic with the run left alive.
    let Some(deadline) = Instant::now().checked_add(waited) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(BuildError::TimedOut {
            seconds: waited.as_secs(),
        });
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(cause) => {
                return Err(BuildError::Failed {
                    said: cause.to_string(),
                })
            }
        }
        if Instant::now() >= deadline {
            // The run is killed here and not left to the exit of this process.
            // A `claude` nobody waits for keeps reading the backlog, and it
            // keeps spending while it does.
            let _ = child.kill();
            let _ = child.wait();
            return Err(BuildError::TimedOut {
                seconds: waited.as_secs(),
            });
        }
        thread::sleep(POLL);
    }
}

/// The reason a run that failed gave, out of the pipes and the write.
///
/// `complained` is what the run wrote on standard error, `printed` is what it
/// wrote on standard output, and `pipe` is the error of the write of the
/// prompt, for a write that failed.
///
/// The order is the order in which a candidate names the run. Standard error
/// is where a program writes a reason, so it stands first. Standard output is
/// where a run that mixes the two writes it, so it stands next. The error of
/// the pipe describes this tool and not the run — a broken pipe says the run
/// was gone before it read the prompt, and never why it went — so it stands
/// last.
///
/// A candidate counts only when it holds a character that is not whitespace.
/// A pipe that carried one newline carried nothing, and it must not stand in
/// front of the pipe that carried the reason.
fn reason_of(complained: &str, printed: &str, pipe: Option<&str>) -> String {
    [complained, printed]
        .into_iter()
        .chain(pipe)
        .find(|candidate| !candidate.trim().is_empty())
        .unwrap_or_default()
        .to_string()
}

/// The refusal a run that failed earns, out of the reason it gave.
///
/// [`reason_of`] picks that reason out of the pipes of the run. A run that
/// could not log in is the one failure with an answer the reader can act on,
/// so it is the one failure that is told apart. Every other one carries what
/// `claude` said, because this tool cannot know what that is.
///
/// Every mark names a whole phrase: the name of the command, or the words a
/// run writes when it turns the reader away. A bare `login` names none of
/// those, because a reason that only holds those letters — the name of a
/// login server, for one — is a failure of something else, and
/// [`BuildError::NotAuthenticated`] carries no text, so such a run loses the
/// reason it gave.
pub fn refusal_of(said: &str) -> BuildError {
    let clause = said.trim();
    let lowered = clause.to_lowercase();
    if ["not authenticated", "/login", "log in"]
        .iter()
        .any(|mark| lowered.contains(mark))
    {
        return BuildError::NotAuthenticated;
    }
    BuildError::Failed {
        said: clause.to_string(),
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
    /// The value of [`TIMEOUT_ENV`] names more seconds than a run waits.
    #[error(
        "{TIMEOUT_ENV} names {value:?} seconds, and no run waits that long. Name \
         {MAX_TIMEOUT_SECONDS} seconds at the most: {TIMEOUT_ENV}={DEFAULT_TIMEOUT_SECONDS}"
    )]
    TimeoutTooFar {
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
    #[error(
        "claude took longer than {seconds} seconds to build a plan. {TIMEOUT_ENV} names a \
         different number of seconds."
    )]
    TimedOut {
        /// The seconds it was given.
        seconds: u64,
    },
    /// `claude` has no account to run under.
    #[error("claude is not logged in. Run: claude login")]
    NotAuthenticated,
    /// The directory the run would happen in is in no repository.
    ///
    /// The skill asks `gh` and `git` about the repository of the directory
    /// `wn` was run in, and its gather script turns a failure of either into a
    /// warning rather than a crash. So a run in such a directory spends a
    /// minute and real money and then answers that the plan holds no work.
    /// The refusal stands before the run, where it costs one cheap call.
    ///
    /// It names which of the two repositories failed. `--repo` names the
    /// repository `wn` asks about and never the one a run plans, and the
    /// reader of this message has often passed it already.
    #[error(
        "a plan is built for the repository of this directory, and gh can name none for it. \
         Run wn inside a checkout — --repo names the repository wn asks about and never the one \
         a run plans.\n{said}"
    )]
    NoRepository {
        /// What said the directory is in no repository.
        said: String,
    },
    /// The run printed something that is no envelope.
    ///
    /// The run is asked for `--output-format json`, so what it prints is one
    /// JSON envelope and the plan is one field of it. A text that is no
    /// envelope is a `claude` that answered in another shape, and the plan
    /// reader must never be handed it: the refusal would then name the plan
    /// and the fault is in the run.
    #[error("claude answered with {text:?}, which is no JSON envelope: {cause}")]
    BadEnvelope {
        /// What the run printed, cut to the length every message of this tool
        /// cuts to.
        text: Snippet,
        /// What the JSON reader said about it.
        cause: String,
    },
    /// The run failed for a reason only `claude` knows.
    #[error("claude could not build a plan: {said}")]
    Failed {
        /// The reason the run gave, on whichever pipe carried it, with the
        /// space around it dropped.
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
        // The envelope is what says what the run cost. The plan is one field
        // of it, so a run without this pair prints a plan nobody priced.
        assert!(ARGUMENTS.contains(&"--output-format"), "{ARGUMENTS:?}");
        assert!(ARGUMENTS.contains(&"json"), "{ARGUMENTS:?}");
        assert!(ARGUMENTS.contains(&"--allowed-tools"), "{ARGUMENTS:?}");
        assert!(ARGUMENTS.contains(&ALLOWED_TOOLS), "{ARGUMENTS:?}");
        assert!(
            !ARGUMENTS.contains(&"--dangerously-skip-permissions"),
            "{ARGUMENTS:?}"
        );
        // The gather script of the skill is a program, and Bash is what runs
        // it.
        assert!(ALLOWED_TOOLS.contains("Bash"), "{ALLOWED_TOOLS}");
        // The skill dispatches a subagent when the backlog holds eight open
        // issues or more, which is the case this tool is built for. That tool
        // is named Agent in a current `claude` and Task in an older one, and
        // `wn` names the versions of `claude` it finds rather than the one it
        // was built beside. A name the run does not know is read past, and a
        // name it needs and does not carry is a permission prompt no run under
        // --print can answer.
        for spelling in ["Agent", "Task"] {
            assert!(ALLOWED_TOOLS.contains(spelling), "{ALLOWED_TOOLS}");
        }
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
    fn a_failure_that_only_holds_the_word_login_keeps_its_reason() {
        // The bare mark claimed such a run, and NotAuthenticated carries no text,
        // so the reason went missing.
        let said = "could not reach the login server: 503";
        assert_eq!(
            refusal_of(said),
            BuildError::Failed {
                said: said.to_string()
            }
        );
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
    fn standard_error_is_the_first_place_a_reason_is_read_from() {
        // A program writes a reason on standard error, so that pipe outranks
        // the document the run printed and outranks the error of the pipe the
        // prompt went into.
        assert_eq!(
            reason_of(
                "the model is overloaded",
                "half a plan",
                Some("Broken pipe (os error 32)")
            ),
            "the model is overloaded"
        );
    }

    #[test]
    fn standard_output_stands_when_standard_error_held_nothing() {
        // A run that mixes the two pipes writes its reason on standard output.
        // A standard error of one newline carried nothing, and it must not
        // stand in front of the pipe that carried the reason.
        assert_eq!(
            reason_of(
                " \n ",
                "the model is overloaded",
                Some("Broken pipe (os error 32)")
            ),
            "the model is overloaded"
        );
    }

    #[test]
    fn the_error_of_the_pipe_stands_only_when_both_pipes_held_nothing() {
        // It describes this tool and not the run, so it is the last thing
        // read. It is still better than a refusal that stops at the colon.
        assert_eq!(
            reason_of("", "\n", Some("Broken pipe (os error 32)")),
            "Broken pipe (os error 32)"
        );
    }

    #[test]
    fn a_run_that_said_nothing_anywhere_gives_no_reason() {
        assert_eq!(reason_of("", "", None), "");
        assert_eq!(reason_of("  ", " \n ", Some("  \t ")), "");
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
    fn a_directory_in_no_repository_says_that_naming_one_does_not_help() {
        // The reader of this message has often passed --repo already, and the
        // message the repository reader writes tells them to pass it. --repo
        // names the repository `wn` asks about and never the one a run plans,
        // so the message says which of the two failed.
        let refused = BuildError::NoRepository {
            said: "`gh repo view` failed.".to_string(),
        };
        assert_eq!(
            refused.to_string(),
            "a plan is built for the repository of this directory, and gh can name none for it. \
Run wn inside a checkout — --repo names the repository wn asks about and never the one a run \
plans.\n`gh repo view` failed."
        );
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

    #[test]
    fn a_timeout_no_run_waits_is_a_refusal() {
        let refused = seconds(Some("18446744073709551615")).expect_err("no run waits that long");
        assert!(refused.to_string().contains(TIMEOUT_ENV), "{refused}");
    }

    #[test]
    fn the_cap_itself_is_a_timeout_a_run_takes() {
        // The cap is the last value the refusal lets through, and a reader who
        // names it gets the run they asked for.
        assert_eq!(
            seconds(Some("31536000")),
            Ok(Duration::from_secs(31_536_000))
        );
    }

    #[test]
    fn a_timeout_no_clock_can_hold_kills_the_child_rather_than_panicking() {
        // The panic this pins left the child alive: `wn` died at the addition and
        // the run it started was reparented to PID 1.
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("5")
            .spawn()
            .expect("/bin/sleep");
        let refused = wait_for(&mut child, Duration::from_secs(u64::MAX))
            .expect_err("no clock holds that deadline");
        assert!(
            matches!(refused, BuildError::TimedOut { .. }),
            "{refused:?}"
        );
    }
}
