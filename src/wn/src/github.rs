//! Asking GitHub what it holds for each number of the chain.
//!
//! The whole chain is one question, and it is asked as one GraphQL query with
//! one alias for each number. A REST call for each issue would ask the same
//! thing in six round trips, spend six units of the rate limit, and answer a
//! chain of six issues six times slower.
//!
//! The query goes through `gh`, and it goes through `gh` for the credential.
//! The GitHub CLI already holds a token for the host the repository lives on,
//! it refreshes that token, and it knows the enterprise host a repository sits
//! behind. A second token in a second place is a second thing to keep.
//!
//! # Every number is asked about as an issue or a pull request
//!
//! `issueOrPullRequest` answers for both, and a chain is written by hand, so a
//! number in it is a pull request sooner or later. Asking `issue(number:)`
//! alone would answer `null` for that number, and the tool would then report a
//! pull request that exists as a number the repository does not have.
//!
//! # A missing number is an answer, and a refusal is not
//!
//! GraphQL answers a number the repository does not have with `null` for that
//! alias and an entry in a top-level `errors` list. `gh` reads that list and
//! exits non-zero. The body on standard output still carries every other
//! answer, so this module reads the body whenever there is one and reads the
//! exit status only for a run that printed nothing at all. One typo in a chain
//! of six thus costs one row of the output rather than the whole run.
//!
//! The `null` alone does not say the repository lacks the number. GitHub
//! writes the same `null` for an alias it refuses to answer for, and puts an
//! error beside it in the same list. Only the `type` of that error parts the
//! two: `NOT_FOUND` is the number the repository does not have, and every
//! other type — `FORBIDDEN`, `INTERNAL`, `SERVICE_UNAVAILABLE` — says GitHub
//! could not answer. So this module reads the reason beside each `null`, and
//! it fails on a reason that is not `NOT_FOUND`. A run that reported such a
//! number as missing would print the red note `#N is not in owner/name` and
//! send the reader to hunt for a typo they did not make.

use std::collections::HashMap;
use std::fmt;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use crate::chain::IssueNumber;
use crate::report::{Entry, Status};

/// The GitHub CLI, which carries the credential and the host.
const GH: &str = "gh";

/// A repository, as GitHub names one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    owner: String,
    name: String,
}

