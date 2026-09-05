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
//!
//! The run says what it cost, because a reader who pays for one must learn the
//! price, and it says what it is doing, because a run of ten minutes behind one
//! constant is a run nobody can tell from a dead one.
//!
//! `--output-format stream-json --verbose` carries both. It makes the run write
//! one JSON object for each event as the event happens, which [`crate::stream`]
//! reads on a thread and [`crate::progress`] puts on the line. The last of
//! those objects is the envelope that carries the plan and the price beside it,
//! which [`crate::envelope`] takes apart. A run that failed and printed an
//! envelope says what it cost as well, because such a run spent the money
//! before it failed.
//!
//! The two pipes are read to their end before the wait is judged. `claude`
//! writes the whole envelope and only then exits, and those two moments are not
//! the same moment. So a run killed at its deadline can hold a finished plan in
//! the pipe, and a path that gave up on the wait first would throw that plan
//! away.

use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::chain::Snippet;
use crate::envelope::Envelope;
use crate::progress::Progress;
use crate::stream::{self, Transcript};

/// The variable that turns the run off.
///
/// It has the shape [`crate::input::NO_CLIPBOARD_ENV`] has: any value with a
/// character in it turns the run off, and an empty value leaves it on.
pub const NO_CLAUDE_ENV: &str = "WN_NO_CLAUDE";

/// The variable that names the seconds a run may take.
pub const TIMEOUT_ENV: &str = "WN_PLAN_TIMEOUT";

/// The variable that names the level of effort a run asks for.
pub const EFFORT_ENV: &str = "WN_PLAN_EFFORT";

/// The variable that names the model a run asks for.
pub const MODEL_ENV: &str = "WN_PLAN_MODEL";

/// The levels of effort a run may ask for.
///
/// The envelope of a run carries no field that names one, so the report can
/// only name the level the run asked for. That is why the level is read here
/// and passed on, rather than taken out of the answer.
const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The level of effort a run asks for.
///
/// A newtype rather than a `String`, because the value holds one rule every
/// reader of it depends on: it is one of [`EFFORT_LEVELS`]. A level the run
/// does not know is a run that stops before it starts, and the report would
/// then name a level nothing ran at.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Effort(String);

/// The model a run asks for.
///
/// A newtype rather than a `String`, for the rule it holds: the value is not
/// empty, and it does not open with a dash. A value that opens with a dash is
/// a flag, and a variable that can put a flag on the command line of the run
/// decides what the run is allowed to do. That decision belongs to the reader
/// and never to a variable.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelName(String);

impl Effort {
    /// The level `value`, the value of [`EFFORT_ENV`], names.
    ///
    /// An absent value gives `None`, and so does a value of nothing but
    /// whitespace: an exported but empty variable is a common accident. The
    /// run then asks for no level and the report names none, because a report
    /// that named a level nobody chose is worth nothing.
    ///
    /// The case of the value is the reader's to choose, so `HIGH` is `high`.
    ///
    /// # Errors
    ///
    /// Gives [`BuildError::BadEffort`] for a value that is not one of
    /// [`EFFORT_LEVELS`]. A reader who wrote `WN_PLAN_EFFORT=quick` and got
    /// the default back would learn nothing about why the plan still cost what
    /// it cost.
    fn new(value: Option<&str>) -> Result<Option<Self>, BuildError> {
        let Some(named) = value.map(str::trim).filter(|named| !named.is_empty()) else {
            return Ok(None);
        };
        let lowered = named.to_lowercase();
        if EFFORT_LEVELS.contains(&lowered.as_str()) {
            Ok(Some(Self(lowered)))
        } else {
            Err(BuildError::BadEffort {
                value: named.to_string(),
            })
        }
    }

