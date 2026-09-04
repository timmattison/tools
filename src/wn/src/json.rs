//! Reading a plan written as JSON, the shape a program hands back.
//!
//! The four written forms of a plan — a chain, the records of a plan, the
//! Markdown table of a plan, and the box-drawn table of a plan — were all
//! written for a person to read. Three of them carry layout the reader has to
//! undo: a column width, a border, a cell that wrapped onto a second line.
//!
//! Layout is lossy. A box table that left a terminal 100 columns wide and
//! arrives in one 80 columns wide was re-wrapped by whatever pasted it. A
//! `Notes` cell that lost its second line costs nothing. An `Order` cell that
//! lost its second line costs a step.
//!
//! A plan a program wrote for a program needs no layout at all, so this module
//! reads the JSON document the `plan-parallel-work` skill prints.
//!
//! # What it reads
//!
//! `streams`, and nothing else. Each element of the `order` array of a stream
//! is one step:
//!
//! * `issue` is the issue number.
//! * `pr`, when it stands, is the pull request that does the work of that
//!   issue. The pair is the pair `PR#344 (#341)` writes, so it reaches the
//!   report as the pair the report already renders.
//! * `waitsFor` is the set of numbers that come before that step. It is the
//!   JSON spelling of the `Waits for` cell of a table, and it reaches the same
//!   graph. It holds numbers and nothing else, so a `waitsFor` that names the
//!   issue of a pair reaches the pair. A cell writes `PR#102 (#94)` and a
//!   `waitsFor` writes `94`, and both name the one piece of work, which is
//!   what keeps the two readers of one plan together.
//!
//! `housekeeping` and `warnings` are read past. They stand in the document
//! because the person who ran the skill wants them, and `wn` answers one
//! question.
//!
//! # One character claims the text
//!
//! JSON is tried before the tables and before the chain. A text whose first
//! character that is not a space is `{` is a JSON document, and nothing else
//! `wn` reads starts that way. So the claim is decided on one character and
//! never on a partial parse.
//!
//! A text that starts with `{` and does not parse is an error, and never a
//! walk on to the next reader. A reader that fell through on a broken document
//! would take a document with one missing brace to the chain reader, which
//! would then report `"version" is not an issue number`. That message names
//! the wrong problem.
//!
//! # The answer is a graph
//!
//! The `order` of a stream is a chain, so each step of it comes before the
//! step after it. The `waitsFor` of a step names the work that comes before
//! that step. Both are edges, so a JSON plan is the graph a picture draws and
//! the graph a `Waits for` column draws, and one report answers all three. Two
//! reports of one question drift apart.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::Value;
use thiserror::Error;

use crate::chain::{IssueNumber, Snippet};
use crate::graph::{of_parts, Graph, GraphError};
use crate::plan::Step;

/// The character a JSON document opens with.
const OPENING_BRACE: char = '{';

/// The version of the schema this reader knows.
const SCHEMA_VERSION: u64 = 1;

/// The key that names the version of the schema.
const VERSION: &str = "version";

/// The key that holds the streams of the plan.
const STREAMS: &str = "streams";

/// The key of a stream that holds its chain of steps.
const ORDER: &str = "order";

/// The key of a step that holds the number of its issue.
const ISSUE: &str = "issue";

/// The key of a step that holds the number of the pull request doing the work.
const PULL_REQUEST: &str = "pr";

/// The key of a step that holds the numbers that come before it.
const WAITS_FOR: &str = "waitsFor";

/// The key of a stream that holds its short name, such as `S0`.
const ID: &str = "id";

/// The key of a stream that holds the words of its name.
const NAME: &str = "name";

/// Where in the document a value stands, written `streams[1].order[0].issue`.
///
/// A newtype rather than a `String`, because the spelling of a path is then a
/// rule of the type and not a rule each message remembers. Every message that
/// names a place of the document names it the same way, so a reader who
/// followed one path follows every other one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path(String);

impl Path {
    /// The path of the key `name` of the document itself.
    #[must_use]
    fn root(name: &str) -> Self {
        Self(name.to_string())
    }

    /// The path of the element at `index` of the array this path names.
    #[must_use]
    fn at(&self, index: usize) -> Self {
        Self(format!("{}[{index}]", self.0))
    }

    /// The path of the key `name` of the object this path names.
    #[must_use]
    fn then(&self, name: &str) -> Self {
        Self(format!("{}.{name}", self.0))
    }
}