impl Repo {
    /// Read a repository out of an `owner/name` argument.
    ///
    /// # Errors
    ///
    /// Fails when the argument is not two non-empty parts divided by one `/`.
    pub fn parse(spec: &str) -> Result<Self> {
        let mut parts = spec.split('/');
        let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
            bail!("{spec:?} is not a repository. Write it as owner/name");
        };
        if owner.is_empty() || name.is_empty() {
            bail!("{spec:?} is not a repository. Write it as owner/name");
        }
        Ok(Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    /// The account or the organization that owns the repository.
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The name of the repository inside that account.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for Repo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// The repository of the current directory, as `gh` resolves it.
///
/// # Errors
///
/// Fails when `gh` is not installed, when the current directory is in no
/// repository `gh` can name, or when `gh` answers something that is not
/// `owner/name`.
pub fn current_repo() -> Result<Repo> {
    let output = Command::new(GH)
        .args([
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "-q",
            ".nameWithOwner",
        ])
        .output()
        .with_context(|| format!("could not run `{GH}`. Is the GitHub CLI installed?"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`{GH} repo view` failed. Name the repository with --repo owner/name.\n{}",
            stderr.trim()
        );
    }
    let spec = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Repo::parse(&spec).with_context(|| format!("`{GH} repo view` answered {spec:?}"))
}

/// The alias one number of the chain carries in the query.
///
/// A GraphQL alias cannot start with a digit, so the number gets a letter in
/// front of it.
fn alias(number: IssueNumber) -> String {
    format!("i{}", number.get())
}

/// Build the one query that asks about every number of the chain.
#[must_use]
pub fn build_query(numbers: &[IssueNumber]) -> String {
    let fields: String = numbers
        .iter()
        .map(|number| {
            format!(
                "    {}: issueOrPullRequest(number: {}) {{\n      \
                 __typename\n      \
                 ... on Issue {{ number title state stateReason }}\n      \
                 ... on PullRequest {{ number title state }}\n    }}\n",
                alias(*number),
                number.get()
            )
        })
        .collect();
    format!(
        "query($owner: String!, $name: String!) {{\n  \
         repository(owner: $owner, name: $name) {{\n{fields}  }}\n}}\n"
    )
}

/// Read the answer of the query back into one entry for each number, in the
/// order the numbers were asked about.
///
/// # Errors
///
/// Fails when the body is not JSON, when it carries no repository (a name
/// nobody can read, or a credential that cannot see it), when GitHub could
/// not answer for one number of the chain, or when GitHub gives a state this
/// tool does not know.
pub fn parse_response(body: &str, numbers: &[IssueNumber]) -> Result<Vec<Entry>> {
    let answer: Value = serde_json::from_str(body)
        .with_context(|| format!("GitHub answered with no JSON: {}", body.trim()))?;

    let Some(repository) = answer.pointer("/data/repository").filter(|v| !v.is_null()) else {
        bail!(
            "{}",
            errors_of(&answer).unwrap_or_else(|| "GitHub named no repository".to_string())
        );
    };

    let reasons = Reasons::read(&answer);
    numbers
        .iter()
        .map(|number| entry_of(repository, &reasons, *number))
        .collect()
}

/// The `type` GitHub gives for a number the repository does not have. Every
/// other type says GitHub could not answer for the number, which is a
/// different thing.
const NOT_FOUND: &str = "NOT_FOUND";

/// What to write for an error that carries neither a message nor a type.
const NO_REASON: &str = "it gave no reason";

/// Why GitHub answered `null` for an alias, read one time for the whole
/// chain.
///
/// An error that belongs to one field names that field in its `path`, which
/// is a list such as `["repository", "i999"]`. So this lookup is keyed by the
/// names in that path, and an entry with no path — or with a path that holds
/// no names — belongs to no alias and answers for no number.
///
/// The whole chain is one question and it gets one answer, so the list is
/// read one time rather than one time for each number of the chain.
struct Reasons<'a> {
    by_alias: HashMap<&'a str, &'a Value>,
}

impl<'a> Reasons<'a> {
    /// Read the top-level `errors` list of `answer`.
    ///
    /// The first error that names an alias is the reason for that alias.
    fn read(answer: &'a Value) -> Self {
        let mut by_alias: HashMap<&'a str, &'a Value> = HashMap::new();
        for error in answer
            .get("errors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(path) = error.get("path").and_then(Value::as_array) else {
                continue;
            };
            for name in path.iter().filter_map(Value::as_str) {
                by_alias.entry(name).or_insert(error);
            }
        }
        Self { by_alias }
    }

    /// What GitHub said about the alias of `number`, when what it said is not
    /// that the repository does not have the number.
    ///
    /// `None` for an alias with no error of its own, and for [`NOT_FOUND`],
    /// which is the answer for a number the repository does not have. The
    /// text is the message GitHub wrote, because that message is the only
    /// thing that says what to do next. It falls back to the type, and then
    /// to [`NO_REASON`], for an error that carries less than that.
    fn refusal(&self, number: IssueNumber) -> Option<&'a str> {
        let error = self.by_alias.get(alias(number).as_str()).copied()?;
        let kind = error
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind == NOT_FOUND {
            return None;
        }
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Some(match (message, kind) {
            ("", "") => NO_REASON,
            ("", kind) => kind,
            (message, _) => message,
        })
    }
}