    /// The level, as the command line and the report write it.
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl ModelName {
    /// The model `value`, the value of [`MODEL_ENV`], names.
    ///
    /// An absent value gives `None`, and so does a value of nothing but
    /// whitespace. The run then asks for no model, and the report names the
    /// models the answer says the run really used.
    ///
    /// # Errors
    ///
    /// Gives [`BuildError::BadModel`] for a value that opens with a dash.
    /// Every other value goes through: the models of `claude` are named by
    /// `claude` and not by this tool, so a list here would refuse a model that
    /// shipped after this build.
    fn new(value: Option<&str>) -> Result<Option<Self>, BuildError> {
        let Some(named) = value.map(str::trim).filter(|named| !named.is_empty()) else {
            return Ok(None);
        };
        if named.starts_with('-') {
            return Err(BuildError::BadModel {
                value: named.to_string(),
            });
        }
        Ok(Some(Self(named.to_string())))
    }

    /// The model, as the command line writes it.
    fn as_str(&self) -> &str {
        &self.0
    }
}

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
/// `--output-format stream-json` is what makes the run say what it is doing.
/// Standard output then carries one JSON object for each event of the run, one
/// to a line, written as the event happens. [`crate::stream`] reads them, and
/// the line on standard error names the tool the run just reached for.
///
/// The last of those objects is the envelope that says what the run cost, and
/// the plan is its `result` field, which [`Envelope`] takes back out. The
/// plain `json` mode carries that envelope and nothing else, so it can say
/// what a run cost and it can never say what a run is doing.
///
/// `--verbose` is not a choice. `claude` refuses the stream without it.
const ARGUMENTS: [&str; 6] = [
    "--print",
    "--output-format",
    "stream-json",
    "--verbose",
    "--allowed-tools",
    ALLOWED_TOOLS,
];

/// The arguments of one run: [`ARGUMENTS`], and the level and the model the
/// environment named.
///
/// A run that names neither gets [`ARGUMENTS`] and nothing more, so the two
/// variables cost the reader who sets neither of them nothing at all.
fn arguments(effort: Option<&Effort>, model: Option<&ModelName>) -> Vec<String> {
    let mut carried: Vec<String> = ARGUMENTS.iter().map(ToString::to_string).collect();
    if let Some(effort) = effort {
        carried.push(EFFORT_FLAG.to_string());
        carried.push(effort.as_str().to_string());
    }
    if let Some(model) = model {
        carried.push(MODEL_FLAG.to_string());
        carried.push(model.as_str().to_string());
    }
    carried
}

/// The flag that names the level of effort of a run.
const EFFORT_FLAG: &str = "--effort";

/// The flag that names the model of a run.
const MODEL_FLAG: &str = "--model";

/// How often a waiting run is asked whether it is finished.
const POLL: Duration = Duration::from_millis(100);

/// The seconds the readers of a killed run are given to reach the end of the
/// two pipes.
///
/// A run that ended on its own closed both pipes, so its readers are waited for
/// with no bound at all. A run that was killed is a run whose write end can
/// outlive it, because a tool the run started can hold the pipe open, and a
/// wait with no bound there would outlive the deadline it reports. This is the
/// drain of a pipe that is already closed and never a read of a run that still
/// works.
const GRACE: Duration = Duration::from_secs(5);

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

/// The clause of a refusal that says how far a run got, or nothing at all for a
/// run that printed nothing.
///
/// A run writes one JSON object for each event, so this text is the end of what
/// the transcript kept and never a document. The end rather than the front:
/// every run opens with the same event, so the front of one reads like the
/// front of every other, and the newest events are what tell a run that was
/// working from a run that only started. It is here because the bytes a killed
/// run wrote are the only evidence that the run was working, and a refusal that
/// dropped them lost them for good.
fn got_as_far_as(printed: &Snippet) -> String {
    if printed.is_empty() {
        return String::new();
    }
    format!("\n\nIt got as far as: {printed:?}")
}

/// What the environment said about one run.
///
/// One struct rather than three arguments, because every one of them is a read
/// of process-global state and `main` is the one place that reads it. The
/// functions under it take values, so a test of them touches no environment.
pub struct Settings<'a> {
    /// The value of [`TIMEOUT_ENV`].
    pub timeout: Option<&'a str>,
    /// The value of [`EFFORT_ENV`].
    pub effort: Option<&'a str>,
    /// The value of [`MODEL_ENV`].
    pub model: Option<&'a str>,
}

