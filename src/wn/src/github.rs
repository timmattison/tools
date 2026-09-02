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
//! # A missing number is an answer, not a failure
//!
//! GraphQL answers a number the repository does not have with `null` for that
//! alias and an entry in a top-level `errors` list. `gh` reads that list and
//! exits non-zero. The body on standard output still carries every other
//! answer, so this module reads the body first and reads the exit status only
//! when the body holds nothing to use. One typo in a chain of six thus costs
//! one row of the output rather than the whole run.

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
        let _ = spec;
        bail!("not implemented")
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
        .args(["repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"])
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
    let _ = numbers;
    String::new()
}

/// Read the answer of the query back into one entry for each number, in the
/// order the numbers were asked about.
///
/// # Errors
///
/// Fails when the body is not JSON, when it carries no repository (a name
/// nobody can read, or a credential that cannot see it), or when GitHub gives
/// a state this tool does not know.
pub fn parse_response(body: &str, numbers: &[IssueNumber]) -> Result<Vec<Entry>> {
    let _ = (body, numbers);
    Ok(Vec::new())
}

/// Ask GitHub about every number of the chain.
///
/// # Errors
///
/// Fails when `gh` cannot run, and when the answer holds no repository. A
/// number the repository does not have is [`Status::Missing`] rather than an
/// error.
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

    let body = String::from_utf8_lossy(&output.stdout);
    // The body carries every answer even when `gh` exits non-zero, which it
    // does for a number the repository does not have. So the body is read
    // first, and the exit status is only read when the body is unusable.
    parse_response(&body, numbers).map_err(|err| {
        if output.status.success() {
            err
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow!("`{GH} api graphql` failed: {}", stderr.trim())
        }
    })
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
            err.to_string().contains("Could not resolve to a Repository"),
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