impl fmt::Display for Path {
    /// Writes the path, with nothing around it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The kind of value the schema names at one place of the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A JSON array.
    Array,
    /// A JSON object.
    Object,
    /// A JSON string.
    Text,
    /// A whole number, which is what a version is.
    Number,
    /// A number GitHub gives an issue or a pull request, so one and up.
    Issue,
}

impl Kind {
    /// The words a message writes this kind with.
    fn word(self) -> &'static str {
        match self {
            Self::Array => "an array",
            Self::Object => "an object",
            Self::Text => "a string",
            Self::Number => "a number",
            Self::Issue => "an issue number",
        }
    }
}

impl fmt::Display for Kind {
    /// Writes the words of the kind, with nothing around them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

/// Why a JSON document is not a plan.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum JsonError {
    /// The text opens with `{` and is not a JSON document.
    ///
    /// It names the text as well as the cause, because the text arrives from
    /// the clipboard as readily as from a pipe and a reader who pasted the
    /// wrong thing recognizes it by its first line.
    #[error("{text:?} is not a JSON document: {cause}")]
    NotJson {
        /// The document, cut to the length every message of this tool cuts to.
        text: Snippet,
        /// What the JSON reader said about it.
        cause: String,
    },
    /// The document names a version of the schema this reader does not know.
    ///
    /// A consumer that guesses at a schema it does not know is a consumer that
    /// answers with the wrong plan, so the run stops and the message names
    /// both versions.
    #[error(
        "the plan names version {read} of the schema, and this reader knows version \
         {SCHEMA_VERSION}"
    )]
    Version {
        /// The version the document names.
        read: u64,
    },
    /// A key the schema names stands nowhere.
    #[error("{0} is missing, and the schema of a plan names it")]
    Missing(Path),
    /// A value stands where the schema names another kind of value.
    #[error("{path} is not {wanted}")]
    Wrong {
        /// Where in the document the value stands.
        path: Path,
        /// The kind the schema names there.
        wanted: Kind,
    },
    /// The order of the plan returns to a step that comes before it.
    ///
    /// One message answers every form of a plan, because one graph carries
    /// every form. A cycle has no step to start, and an answer of "nothing is
    /// ready" hides the reason.
    #[error(transparent)]
    Order(#[from] GraphError),
}

/// The graph the JSON document `text` writes, or `None` when `text` is no JSON
/// document.
///
/// The claim and the read share all of their work, so one function does both,
/// exactly as [`crate::graph::read`] does for a picture. The claim is the
/// first character that is not a space: a `{` is a JSON document, and every
/// other text walks on to the next reader.
///
/// # Errors
///
/// Gives [`JsonError::NotJson`] for a text that opens with `{` and does not
/// parse, [`JsonError::Version`] for a version this reader does not know,
/// [`JsonError::Missing`] and [`JsonError::Wrong`] for a document that is not
/// the schema, and [`JsonError::Order`] for a plan whose steps wait for each
/// other.
#[must_use]
pub fn read(text: &str) -> Option<Result<Graph, JsonError>> {
    if !text.trim_start().starts_with(OPENING_BRACE) {
        return None;
    }
    Some(graph_of(text))
}

/// One step of the plan, as one element of an `order` array writes it.
///
/// It holds the step and the work that comes before it, because those are two
/// different things: the step becomes a node, and the numbers become edges.
struct Reading {
    /// The work itself, and the issue that work closes.
    step: Step,
    /// The numbers that come before this step.
    waits_for: Vec<IssueNumber>,
}

/// The graph the document `text` writes.
///
/// # Errors
///
/// Gives the refusals of [`JsonError`].
fn graph_of(text: &str) -> Result<Graph, JsonError> {
    let document: Value = serde_json::from_str(text).map_err(|cause| JsonError::NotJson {
        text: Snippet::new(text),
        cause: cause.to_string(),
    })?;
    refuse_version(&document)?;
    let at = Path::root(STREAMS);
    let mut streams: Vec<Vec<Reading>> = array(field(&document, STREAMS, &at)?, &at)?
        .iter()
        .enumerate()
        .map(|(place, stream)| read_stream(stream, &at.at(place)))
        .collect::<Result<_, _>>()?;
    name_the_work(&mut streams);
    Ok(of_parts(nodes_of(&streams), &edges_of(&streams))?)
}