/// The plan a run of `claude` builds, as the document it printed.
///
/// `paths` and `answers` are what [`find`] takes, and `settings` is what the
/// environment said. All of them arrive as arguments rather than as reads of
/// the machine and of the environment, so the caller owns every input of the
/// run and a test of the pieces under it touches neither.
///
/// The run inherits the directory `wn` was started in, because the skill asks
/// `gh` and `git` about the repository of that directory.
///
/// A line runs on standard error while the run works, saying how long the run
/// waited against its deadline and which tool it just reached for. The report
/// of what the run cost stands there after it, so a pipe still gets the
/// document alone. A run that failed and printed an envelope earns that report
/// as well, because it spent the money before it failed.
///
/// A run that outlived its deadline and finished the plan before it was killed
/// answers with that plan, and the deadline stands on standard error above the
/// price.
///
/// # Errors
///
/// Gives [`BuildError::BadTimeout`] for a timeout that names no seconds,
/// [`BuildError::TimeoutTooFar`] for a timeout longer than a run waits,
/// [`BuildError::BadEffort`] for a level that is not one of
/// [`EFFORT_LEVELS`], [`BuildError::BadModel`] for a model that opens with a
/// dash, [`BuildError::NotInstalled`] when no path holds a `claude`,
/// [`BuildError::TimedOut`] for a run that outlived its deadline,
/// [`BuildError::NotAuthenticated`] for a `claude` with no account,
/// [`BuildError::BadEnvelope`] for a run that printed no envelope, and
/// [`BuildError::Failed`] for every other failure.
///
/// The refusals of the settings stand before the run starts, because all
/// three are read before a path is found and before a child is spawned. A
/// value the run cannot use must cost no run at all.
pub fn plan(
    paths: &[String],
    answers: &dyn Fn(&str) -> bool,
    settings: &Settings,
) -> Result<String, BuildError> {
    let waited = seconds(settings.timeout)?;
    let effort = Effort::new(settings.effort)?;
    let model = ModelName::new(settings.model)?;
    let path = find(paths, answers)?;

    let progress = Progress::start(waited);
    let answered = ask(&path, waited, effort.as_ref(), model.as_ref(), &progress);
    progress.stop();

    let answer = answered?;
    // The deadline came and the run had finished the plan all the same. Both
    // facts stand on standard error, above the price.
    if let Some(seconds) = answer.overran {
        eprintln!("{}", overran_line(seconds));
    }
    // The report stands after the line is cleared, and on the pipe that line
    // drew on. The document goes to standard output, and a reader who pipes
    // that output must get the document alone.
    //
    // It also stands before the answer is taken. A run that failed after
    // several turns spent the money before it failed, so the reader of such a
    // run learns the price as well.
    if let Some(report) = answer.envelope.report(effort.as_ref().map(Effort::as_str)) {
        eprintln!("{report}");
    }
    Ok(answer.envelope.answer()?.to_string())
}

/// The line a run that outlived its deadline and finished the plan earns.
///
/// The plan goes to standard output all the same, so this line says two things:
/// the deadline came, and `wn` answers with the plan the run finished before
/// it. A reader who saw the deadline alone would believe the run answered
/// nothing, and the plan they paid for would be gone.
fn overran_line(seconds: u64) -> String {
    format!(
        "claude took longer than {seconds} seconds, and it had finished the plan by then. \
         wn answers with that plan. {TIMEOUT_ENV} names a different number of seconds."
    )
}

/// What one run of `claude` answered.
#[derive(Debug)]
struct Answer {
    /// The envelope the run printed.
    envelope: Envelope,
    /// The seconds the deadline gave, for a run that outlived it and printed a
    /// whole envelope before it was killed.
    overran: Option<u64>,
}