/// Read one number out of the answer.
fn entry_of(repository: &Value, reasons: &Reasons<'_>, number: IssueNumber) -> Result<Entry> {
    // An alias GitHub could not resolve is null, and one it never carried is
    // absent. Two answers arrive in that shape, and the reason beside the
    // null parts them: a number the repository does not have, which is one
    // row of the output, and a number GitHub could not answer for, which
    // stops the run.
    let Some(node) = repository.get(alias(number)).filter(|v| !v.is_null()) else {
        if let Some(reason) = reasons.refusal(number) {
            bail!("GitHub could not answer for {number}: {reason}");
        }
        return Ok(Entry {
            number,
            title: String::new(),
            status: Status::Missing,
            closes: None,
        });
    };

    let state = node
        .get("state")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("GitHub gave {number} no state"))?;
    let kind = node.get("__typename").and_then(Value::as_str).unwrap_or("");
    let reason = node.get("stateReason").and_then(Value::as_str);

    Ok(Entry {
        number,
        title: node
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        status: status_of(state, kind, reason).ok_or_else(|| {
            anyhow!("GitHub gave {number} the state {state}, which wn cannot read")
        })?,
        closes: None,
    })
}

/// The `__typename` of a pull request, which is the one kind whose closed
/// state does not say the work was done.
const PULL_REQUEST: &str = "PullRequest";

/// What one state of GitHub means for a chain, or `None` for a state this
/// tool has never been taught.
///
/// A closed issue counts as done unless GitHub says the work was not done. A
/// closed pull request is the mirror: it counts as dropped unless it was
/// merged, and a merged one carries the state `MERGED` rather than `CLOSED`.
fn status_of(state: &str, kind: &str, reason: Option<&str>) -> Option<Status> {
    match state {
        "OPEN" => Some(Status::Open),
        "MERGED" => Some(Status::Done),
        "CLOSED" if kind == PULL_REQUEST => Some(Status::Dropped),
        "CLOSED" => Some(match reason {
            Some("NOT_PLANNED" | "DUPLICATE") => Status::Dropped,
            _ => Status::Done,
        }),
        _ => None,
    }
}

/// What GitHub said went wrong, as one line.
fn errors_of(answer: &Value) -> Option<String> {
    let errors = answer.get("errors")?.as_array()?;
    let messages: Vec<&str> = errors
        .iter()
        .filter_map(|error| error.get("message").and_then(Value::as_str))
        .collect();
    (!messages.is_empty()).then(|| messages.join(" "))
}