/// The steps of one stream, in the order its `order` array writes them.
///
/// The `id` and the `name` of a stream are read for their kind and for nothing
/// else. The answer of a plan that draws edges is one list of rows in the order
/// of the work, so it carries no label of a stream to write them into. They are
/// read all the same, because a document that writes them as something other
/// than words is not the schema, and a reader that answered such a document
/// would be guessing about the rest of it.
///
/// # Errors
///
/// Gives [`JsonError::Wrong`] for a stream that is not an object, for an `id`
/// or a `name` that is not a string, and for an `order` that is not an array.
/// Gives [`JsonError::Missing`] for a stream with no `order`, and the errors of
/// [`read_step`] for one step of it.
fn read_stream(value: &Value, at: &Path) -> Result<Vec<Reading>, JsonError> {
    refuse_unless_object(value, at)?;
    for key in [ID, NAME] {
        if optional(value, key).is_some_and(|named| !named.is_string()) {
            return Err(JsonError::Wrong {
                path: at.then(key),
                wanted: Kind::Text,
            });
        }
    }
    let at = at.then(ORDER);
    array(field(value, ORDER, &at)?, &at)?
        .iter()
        .enumerate()
        .map(|(place, step)| read_step(step, &at.at(place)))
        .collect()
}

/// The one step of an `order` array that `value` writes.
///
/// A step that names a `pr` is the pair `PR#344 (#341)` writes: the pull
/// request is the work, and the issue is what the work finishes. So the number
/// of the step is the pull request, and the issue stands beside it.
///
/// # Errors
///
/// Gives [`JsonError::Wrong`] for a step that is not an object, for a number
/// that is not an issue number, and for a `waitsFor` that is not an array.
/// Gives [`JsonError::Missing`] for a step with no `issue`.
fn read_step(value: &Value, at: &Path) -> Result<Reading, JsonError> {
    refuse_unless_object(value, at)?;
    let issue_at = at.then(ISSUE);
    let issue = issue_number(field(value, ISSUE, &issue_at)?, &issue_at)?;
    let pull_at = at.then(PULL_REQUEST);
    let pull = optional(value, PULL_REQUEST)
        .map(|value| issue_number(value, &pull_at))
        .transpose()?;
    let waits_at = at.then(WAITS_FOR);
    let waits_for = match optional(value, WAITS_FOR) {
        Some(value) => array(value, &waits_at)?
            .iter()
            .enumerate()
            .map(|(place, number)| issue_number(number, &waits_at.at(place)))
            .collect::<Result<Vec<IssueNumber>, JsonError>>()?,
        None => Vec::new(),
    };
    let step = match pull {
        Some(pull) => Step::new(pull, Some(issue)),
        None => Step::new(issue, None),
    };
    Ok(Reading { step, waits_for })
}

/// The number of the work of every number the plan names.
///
/// A step names its own number. A pair names one number more: the issue its
/// pull request closes. Both numbers reach one piece of work, so both give the
/// number of that step — the pull request of a pair, and the issue of a step
/// that stands alone. A number no step names stands in the map nowhere.
///
/// A number one step carries as its own work and another step closes gives
/// itself. The work a document names directly is the work. The map is then the
/// same map however the streams of that document stand, and a rule that took
/// the first stream would answer two orders of one plan two ways. Among pairs
/// that close one issue, the first pair the document writes owns it, which is
/// the rule [`nodes_of`] holds for a number that stands in two places.
fn work_of(streams: &[Vec<Reading>]) -> BTreeMap<IssueNumber, IssueNumber> {
    let steps = || streams.iter().flatten().map(|reading| reading.step);
    let mut work: BTreeMap<IssueNumber, IssueNumber> = BTreeMap::new();
    for step in steps() {
        if let Some(closes) = step.closes() {
            work.entry(closes).or_insert_with(|| step.number());
        }
    }
    for step in steps() {
        work.insert(step.number(), step.number());
    }
    work
}

/// Rewrite every `waitsFor` number to the number of the work that carries it.
///
/// A `waitsFor` holds numbers and nothing else, so a document has no way to
/// write the pair `PR#102 (#94)` in one. A reader who waits for that work
/// writes the issue of it, and this is where that number becomes the pull
/// request that does the work.
///
/// One walk owns the rule, so [`nodes_of`] and [`edges_of`] both read numbers
/// that name work. A number no step of the plan names is left as it stands,
/// and it reaches the rows as a blocker the repository does not have.
fn name_the_work(streams: &mut [Vec<Reading>]) {
    let work = work_of(streams);
    for reading in streams.iter_mut().flatten() {
        for blocker in &mut reading.waits_for {
            if let Some(&carries) = work.get(blocker) {
                *blocker = carries;
            }
        }
    }
}