/// Hand `path` the prompt and read the envelope out of the stream it writes.
///
/// `progress` is the line the run stands behind, and what the run reaches for
/// goes on it as the run reaches for it.
///
/// Both pipes are read to their end on every path out of this function. A run
/// that outlived `waited` and printed a whole envelope before it was killed
/// answers with the plan of that envelope, and the [`Answer`] then names the
/// seconds the deadline gave.
///
/// # Errors
///
/// Gives [`BuildError::TimedOut`] when the run outlives `waited` and printed no
/// whole envelope, and [`BuildError::BadEnvelope`] for a run that printed no
/// envelope. Gives the refusals of [`refusal_of`] for a run that ended with a
/// failure and printed no envelope that says why. Such a refusal carries the
/// reason [`reason_of`] picks out of the two pipes and out of the write of the
/// prompt.
///
/// A run that ended with a failure and printed an envelope that says so gives
/// that envelope. The reason then stands in its `result`, and
/// [`Envelope::answer`] is what hands it on.
fn ask(
    path: &str,
    waited: Duration,
    effort: Option<&Effort>,
    model: Option<&ModelName>,
    progress: &Progress,
) -> Result<Answer, BuildError> {
    let mut child = Command::new(path)
        .args(arguments(effort, model))
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
    //
    // Standard output carries the stream of events, so its reader reads one
    // line at a time and puts each reach on the line as it arrives. A reader
    // that joined at the end would say what the run did at the one moment
    // nobody needs telling any more.
    let doing = progress.doing();
    let printed = child.stdout.take().map(|pipe| {
        Reading::start(move || stream::transcribe(pipe, move |reach| doing.set(reach)))
    });
    let complained = child
        .stderr
        .take()
        .map(|pipe| Reading::start(move || read_whole(pipe)));

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

    let waiting = wait_for(&mut child, waited);
    // Both pipes are read to their end before the wait is judged. A `?` on the
    // wait would drop the reader of standard output without taking what it
    // read, and what it read is the plan: `claude` writes the whole envelope
    // and only then exits.
    //
    // A run that ended on its own is waited for with no bound. A run that was
    // killed gets the grace, because a tool it started can hold the write end
    // open and a wait with no bound would then outlive the deadline it reports.
    let grace = waiting.as_ref().err().map(|_| GRACE);
    let printed = printed
        .and_then(|reading| reading.taken(grace))
        .unwrap_or_default();
    let complained = complained
        .and_then(|reading| reading.taken(grace))
        .unwrap_or_default();

    let status = match waiting {
        Ok(status) => status,
        Err(Unfinished::Overran(seconds)) => return answer_past_the_deadline(seconds, &printed),
        Err(Unfinished::Unreadable(said)) => return Err(BuildError::Failed { said }),
    };

    if status.success() {
        // A run that answered with a plan says the write of the prompt got
        // there. An error of that pipe is then worth nothing, so it is dropped.
        Envelope::read(printed.envelope()).map(|envelope| Answer {
            envelope,
            overran: None,
        })
    } else {
        // The envelope stands in front of the pipes for a run that failed. A
        // failing run prints its envelope as well, and the `result` of that
        // envelope carries the sentence a reader can act on. Standard error
        // carries a machine tag on the likeliest mistake of all — a model that
        // does not exist — and a tag names the fault without saying what to do
        // about it.
        //
        // A run that printed no envelope falls back to the pipes, and so does
        // one whose envelope says it did not fail: such an envelope says
        // nothing at all about the failure the exit status reports.
        match Envelope::read(printed.envelope()) {
            Ok(envelope) if envelope.answer().is_err() => Ok(Answer {
                envelope,
                overran: None,
            }),
            _ => Err(refusal_of(&reason_of(
                &complained,
                printed.printed(),
                pipe.as_deref(),
            ))),
        }
    }
}

/// The answer a run that outlived its deadline earns, out of what it printed.
///
/// `claude` writes the whole plan in one `result` line and then exits, and a
/// run killed between those two moments holds a finished plan in the pipe. So
/// the plan is looked for before the refusal is built, and a run that printed
/// one answers with it.
///
/// A line that parses is a line the run finished. [`crate::stream`] keeps a
/// line as the envelope only when the whole of it parses as JSON, so the half
/// line a killed run leaves behind is never handed on as a document.
///
/// # Errors
///
/// Gives [`BuildError::TimedOut`] for a run that printed no whole envelope, and
/// for one whose envelope carries a reason rather than a plan. The refusal
/// carries the end of what the run printed, cut, so the reader learns how far
/// it got.
fn answer_past_the_deadline(seconds: u64, printed: &Transcript) -> Result<Answer, BuildError> {
    match Envelope::read(printed.envelope()) {
        Ok(envelope) if envelope.answer().is_ok() => Ok(Answer {
            envelope,
            overran: Some(seconds),
        }),
        _ => Err(BuildError::TimedOut {
            seconds,
            printed: Snippet::tail(printed.printed()),
        }),
    }
}

/// One pipe of the run, read on a thread of its own.
///
/// A pipe nobody reads fills up, and a child that writes into a full pipe
/// blocks there until the deadline. So both pipes are read as the run writes,
/// and this is what the reader hands its answer back through.
///
/// A channel rather than a [`std::thread::JoinHandle`], because a join has no
/// bound and the paths that kill the child need one.
struct Reading<T> {
    /// Where the reader hands what it read.
    from: mpsc::Receiver<T>,
}