/// Ask GitHub about every number of the chain.
///
/// # Errors
///
/// Fails when `gh` cannot run, and when the answer holds no repository. A
/// number the repository does not have is [`Status::Missing`] rather than an
/// error, and a number GitHub could not answer for is an error rather than
/// [`Status::Missing`]. The reason beside the `null` answer parts the two.
pub fn fetch(repo: &Repo, numbers: &[IssueNumber]) -> Result<Vec<Entry>> {
    let query = build_query(numbers);
    let output = Command::new(GH)
        .arg("api")
        .arg("graphql")
        .arg("-f")
        .arg(format!("owner={}", repo.owner()))
        .arg("-f")
        .arg(format!("name={}", repo.name()))
        .arg("-f")
        .arg(format!("query={query}"))
        .output()
        .with_context(|| format!("could not run `{GH}`. Is the GitHub CLI installed?"))?;

    // The body carries every answer even when `gh` exits non-zero, which it
    // does for a number the repository does not have. So the answer is the
    // body whenever there is one, and the exit status speaks only for a run
    // that printed nothing at all.
    let body = String::from_utf8_lossy(&output.stdout);
    if body.trim().is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`{GH} api graphql` answered nothing: {}", stderr.trim());
    }
    parse_response(&body, numbers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(numbers: &[u64]) -> Vec<IssueNumber> {
        numbers
            .iter()
            .map(|n| IssueNumber::new(*n).expect("the test number is an issue number"))
            .collect()
    }

    fn statuses(entries: &[Entry]) -> Vec<(u64, Status)> {
        entries
            .iter()
            .map(|entry| (entry.number.get(), entry.status))
            .collect()
    }

    /// The message GitHub writes for a repository the credential may read in
    /// part and not in whole.
    const REFUSED_MESSAGE: &str = "Resource not accessible by integration";

    /// A body that answers `null` for #42, with `kind` as the reason beside
    /// the null.
    ///
    /// This is the shape of every per-field error: the alias answers `null`,
    /// and the entry in the top-level `errors` list names that alias in its
    /// `path`. Only the `type` says whether the repository has the number.
    fn refused_body(kind: &str) -> String {
        format!(
            r#"{{"data":{{"repository":{{"i42":null}}}},
               "errors":[{{"type":"{kind}","path":["repository","i42"],
               "message":"{REFUSED_MESSAGE}"}}]}}"#
        )
    }

    #[test]
    fn a_repository_is_two_parts_divided_by_one_slash() {
        let repo = Repo::parse("timmattison/tools").expect("that is a repository");
        assert_eq!(repo.owner(), "timmattison");
        assert_eq!(repo.name(), "tools");
        assert_eq!(repo.to_string(), "timmattison/tools");
    }

    #[test]
    fn a_repository_refuses_everything_that_is_not_two_parts() {
        for spec in ["tools", "timmattison/", "/tools", "", "a/b/c", "a//b"] {
            assert!(
                Repo::parse(spec).is_err(),
                "{spec:?} is not a repository name"
            );
        }
    }

    #[test]
    fn the_query_asks_about_every_number_once() {
        let query = build_query(&chain(&[277, 278]));
        assert!(
            query.contains("i277: issueOrPullRequest(number: 277)"),
            "the query asks about 277, in {query}"
        );
        assert!(
            query.contains("i278: issueOrPullRequest(number: 278)"),
            "the query asks about 278, in {query}"
        );
    }

    #[test]
    fn the_query_takes_the_repository_as_a_variable() {
        // The owner and the name arrive from a command line and from `gh`, so
        // they go in as variables rather than into the text of the query.
        let query = build_query(&chain(&[1]));
        assert!(
            query.contains("query($owner: String!, $name: String!)"),
            "the query declares its variables, in {query}"
        );
        assert!(
            query.contains("repository(owner: $owner, name: $name)"),
            "the query reads its variables, in {query}"
        );
    }

    #[test]
    fn the_query_asks_for_the_reason_a_closed_issue_was_closed() {
        let query = build_query(&chain(&[1]));
        assert!(
            query.contains("stateReason"),
            "the query asks why an issue closed, in {query}"
        );
    }

    #[test]
    fn reads_the_state_of_each_issue() {
        let body = r#"{"data":{"repository":{
            "i277":{"__typename":"Issue","number":277,"title":"First","state":"CLOSED","stateReason":"COMPLETED"},
            "i278":{"__typename":"Issue","number":278,"title":"Second","state":"OPEN","stateReason":null}
        }}}"#;
        let entries = parse_response(body, &chain(&[277, 278])).expect("that body is an answer");
        assert_eq!(
            statuses(&entries),
            vec![(277, Status::Done), (278, Status::Open)]
        );
        assert_eq!(entries[0].title, "First");
        assert_eq!(entries[1].title, "Second");
    }

    #[test]
    fn an_issue_closed_without_the_work_is_dropped_rather_than_done() {
        let body = r#"{"data":{"repository":{
            "i1":{"__typename":"Issue","number":1,"title":"One","state":"CLOSED","stateReason":"NOT_PLANNED"},
            "i2":{"__typename":"Issue","number":2,"title":"Two","state":"CLOSED","stateReason":"DUPLICATE"},
            "i3":{"__typename":"Issue","number":3,"title":"Three","state":"CLOSED","stateReason":"REOPENED"}
        }}}"#;
        let entries = parse_response(body, &chain(&[1, 2, 3])).expect("that body is an answer");
        assert_eq!(
            statuses(&entries),
            vec![
                (1, Status::Dropped),
                (2, Status::Dropped),
                // A reason this tool does not know still says the issue closed,
                // and a closed issue is not work to start.
                (3, Status::Done),
            ]
        );
    }

    #[test]
    fn a_merged_pull_request_is_done_and_a_closed_one_is_dropped() {
        let body = r#"{"data":{"repository":{
            "i1":{"__typename":"PullRequest","number":1,"title":"One","state":"MERGED"},
            "i2":{"__typename":"PullRequest","number":2,"title":"Two","state":"CLOSED"},
            "i3":{"__typename":"PullRequest","number":3,"title":"Three","state":"OPEN"}
        }}}"#;
        let entries = parse_response(body, &chain(&[1, 2, 3])).expect("that body is an answer");
        assert_eq!(
            statuses(&entries),
            vec![(1, Status::Done), (2, Status::Dropped), (3, Status::Open)]
        );
    }

    #[test]
    fn a_number_the_repository_does_not_have_is_missing_and_not_an_error() {
        // GraphQL answers the number with null and puts the reason in a
        // top-level errors list. Every other answer of the same body stands.
        let body = r#"{"data":{"repository":{
            "i277":{"__typename":"Issue","number":277,"title":"First","state":"OPEN","stateReason":null},
            "i999":null
        }},"errors":[{"type":"NOT_FOUND","path":["repository","i999"],"message":"Could not resolve to an issue or pull request with the number of 999."}]}"#;
        let entries = parse_response(body, &chain(&[277, 999])).expect("that body is an answer");
        assert_eq!(
            statuses(&entries),
            vec![(277, Status::Open), (999, Status::Missing)]
        );
        assert_eq!(entries[1].title, "");
    }

    #[test]
    fn an_alias_the_answer_never_carried_is_missing() {
        let body = r#"{"data":{"repository":{}}}"#;
        let entries = parse_response(body, &chain(&[1])).expect("that body is an answer");
        assert_eq!(statuses(&entries), vec![(1, Status::Missing)]);
    }

    #[test]
    fn a_number_github_could_not_answer_for_is_an_error_and_not_a_missing_number() {
        // GitHub writes the same null for a number it refuses to answer for
        // as for a number the repository does not have. Only the type beside
        // the null parts the two. A tool that reads the null alone reports a
        // number the repository does have as one it lacks, and the reader
        // then hunts for a typo they did not make.
        //
        // The last type is one this tool has never seen, because a type that
        // is not NOT_FOUND is a refusal whether this tool knows it or not.
        for kind in ["FORBIDDEN", "INTERNAL", "SERVICE_UNAVAILABLE", "RATIONED"] {
            let err = parse_response(&refused_body(kind), &chain(&[42]))
                .expect_err("GitHub could not answer for that number");
            assert!(
                err.to_string().contains(REFUSED_MESSAGE),
                "the error carries what GitHub said about {kind}, in {err:#}"
            );
        }
    }

    #[test]
    fn the_error_names_the_number_github_could_not_answer_for() {
        // A chain of six that fails has to say which of the six it failed on,
        // or the reader walks the whole chain again by hand.
        let body = r#"{"data":{"repository":{
            "i1":{"__typename":"Issue","number":1,"title":"One","state":"CLOSED","stateReason":"COMPLETED"},
            "i2":{"__typename":"Issue","number":2,"title":"Two","state":"CLOSED","stateReason":"COMPLETED"},
            "i3":{"__typename":"Issue","number":3,"title":"Three","state":"OPEN","stateReason":null},
            "i4":null,
            "i5":{"__typename":"Issue","number":5,"title":"Five","state":"OPEN","stateReason":null},
            "i6":{"__typename":"Issue","number":6,"title":"Six","state":"OPEN","stateReason":null}
        }},"errors":[{"type":"SERVICE_UNAVAILABLE","path":["repository","i4"],"message":"Something went wrong while executing your query."}]}"#;
        let err = parse_response(body, &chain(&[1, 2, 3, 4, 5, 6]))
            .expect_err("GitHub could not answer for one of the six");
        assert!(
            err.to_string().contains("#4"),
            "the error names the number, in {err:#}"
        );
    }

    #[test]
    fn a_number_github_could_not_answer_for_beats_a_number_the_repository_does_not_have() {
        // One body carries both: #999 is a typo, and GitHub refused to answer
        // for #42. The run fails, because a wrong answer is worse than no
        // answer.
        let body = r#"{"data":{"repository":{
            "i277":{"__typename":"Issue","number":277,"title":"First","state":"OPEN","stateReason":null},
            "i999":null,
            "i42":null
        }},"errors":[
            {"type":"NOT_FOUND","path":["repository","i999"],"message":"Could not resolve to an issue or pull request with the number of 999."},
            {"type":"FORBIDDEN","path":["repository","i42"],"message":"Resource not accessible by integration"}
        ]}"#;
        let err = parse_response(body, &chain(&[277, 999, 42]))
            .expect_err("GitHub could not answer for #42");
        assert!(
            err.to_string().contains(REFUSED_MESSAGE),
            "the error carries what GitHub said, in {err:#}"
        );
        assert!(
            err.to_string().contains("#42"),
            "the error names the number GitHub could not answer for, in {err:#}"
        );
    }

    #[test]
    fn a_null_answer_with_no_reason_beside_it_is_missing() {
        // The one entry of the errors list names another alias, so it says
        // nothing about #999. A null with no reason beside it stays a number
        // the repository does not have.
        let body = r#"{"data":{"repository":{"i999":null}},
            "errors":[{"type":"FORBIDDEN","path":["repository","i1"],
            "message":"Resource not accessible by integration"}]}"#;
        let entries = parse_response(body, &chain(&[999])).expect("that body is an answer");
        assert_eq!(statuses(&entries), vec![(999, Status::Missing)]);
    }

    #[test]
    fn an_error_that_names_no_alias_belongs_to_no_number() {
        // An entry with no path, and one whose path is not a list of names,
        // belong to no alias of the query. Reading either of them as the
        // reason for #999 would fail a run over a number that is a typo.
        let body = r#"{"data":{"repository":{"i999":null}},"errors":[
            {"type":"FORBIDDEN","message":"Resource not accessible by integration"},
            {"type":"INTERNAL","path":[1,2],"message":"Something went wrong."}
        ]}"#;
        let entries = parse_response(body, &chain(&[999])).expect("that body is an answer");
        assert_eq!(statuses(&entries), vec![(999, Status::Missing)]);
    }

    #[test]
    fn the_entries_come_back_in_the_order_they_were_asked_about() {
        // A JSON object holds no order, and the chain does.
        let body = r#"{"data":{"repository":{
            "i3":{"__typename":"Issue","number":3,"title":"Three","state":"OPEN","stateReason":null},
            "i1":{"__typename":"Issue","number":1,"title":"One","state":"OPEN","stateReason":null},
            "i2":{"__typename":"Issue","number":2,"title":"Two","state":"OPEN","stateReason":null}
        }}}"#;
        let entries = parse_response(body, &chain(&[1, 2, 3])).expect("that body is an answer");
        assert_eq!(
            entries.iter().map(|e| e.number.get()).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn a_body_with_no_repository_is_an_error() {
        // A repository nobody can read answers null, and reporting every issue
        // of the chain as missing would hide the one real problem.
        let body = r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","message":"Could not resolve to a Repository with the name 'timmattison/nope'."}]}"#;
        let err = parse_response(body, &chain(&[1])).expect_err("no repository is an error");
        assert!(
            err.to_string()
                .contains("Could not resolve to a Repository"),
            "the error carries what GitHub said, in {err:#}"
        );
    }

    #[test]
    fn a_body_that_is_not_json_is_an_error() {
        let err = parse_response("gh: command not found", &chain(&[1]))
            .expect_err("that body is not an answer");
        assert!(
            err.to_string().contains("gh: command not found"),
            "the error carries the body, in {err:#}"
        );
    }

    #[test]
    fn a_state_this_tool_does_not_know_is_an_error() {
        // A new state is a change this tool must be taught, and calling it
        // open or closed would answer the reader's question with a guess.
        let body = r#"{"data":{"repository":{
            "i1":{"__typename":"Issue","number":1,"title":"One","state":"HIBERNATING","stateReason":null}
        }}}"#;
        let err = parse_response(body, &chain(&[1])).expect_err("that state is unknown");
        assert!(
            err.to_string().contains("HIBERNATING"),
            "the error names the state, in {err:#}"
        );
    }
}