/// The steps of the whole plan, one for each number, in the order the document
/// writes them.
///
/// The walk takes the streams in the order of the document, and inside a
/// stream it takes the chain first and the work that stream waits for after
/// it. A number that stands in two places is one node, and the step of the
/// first place stands: a document that wrote the pair of a step once wrote it
/// where the step first appears. [`crate::graph::of_plan`] states the same
/// rule for a table, because a node is a node whichever form named it.
///
/// A number a `waitsFor` names and no step of the plan names is a node all the
/// same. A blocker the repository does not have must reach the rows and turn
/// the run red, and a row of the answer is the only place that says so. The
/// issue of a pair is a number a step names, because [`name_the_work`] gave
/// that `waitsFor` the number of the pull request before this walk.
fn nodes_of(streams: &[Vec<Reading>]) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    for stream in streams {
        let blockers = stream
            .iter()
            .flat_map(|reading| &reading.waits_for)
            .map(|&number| Step::new(number, None));
        for step in stream.iter().map(|reading| reading.step).chain(blockers) {
            if !steps.iter().any(|held| held.number() == step.number()) {
                steps.push(step);
            }
        }
    }
    steps
}

/// The edges the whole plan draws.
///
/// The `order` of a stream is a chain, so each step of it comes before the step
/// after it. The `waitsFor` of a step names the work that comes before that
/// step, and before that step alone.
///
/// A `waitsFor` that names the work of its own step draws no edge, because
/// such an edge runs from a step to itself and says nothing. The issue of a
/// pair names the work of that pair, so a pair that waits for its own issue
/// draws no edge either. A `waitsFor` that names a step the chain of its own
/// stream already reached draws an edge the chain already carries, and one edge
/// stands between two nodes however many times the document writes it. A
/// `waitsFor` that names a *later* step of its own stream draws an edge the
/// chain contradicts, so the plan is a cycle and [`of_parts`] refuses it.
fn edges_of(streams: &[Vec<Reading>]) -> Vec<(IssueNumber, IssueNumber)> {
    let mut edges: Vec<(IssueNumber, IssueNumber)> = Vec::new();
    for stream in streams {
        for (earlier, later) in stream.iter().zip(stream.iter().skip(1)) {
            edges.push((earlier.step.number(), later.step.number()));
        }
        for reading in stream {
            let number = reading.step.number();
            edges.extend(
                reading
                    .waits_for
                    .iter()
                    .filter(|&&blocker| blocker != number)
                    .map(|&blocker| (blocker, number)),
            );
        }
    }
    edges
}

/// Refuse a document whose `version` this reader does not know.
///
/// # Errors
///
/// Gives [`JsonError::Missing`] for a document that names no version,
/// [`JsonError::Wrong`] for a version that is not a whole number, and
/// [`JsonError::Version`] for every version but [`SCHEMA_VERSION`].
fn refuse_version(document: &Value) -> Result<(), JsonError> {
    let at = Path::root(VERSION);
    let named = field(document, VERSION, &at)?;
    let read = named.as_u64().ok_or(JsonError::Wrong {
        path: at,
        wanted: Kind::Number,
    })?;
    if read == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(JsonError::Version { read })
    }
}

/// The value the key `name` of `parent` holds.
///
/// A key that holds `null` counts as a key that stands nowhere, because a
/// writer that has nothing to say for a key writes either.
///
/// # Errors
///
/// Gives [`JsonError::Missing`] naming `at`.
fn field<'a>(parent: &'a Value, name: &str, at: &Path) -> Result<&'a Value, JsonError> {
    optional(parent, name).ok_or_else(|| JsonError::Missing(at.clone()))
}

/// The value the key `name` of `parent` holds, for a key the schema does not
/// need. `null` reads as a key that stands nowhere, as it does in [`field`].
fn optional<'a>(parent: &'a Value, name: &str) -> Option<&'a Value> {
    parent.get(name).filter(|value| !value.is_null())
}