impl<T: Send + 'static> Reading<T> {
    /// Start `read` on a thread of its own.
    fn start(read: impl FnOnce() -> T + Send + 'static) -> Self {
        let (to, from) = mpsc::channel();
        thread::spawn(move || {
            // A send that fails is a caller that stopped waiting, and the
            // thread has nothing more to do about it.
            let _ = to.send(read());
        });
        Self { from }
    }

    /// What the reader read, or nothing when it read nothing in time and
    /// nothing when the thread went down.
    ///
    /// `grace` bounds the wait. `None` waits for the end of the pipe, which is
    /// where a run that ended on its own leaves it.
    fn taken(self, grace: Option<Duration>) -> Option<T> {
        match grace {
            None => self.from.recv().ok(),
            Some(grace) => self.from.recv_timeout(grace).ok(),
        }
    }
}

/// The whole of `pipe`, as text.
///
/// A pipe that ends early is a run that ended early, and the status of the run
/// is what says so. What was read up to that point is kept.
fn read_whole<R: Read>(mut pipe: R) -> String {
    let mut read = Vec::new();
    let _ = pipe.read_to_end(&mut read);
    String::from_utf8_lossy(&read).into_owned()
}

/// Why a run gave no exit status.
///
/// No refusal is built here, and that is the point. Both of these stand on a
/// path that has to read the two pipes to their end first, and a [`BuildError`]
/// built where the wait happens is one a caller carries past those readers with
/// a `?`. That is how the plan of a run that beat its deadline by a second was
/// thrown away.
#[derive(Debug)]
enum Unfinished {
    /// The run outlived the seconds it was given, and it was killed.
    Overran(u64),
    /// The state of the child could not be read, and it was killed.
    Unreadable(String),
}