/// The elements of the array `value` holds.
///
/// # Errors
///
/// Gives [`JsonError::Wrong`] naming `at` for a value that is no array.
fn array<'a>(value: &'a Value, at: &Path) -> Result<&'a [Value], JsonError> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| JsonError::Wrong {
            path: at.clone(),
            wanted: Kind::Array,
        })
}

/// Refuse a value that is no object.
///
/// # Errors
///
/// Gives [`JsonError::Wrong`] naming `at`.
fn refuse_unless_object(value: &Value, at: &Path) -> Result<(), JsonError> {
    if value.is_object() {
        Ok(())
    } else {
        Err(JsonError::Wrong {
            path: at.clone(),
            wanted: Kind::Object,
        })
    }
}

/// The issue number `value` writes.
///
/// GitHub numbers an issue from one and up, so a float, a negative, a string,
/// and a zero are each a number this tool cannot ask GitHub about.
///
/// # Errors
///
/// Gives [`JsonError::Wrong`] naming `at`.
fn issue_number(value: &Value, at: &Path) -> Result<IssueNumber, JsonError> {
    value
        .as_u64()
        .and_then(IssueNumber::new)
        .ok_or_else(|| JsonError::Wrong {
            path: at.clone(),
            wanted: Kind::Issue,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The document of the schema, as the `plan-parallel-work` skill writes
    /// it.
    ///
    /// The same file the test of the binary reads, so the test of this reader
    /// and the test of the command line read the same bytes. A second copy
    /// would drift.
    const DOCUMENT: &str = include_str!("../fixtures/plan-parallel-work.json");

    /// The character a JSON document closes with.
    ///
    /// The test of a document that does not parse takes it off the end, which
    /// leaves a text that still opens with `{` and holds one brace too few.
    const CLOSING_BRACE: char = '}';

    /// The box-drawn table of a plan, as it arrives on the clipboard.
    ///
    /// A reader tried first must not claim what it should not, so this reader
    /// is asked about the text the table reader answers.
    const BOX_TABLE: &str = include_str!("../fixtures/plan-parallel-work.txt");

    /// The graph `text` writes.
    fn graph_of(text: &str) -> Graph {
        read(text)
            .expect("the text is a JSON document")
            .expect("the document reads")
    }

    /// The refusal `text` earns.
    ///
    /// A [`Graph`] writes no `Debug` of itself, so this reads the error out of
    /// the answer rather than through `expect_err`.
    fn refusal(text: &str) -> JsonError {
        match read(text).expect("the text is a JSON document") {
            Ok(_) => panic!("the document reads, and this text is a refusal"),
            Err(error) => error,
        }
    }

    /// The number of every node of `graph`, sorted, so a test states the shape
    /// of the graph and never the order the steps stand in.
    fn nodes(graph: &Graph) -> Vec<u64> {
        let mut numbers: Vec<u64> = graph
            .steps()
            .iter()
            .map(|step| step.number().get())
            .collect();
        numbers.sort_unstable();
        numbers
    }

    /// The edges of `graph`: the number of the step before, and the number of
    /// the step after. Sorted, for the reason [`nodes`] sorts.
    fn edges(graph: &Graph) -> Vec<(u64, u64)> {
        let mut edges: Vec<(u64, u64)> = Vec::new();
        for (position, step) in graph.steps().iter().enumerate() {
            for &before in graph.before(position) {
                edges.push((graph.steps()[before].number().get(), step.number().get()));
            }
        }
        edges.sort_unstable();
        edges
    }

    /// The document, with `find` replaced by `put`.
    ///
    /// One edit of one text keeps every test of a refusal beside the document
    /// it refuses, so a test says which one thing it changed.
    fn edited(find: &str, put: &str) -> String {
        assert!(DOCUMENT.contains(find), "the document holds {find:?}");
        DOCUMENT.replace(find, put)
    }

    /// A document of one stream whose `order` is `order`.
    fn document_of(order: &str) -> String {
        format!("{{ \"version\": 1, \"streams\": [ {{ \"order\": {order} }} ] }}")
    }

    #[test]
    fn reads_the_two_streams_of_the_document() {
        // The chain of a stream draws an edge from each step to the step after
        // it, and a `waitsFor` draws one into the step that names it. So the
        // document draws `#96 → #91` and `#91 → #94`, and the step of `#94` is
        // the pair its pull request makes.
        let graph = graph_of(DOCUMENT);
        assert_eq!(nodes(&graph), vec![91, 96, 102]);
        assert_eq!(edges(&graph), vec![(91, 102), (96, 91)]);
    }

    #[test]
    fn a_pull_request_reaches_the_step_as_the_pair_it_makes() {
        // `"pr": 102` on the step of `#94` is the pair `PR#102 (#94)` writes:
        // the pull request is the work, and the issue is what the work
        // finishes.
        let graph = graph_of(DOCUMENT);
        let pair = graph
            .steps()
            .iter()
            .find(|step| step.number().get() == 102)
            .expect("the document names the pull request");
        assert_eq!(pair.closes().map(IssueNumber::get), Some(94));
    }

    #[test]
    fn a_step_with_no_pull_request_closes_nothing() {
        let graph = graph_of(DOCUMENT);
        let alone = graph
            .steps()
            .iter()
            .find(|step| step.number().get() == 96)
            .expect("the document names the issue");
        assert_eq!(alone.closes(), None);
    }

    #[test]
    fn a_blocker_no_stream_carries_is_a_node_of_its_own() {
        // A blocker the repository does not have must reach the rows and turn
        // the run red, and a row of the answer is the only place that says so.
        let graph = graph_of(&edited("\"waitsFor\": [96]", "\"waitsFor\": [96, 999]"));
        assert_eq!(nodes(&graph), vec![91, 96, 102, 999]);
        assert_eq!(edges(&graph), vec![(91, 102), (96, 91), (999, 91)]);
    }

    #[test]
    fn a_version_this_reader_does_not_know_is_a_refusal() {
        assert_eq!(
            refusal(&edited("\"version\": 1", "\"version\": 2")),
            JsonError::Version { read: 2 }
        );
        assert_eq!(
            refusal(&edited("\"version\": 1", "\"version\": 2")).to_string(),
            "the plan names version 2 of the schema, and this reader knows version 1"
        );
    }

    #[test]
    fn a_document_that_names_no_version_is_a_refusal() {
        // A document that names no schema is a document this reader cannot
        // know it reads correctly.
        let refused = refusal(&edited("\"version\": 1,", ""));
        assert_eq!(
            refused.to_string(),
            "version is missing, and the schema of a plan names it"
        );
    }

    #[test]
    fn a_document_that_does_not_parse_names_the_document() {
        // The chain reader never sees it. A reader that fell through here
        // would answer a missing brace with `"version" is not an issue
        // number`, which names the wrong problem.
        let broken = DOCUMENT
            .trim_end()
            .strip_suffix(CLOSING_BRACE)
            .expect("the document closes with a brace");
        let refused = refusal(broken);
        assert!(
            matches!(refused, JsonError::NotJson { .. }),
            "a broken document is not JSON, and this is {refused:?}"
        );
        assert!(
            refused.to_string().starts_with("\"{"),
            "the message names the document, and it reads {refused}"
        );
    }

    #[test]
    fn a_document_with_no_streams_names_streams() {
        let refused = refusal("{ \"version\": 1 }");
        assert_eq!(
            refused.to_string(),
            "streams is missing, and the schema of a plan names it"
        );
    }

    #[test]
    fn a_streams_that_is_not_an_array_names_its_kind() {
        let refused = refusal("{ \"version\": 1, \"streams\": {} }");
        assert_eq!(refused.to_string(), "streams is not an array");
    }

    #[test]
    fn a_stream_with_no_order_names_its_path() {
        let refused = refusal("{ \"version\": 1, \"streams\": [ { \"id\": \"S0\" } ] }");
        assert_eq!(
            refused.to_string(),
            "streams[0].order is missing, and the schema of a plan names it"
        );
    }

    #[test]
    fn a_step_with_no_issue_names_its_path() {
        // The path is what says where to look, so a document of two streams
        // names the stream as well as the step.
        let refused = refusal(&edited(
            "{ \"issue\": 91, \"waitsFor\": [96] }",
            "{ \"waitsFor\": [96] }",
        ));
        assert_eq!(
            refused.to_string(),
            "streams[1].order[0].issue is missing, and the schema of a plan names it"
        );
    }

    #[test]
    fn a_number_that_is_not_a_number_names_its_path() {
        assert_eq!(
            refusal(&document_of("[ { \"issue\": \"96\" } ]")).to_string(),
            "streams[0].order[0].issue is not an issue number"
        );
        assert_eq!(
            refusal(&document_of("[ { \"issue\": 0 } ]")).to_string(),
            "streams[0].order[0].issue is not an issue number"
        );
        assert_eq!(
            refusal(&document_of("[ { \"issue\": 1, \"pr\": -2 } ]")).to_string(),
            "streams[0].order[0].pr is not an issue number"
        );
        assert_eq!(
            refusal(&document_of(
                "[ { \"issue\": 1, \"waitsFor\": [2, \"3\"] } ]"
            ))
            .to_string(),
            "streams[0].order[0].waitsFor[1] is not an issue number"
        );
    }

    #[test]
    fn a_value_of_the_wrong_kind_names_the_kind_the_schema_wants() {
        assert_eq!(
            refusal(&document_of("{}")).to_string(),
            "streams[0].order is not an array"
        );
        assert_eq!(
            refusal(&document_of("[ 96 ]")).to_string(),
            "streams[0].order[0] is not an object"
        );
        assert_eq!(
            refusal("{ \"version\": 1, \"streams\": [ 0 ] }").to_string(),
            "streams[0] is not an object"
        );
        assert_eq!(
            refusal("{ \"version\": \"1\", \"streams\": [] }").to_string(),
            "version is not a number"
        );
        assert_eq!(
            refusal(&document_of("[ { \"issue\": 1, \"waitsFor\": 2 } ]")).to_string(),
            "streams[0].order[0].waitsFor is not an array"
        );
    }

    #[test]
    fn a_name_that_is_not_a_string_is_a_refusal() {
        // The label of a stream is the one thing outside `order` the schema
        // states, so a document that writes it as something else is not the
        // schema.
        assert_eq!(
            refusal(&edited("\"name\": \"daemon leak\"", "\"name\": 5")).to_string(),
            "streams[0].name is not a string"
        );
        assert_eq!(
            refusal(&edited("\"id\": \"S0\"", "\"id\": []")).to_string(),
            "streams[0].id is not a string"
        );
    }

    #[test]
    fn a_document_that_opens_with_space_still_reads() {
        for space in [" ", "\t", "\n", " \n\t "] {
            let text = format!("{space}{DOCUMENT}");
            assert_eq!(nodes(&graph_of(&text)), vec![91, 96, 102], "with {space:?}");
        }
    }

    #[test]
    fn an_empty_streams_array_is_a_plan_with_no_work_in_it() {
        // Not an error. Somebody ran the skill on a repository with nothing to
        // do, and a plan of nothing is the true answer to that.
        let graph = graph_of("{ \"version\": 1, \"streams\": [] }");
        assert!(nodes(&graph).is_empty(), "the plan holds no step");
    }

    #[test]
    fn steps_that_wait_for_each_other_are_a_cycle() {
        let text = "{ \"version\": 1, \"streams\": [
            { \"order\": [ { \"issue\": 91, \"waitsFor\": [96] } ] },
            { \"order\": [ { \"issue\": 96, \"waitsFor\": [91] } ] }
        ] }";
        assert_eq!(
            refusal(text).to_string(),
            "the order returns to #91 and #96, so this text names no step to start first"
        );
    }

    #[test]
    fn a_step_that_waits_for_a_step_its_own_chain_already_names_is_no_cycle() {
        // The chain of the stream already says that `#1` comes before `#2`, so
        // the cell says a true thing twice. One edge stands between two nodes,
        // however many times the document writes it.
        let graph = graph_of(&document_of(
            "[ { \"issue\": 1 }, { \"issue\": 2, \"waitsFor\": [1] } ]",
        ));
        assert_eq!(nodes(&graph), vec![1, 2]);
        assert_eq!(edges(&graph), vec![(1, 2)]);
    }

    #[test]
    fn a_step_that_waits_for_itself_draws_no_edge() {
        // Such an edge runs from a step to itself and says nothing, which is
        // the rule a `Waits for` cell that names the first step of its own
        // stream already reads under.
        let graph = graph_of(&document_of("[ { \"issue\": 1, \"waitsFor\": [1] } ]"));
        assert_eq!(nodes(&graph), vec![1]);
        assert!(edges(&graph).is_empty(), "a step waits for no step");
    }

    #[test]
    fn a_wait_for_the_issue_of_a_pair_reaches_the_pair() {
        // The pull request of a pair does the work of the issue beside it, so
        // a `waitsFor` that names the issue names that pull request. A JSON
        // document cannot spell the pair inside `waitsFor`, so a reader who
        // names the issue has no other way to write it, and a second node for
        // one piece of work tells somebody to start work a pull request
        // already does.
        let text = "{ \"version\": 1, \"streams\": [
            { \"order\": [ { \"issue\": 94, \"pr\": 102 } ] },
            { \"order\": [ { \"issue\": 91, \"waitsFor\": [94] } ] }
        ] }";
        let graph = graph_of(text);
        assert_eq!(nodes(&graph), vec![91, 102]);
        assert_eq!(edges(&graph), vec![(102, 91)]);
    }

    #[test]
    fn a_pair_that_waits_for_its_own_issue_draws_no_edge() {
        // The issue of a pair is the work of that pair, so such an edge runs
        // from a step to itself and says nothing. It is the rule a step that
        // waits for its own number already reads under.
        let graph = graph_of(&document_of(
            "[ { \"issue\": 94, \"pr\": 102, \"waitsFor\": [94] } ]",
        ));
        assert_eq!(nodes(&graph), vec![102]);
        assert!(edges(&graph).is_empty(), "a step waits for no step");
    }

    #[test]
    fn a_number_a_step_carries_as_its_own_work_keeps_itself() {
        // `#94` stands twice: one step closes it with a pull request, and one
        // step is that number itself. The work a document names directly is
        // the work, so the wait reaches the step of `#94` and the pair stands
        // beside it. The answer is the same however the streams stand, because
        // a rule that took the first stream would answer two orders of one
        // plan two ways.
        let pair = "{ \"order\": [ { \"issue\": 94, \"pr\": 102 } ] }";
        let alone = "{ \"order\": [ { \"issue\": 94 } ] }";
        let waiting = "{ \"order\": [ { \"issue\": 91, \"waitsFor\": [94] } ] }";
        for streams in [
            format!("{pair}, {alone}, {waiting}"),
            format!("{alone}, {pair}, {waiting}"),
        ] {
            let text = format!("{{ \"version\": 1, \"streams\": [ {streams} ] }}");
            let graph = graph_of(&text);
            assert_eq!(nodes(&graph), vec![91, 94, 102], "with {streams}");
            assert_eq!(edges(&graph), vec![(94, 91)], "with {streams}");
        }
    }

    #[test]
    fn a_number_that_stands_in_two_streams_is_one_node() {
        let text = "{ \"version\": 1, \"streams\": [
            { \"order\": [ { \"issue\": 1 }, { \"issue\": 2 } ] },
            { \"order\": [ { \"issue\": 2 }, { \"issue\": 3 } ] }
        ] }";
        let graph = graph_of(text);
        assert_eq!(nodes(&graph), vec![1, 2, 3]);
        assert_eq!(edges(&graph), vec![(1, 2), (2, 3)]);
    }

    #[test]
    fn the_steps_stand_in_a_topological_order() {
        // The rows of the answer are the order of the work, so the step that
        // waits for nothing stands first however the document wrote it.
        let text = "{ \"version\": 1, \"streams\": [
            { \"order\": [ { \"issue\": 91, \"waitsFor\": [96] } ] },
            { \"order\": [ { \"issue\": 96 } ] }
        ] }";
        let graph = graph_of(text);
        let order: Vec<u64> = graph
            .steps()
            .iter()
            .map(|step| step.number().get())
            .collect();
        assert_eq!(order, vec![96, 91]);
    }

    #[test]
    fn housekeeping_and_warnings_are_read_past() {
        // They stand in the document because the person who ran the skill
        // wants them, and a plan that carries prose in them is a plan all the
        // same.
        let text = "{ \"version\": 1, \"streams\": [ { \"order\": [ { \"issue\": 5 } ] } ],
            \"housekeeping\": \"anything at all\", \"warnings\": 7, \"whatever\": null }";
        assert_eq!(nodes(&graph_of(text)), vec![5]);
    }

    #[test]
    fn a_text_that_is_no_json_document_walks_on_to_the_next_reader() {
        assert!(read(BOX_TABLE).is_none(), "the box table keeps its reader");
        assert!(read("#277 → #278").is_none(), "a chain keeps its reader");
        assert!(read("").is_none(), "an empty text keeps its reader");
        assert!(
            read("| Stream | Order |").is_none(),
            "a Markdown table keeps its reader"
        );
        assert!(
            read("[ { \"issue\": 1 } ]").is_none(),
            "a JSON array is no document of this schema"
        );
    }
}