/// Wait for `child`, and kill it when it outlives `waited`.
///
/// The child is killed and reaped on every path out that carries an
/// [`Unfinished`], which is what lets the caller read both pipes to their end:
/// a child that still holds a write end open is a read that never ends.
///
/// # Errors
///
/// Gives [`Unfinished::Overran`] for a run that outlived its deadline and for a
/// `waited` the clock cannot hold, and [`Unfinished::Unreadable`] when the state
/// of the child could not be read.
fn wait_for(child: &mut Child, waited: Duration) -> Result<ExitStatus, Unfinished> {
    // `seconds` refuses a value above MAX_TIMEOUT_SECONDS, and this fallback
    // stands under it. This function takes the duration and never the value
    // that named it, and the clock is read twice: once here for the deadline
    // and once for each look at the child. A duration at the edge of what the
    // clock holds must kill the child and give a refusal, and it must never
    // panic with the run left alive.
    let Some(deadline) = Instant::now().checked_add(waited) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Unfinished::Overran(waited.as_secs()));
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(cause) => {
                // The child is killed here as well. The caller reads both pipes
                // to their end after this, and a child that still holds a write
                // end open is a read that never ends.
                let _ = child.kill();
                let _ = child.wait();
                return Err(Unfinished::Unreadable(cause.to_string()));
            }
        }
        if Instant::now() >= deadline {
            // The run is killed here and not left to the exit of this process.
            // A `claude` nobody waits for keeps reading the backlog, and it
            // keeps spending while it does.
            let _ = child.kill();
            let _ = child.wait();
            return Err(Unfinished::Overran(waited.as_secs()));
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
pub(crate) fn refusal_of(said: &str) -> BuildError {
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
         different number of seconds.{}",
        got_as_far_as(.printed)
    )]
    TimedOut {
        /// The seconds it was given.
        seconds: u64,
        /// The end of what the run printed on standard output before it was
        /// killed, cut to the length every message of this tool cuts to.
        ///
        /// A run that printed a whole envelope answers with the plan of it, so
        /// this text is never a plan. It says how far the run got, which is
        /// what tells a run that was working from a run that never started.
        /// The end rather than the front, because every run opens with the same
        /// event and only the newest ones say which run this was.
        printed: Snippet,
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
    /// The value of [`EFFORT_ENV`] is not one of [`EFFORT_LEVELS`].
    #[error(
        "{EFFORT_ENV} names {value:?}, and it names one of {}: {EFFORT_ENV}=high",
        EFFORT_LEVELS.join(", ")
    )]
    BadEffort {
        /// The value the environment named, with the space around it dropped.
        value: String,
    },
    /// The value of [`MODEL_ENV`] opens with a dash, so it names a flag.
    #[error(
        "{MODEL_ENV} names {value:?}, which opens with a dash, and a model is no flag. A variable \
         that can put a flag on the command line of the run decides what the run may do, and that \
         decision is yours: {MODEL_ENV}=claude-opus-5"
    )]
    BadModel {
        /// The value the environment named, with the space around it dropped.
        value: String,
    },
    /// The run printed something that is no envelope.
    ///
    /// The run is asked for `--output-format stream-json`, so what it prints
    /// is one JSON object for each event of the run. The last of those
    /// objects is the envelope, and the plan is one field of it.
    /// [`Transcript::envelope`] picks that `result` line out of the stream,
    /// and for a run that wrote none it gives the end of what the run printed
    /// instead. This refusal stands when what it gives is no envelope.
    ///
    /// A text that is no envelope is a `claude` that answered in another
    /// shape, and the plan reader must never be handed it: the refusal would
    /// then name the plan and the fault is in the run.
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
        /// The reason the run gave, with the space around it dropped.
        ///
        /// It comes out of the envelope the run printed, which is where
        /// `claude` writes the sentence a reader can act on. A run that
        /// printed no envelope gives it on one of the two pipes instead, and
        /// standard output gives the end of what it wrote there.
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
        // The stream is what says what the run does while it works, and its
        // last line is the envelope that says what the run cost. A run without
        // these three prints a plan nobody priced, behind a line that says the
        // same words for ten minutes.
        assert!(ARGUMENTS.contains(&"--output-format"), "{ARGUMENTS:?}");
        assert!(ARGUMENTS.contains(&"stream-json"), "{ARGUMENTS:?}");
        // `claude` refuses the stream without this flag.
        assert!(ARGUMENTS.contains(&"--verbose"), "{ARGUMENTS:?}");
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
        let refused = BuildError::TimedOut {
            seconds: 600,
            printed: Snippet::new(""),
        };
        let message = refused.to_string();
        assert!(message.contains("600"), "{message}");
        assert!(message.contains(TIMEOUT_ENV), "{message}");
    }

    #[test]
    fn a_run_that_printed_something_before_its_deadline_says_how_far_it_got() {
        let refused = BuildError::TimedOut {
            seconds: 600,
            printed: Snippet::new("half of an answer"),
        };
        let message = refused.to_string();
        assert!(message.contains("half of an answer"), "{message}");
    }

    #[test]
    fn a_run_that_printed_nothing_quotes_nothing() {
        // An empty quotation says less than no quotation at all.
        let refused = BuildError::TimedOut {
            seconds: 600,
            printed: Snippet::new("   "),
        };
        let message = refused.to_string();
        assert!(!message.contains("got as far as"), "{message}");
    }

    /// The transcript of a run that printed `text` on standard output.
    fn transcript(text: &str) -> Transcript {
        stream::transcribe(std::io::Cursor::new(text.to_string()), |_| {})
    }

    /// The `result` line that closes the stream of a run that answered
    /// `document`.
    ///
    /// Built with the JSON writer rather than with `format!`, because a plan
    /// holds newlines and quotation marks and every one of them has to be
    /// escaped.
    fn envelope_line(document: &str) -> String {
        serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": document,
        })
        .to_string()
    }

    #[test]
    fn a_killed_run_that_had_finished_the_envelope_answers_with_the_plan() {
        // `claude` writes the whole envelope and only then exits, and those two
        // moments are not the same moment. A run killed between them holds a
        // finished plan, and the reader already paid for it.
        let printed = transcript(&format!("{}\n", envelope_line("the plan")));
        let answer = answer_past_the_deadline(600, &printed).expect("the envelope is whole");
        assert_eq!(answer.overran, Some(600));
        assert_eq!(answer.envelope.answer().expect("a plan"), "the plan");
    }

    #[test]
    fn a_killed_run_that_wrote_half_an_envelope_hands_no_plan_back() {
        // A run killed in the middle of a write leaves broken JSON, and the
        // reader of a plan must never be handed it as a document. What it wrote
        // stands in the refusal instead.
        let whole = envelope_line("the plan");
        let half: String = whole.chars().take(whole.chars().count() / 2).collect();
        let printed = transcript(&half);
        let refused = answer_past_the_deadline(600, &printed).expect_err("the envelope is broken");
        assert!(
            matches!(refused, BuildError::TimedOut { seconds: 600, .. }),
            "{refused:?}"
        );
        assert!(refused.to_string().contains("got as far as"), "{refused}");
    }

    #[test]
    fn a_killed_run_is_quoted_from_the_end_of_what_it_printed() {
        // Every run of `claude` opens with the same event, so a quotation of
        // the front of the stream reads the same for a run that printed one
        // event and for a run that worked nine minutes. The end is what tells
        // those two apart, and the transcript drops its own front as the run
        // goes on, so the front is not even the front of the run by then.
        let printed = transcript(concat!(
            "the first line\nthe second line\nthe third line\n",
            "the fourth line\nthe fifth line\nthe last line\n"
        ));
        let refused = answer_past_the_deadline(600, &printed).expect_err("there is no envelope");
        let message = refused.to_string();
        assert!(message.contains("the last line"), "{message}");
        assert!(!message.contains("the first line"), "{message}");
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
        assert!(matches!(refused, Unfinished::Overran(..)), "{refused:?}");
    }

    #[test]
    fn the_five_levels_are_the_levels_a_run_may_ask_for() {
        for level in ["low", "medium", "high", "xhigh", "max"] {
            assert_eq!(
                Effort::new(Some(level))
                    .expect("the level stands")
                    .map(|effort| effort.as_str().to_string()),
                Some(level.to_string())
            );
        }
    }

    #[test]
    fn an_environment_that_names_no_level_asks_for_none() {
        // A report that named a level nobody chose is worth nothing, so a run
        // that asked for none says nothing about one.
        for value in [None, Some(""), Some("  \t ")] {
            assert_eq!(Effort::new(value), Ok(None), "{value:?}");
        }
    }

    #[test]
    fn a_level_that_is_not_one_of_the_five_is_a_refusal() {
        let refused = Effort::new(Some("quick")).expect_err("quick is no level");
        assert_eq!(
            refused,
            BuildError::BadEffort {
                value: "quick".to_string()
            }
        );
        let message = refused.to_string();
        assert!(message.contains(EFFORT_ENV), "{message}");
        for level in EFFORT_LEVELS {
            assert!(message.contains(level), "{message}");
        }
    }

    #[test]
    fn the_case_of_a_level_is_the_readers_to_choose() {
        assert_eq!(
            Effort::new(Some(" HIGH "))
                .expect("the level stands")
                .map(|effort| effort.as_str().to_string()),
            Some("high".to_string())
        );
    }

    #[test]
    fn the_model_the_environment_names_is_the_model() {
        assert_eq!(
            ModelName::new(Some(" claude-opus-5 "))
                .expect("the model stands")
                .map(|model| model.as_str().to_string()),
            Some("claude-opus-5".to_string())
        );
    }

    #[test]
    fn an_environment_that_names_no_model_asks_for_none() {
        for value in [None, Some(""), Some("  ")] {
            assert_eq!(ModelName::new(value), Ok(None), "{value:?}");
        }
    }

    #[test]
    fn a_model_that_opens_with_a_dash_is_a_refusal() {
        // A variable that can put a flag on the command line of the run
        // decides what the run may do, and this file already says that
        // decision is the reader's and never this tool's.
        let refused =
            ModelName::new(Some("--dangerously-skip-permissions")).expect_err("a flag is no model");
        assert_eq!(
            refused,
            BuildError::BadModel {
                value: "--dangerously-skip-permissions".to_string()
            }
        );
        assert!(refused.to_string().contains(MODEL_ENV), "{refused}");
    }

    #[test]
    fn a_run_that_names_neither_carries_the_arguments_and_nothing_more() {
        assert_eq!(
            arguments(None, None),
            ARGUMENTS
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_level_and_the_model_reach_the_command_line_of_the_run() {
        let effort = Effort::new(Some("high"))
            .expect("the level stands")
            .expect("a level was named");
        let model = ModelName::new(Some("claude-opus-5"))
            .expect("the model stands")
            .expect("a model was named");
        let carried = arguments(Some(&effort), Some(&model));
        for pair in [["--effort", "high"], ["--model", "claude-opus-5"]] {
            let at = carried
                .iter()
                .position(|argument| argument == pair[0])
                .unwrap_or_else(|| panic!("{} stands in {carried:?}", pair[0]));
            assert_eq!(carried.get(at + 1).map(String::as_str), Some(pair[1]));
        }
        // The arguments that were always there are still there.
        for argument in ARGUMENTS {
            assert!(
                carried.iter().any(|carried| carried == argument),
                "{argument} in {carried:?}"
            );
        }
    }
}
