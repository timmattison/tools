//! Finds the containers on every host.
//!
//! `drun` keeps no record of which host runs which container. The Docker
//! engines keep that record, and a local copy goes out of date when somebody
//! removes a container directly or a host restarts. So `drun` asks every host
//! each time.
//!
//! It lists the containers with `docker ps` and does not ask for one name with
//! `docker inspect`. `docker inspect` exits with status 1 both for "no such
//! container" and for "cannot connect to the host", and only the words of the
//! error tell the two apart. `docker ps` exits with status 0 and lists nothing
//! when a host has no such container.

use serde::Deserialize;

use crate::config::Config;
use crate::host::{self, Answer, ContextName, HostFailure};
use crate::table;

/// The label of every container that `drun run` starts.
pub const MANAGED_LABEL: &str = "drun.managed=true";

/// How many generated names `drun` tries before it stops.
const NAME_ATTEMPTS: usize = 20;

/// One container, as one row of `docker ps --format '{{json .}}'` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Container {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Names")]
    names: String,
    #[serde(rename = "Image", default)]
    image: String,
    #[serde(rename = "Status", default)]
    status: String,
    #[serde(rename = "Ports", default)]
    ports: String,
}

/// Which containers to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Only the containers that `drun run` started.
    Managed,
    /// Every container.
    All,
}

/// The containers of all hosts.
#[derive(Debug)]
pub struct Inventory {
    answers: Vec<Answer<Vec<Container>>>,
}

/// Lists the containers in `scope` on every host in `config`, all at the
/// same time. The list includes containers that stopped.
pub fn inventory(config: &Config, scope: Scope) -> Inventory {
    let label_filter = format!("label={MANAGED_LABEL}");
    let mut args = vec!["ps", "--all", "--no-trunc", "--format", "{{json .}}"];
    if scope == Scope::Managed {
        args.extend(["--filter", label_filter.as_str()]);
    }
    let answers = host::ask_all(config.contexts(), |context| {
        let text = host::ask(context, &args, config.host_timeout())?;
        parse_rows(&text).map_err(HostFailure::Unreadable)
    });
    Inventory { answers }
}

/// Reads the rows of `docker ps --format '{{json .}}'`, one JSON object on
/// each line.
///
/// # Errors
///
/// Returns words that say which line is not a container.
fn parse_rows(text: &str) -> Result<Vec<Container>, String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| {
                format!(
                    "\"{}\" is not a container: {error}",
                    host::last_line(line, 80)
                )
            })
        })
        .collect()
}

impl Container {
    /// The names of the container, without the `/` that Docker puts first.
    fn names(&self) -> impl Iterator<Item = &str> {
        self.names
            .split(',')
            .map(|name| name.trim().trim_start_matches('/'))
            .filter(|name| !name.is_empty())
    }
}

impl Inventory {
    /// The hosts that gave no usable answer, and why.
    pub fn failures(&self) -> impl Iterator<Item = (&ContextName, &HostFailure)> {
        self.answers
            .iter()
            .filter_map(|answer| match &answer.result {
                Ok(_) => None,
                Err(failure) => Some((&answer.context, failure)),
            })
    }

    /// The hosts that have a container named `name`, in the order of the
    /// configuration.
    pub fn hosts_with_name(&self, name: &str) -> Vec<&ContextName> {
        // Docker makes a name unique on one host, so no host is in the list
        // two times.
        self.containers_where(|container| container.names().any(|found| found == name))
            .into_iter()
            .map(|(context, _)| context)
            .collect()
    }

    /// Each container that `matches`, with its host, in the order of the
    /// configuration and then in the order of `docker ps`.
    fn containers_where(
        &self,
        matches: impl Fn(&Container) -> bool,
    ) -> Vec<(&ContextName, &Container)> {
        self.answers
            .iter()
            .filter_map(|answer| {
                answer
                    .result
                    .as_ref()
                    .ok()
                    .map(|containers| (&answer.context, containers))
            })
            .flat_map(|(context, containers)| {
                containers
                    .iter()
                    .filter(|container| matches(container))
                    .map(move |container| (context, container))
            })
            .collect()
    }

    /// A container name that no host that answered uses, in the form
    /// `adjective-noun-1234`.
    ///
    /// It is `None` only when every name that `drun` tried is in use.
    pub fn fresh_name(&self) -> Option<String> {
        let candidates = names::Generator::with_naming(names::Name::Numbered).take(NAME_ATTEMPTS);
        first_free(candidates, |name| !self.hosts_with_name(name).is_empty())
    }
}

/// The `docker inspect` format that gives the state and the exit status of a
/// container, as in `exited 0`.
pub const STATE_FORMAT: &str = "{{.State.Status}} {{.State.ExitCode}}";

/// Why `drun` cannot find the one host that has a container.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum LocateError {
    /// No host that answered has the container.
    #[error("no host has a container named {reference}{}", describe_unreachable(.unreachable))]
    NotFound {
        /// The name or the ID that `drun` looked for.
        reference: String,
        /// The hosts that did not answer, and why.
        unreachable: Vec<(ContextName, String)>,
    },
    /// More than one container matches, on one host or on more than one.
    #[error(
        "{reference} matches more than one container: {}. Give more of the ID, or use \
         docker --context HOST",
        join_matches(.matches)
    )]
    Ambiguous {
        /// The name or the ID that `drun` looked for.
        reference: String,
        /// Each container that matches, with its host.
        matches: Vec<Match>,
    },
}

/// The characters of a container ID that a message shows, as `docker ps`
/// does.
const SHORT_ID_LENGTH: usize = 12;

/// One container that a reference matches, for an error message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// The host that has the container.
    pub host: ContextName,
    /// The first name of the container.
    pub name: String,
    /// The first 12 characters of the ID of the container.
    pub short_id: String,
}

impl Match {
    /// The match for `container` on `host`.
    fn new(host: &ContextName, container: &Container) -> Self {
        Self {
            host: host.clone(),
            name: container.names().next().unwrap_or_default().to_owned(),
            short_id: container.id.chars().take(SHORT_ID_LENGTH).collect(),
        }
    }
}

impl std::fmt::Display for Match {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} ({}) on {}",
            self.name, self.short_id, self.host
        )
    }
}

/// Words for the hosts that did not answer, for the end of an error message.
fn describe_unreachable(unreachable: &[(ContextName, String)]) -> String {
    if unreachable.is_empty() {
        return String::new();
    }
    let hosts: Vec<String> = unreachable
        .iter()
        .map(|(context, why)| format!("{context} ({why})"))
        .collect();
    format!(
        ". These hosts did not answer, so drun cannot tell if they have it: {}",
        hosts.join(", ")
    )
}

/// The matches, separated by commas.
fn join_matches(matches: &[Match]) -> String {
    let words: Vec<String> = matches.iter().map(Match::to_string).collect();
    words.join(", ")
}

/// `name` without one leading `/`, and `name` itself when it has none.
///
/// Docker accepts one leading `/` in a container name, as in `--name /job-1`,
/// and records the name without it. `drun` compares and prints the recorded
/// form.
#[must_use]
pub fn bare_name(name: &str) -> &str {
    name.strip_prefix('/').unwrap_or(name)
}

impl Inventory {
    /// The host of the one container that `reference` names.
    ///
    /// `reference` is a name or an ID, as for docker. A container whose name
    /// or full ID is `reference` matches first. When no container matches
    /// that way, a container whose ID starts with `reference` matches.
    /// `reference` must match exactly one container on all hosts together,
    /// so docker never has to choose between two containers on one host.
    ///
    /// A `reference` that starts with `/` is a name, as for docker. It
    /// matches the container whose name is `reference` without that one
    /// slash, and it matches no ID and no start of an ID.
    ///
    /// # Errors
    ///
    /// Returns [`LocateError::NotFound`] when no container on a host that
    /// answered matches, and [`LocateError::Ambiguous`] when more than one
    /// container matches, on one host or on more than one.
    pub fn locate(&self, reference: &str) -> Result<&ContextName, LocateError> {
        let found = if reference.starts_with('/') {
            let name = bare_name(reference);
            self.containers_where(|container| container.names().any(|held| held == name))
        } else {
            let exact = self.containers_where(|container| {
                container.id == reference || container.names().any(|name| name == reference)
            });
            if exact.is_empty() && is_id_start(reference) {
                self.containers_where(|container| container.id.starts_with(reference))
            } else {
                exact
            }
        };
        match found.as_slice() {
            [(host, _)] => Ok(host),
            [] => Err(LocateError::NotFound {
                reference: reference.to_owned(),
                unreachable: self
                    .failures()
                    .map(|(context, failure)| (context.clone(), failure.to_string()))
                    .collect(),
            }),
            many => Err(LocateError::Ambiguous {
                reference: reference.to_owned(),
                matches: many
                    .iter()
                    .map(|(host, container)| Match::new(host, container))
                    .collect(),
            }),
        }
    }

    /// A table of the containers of every host that answered, with the host
    /// in the first column.
    pub fn table(&self) -> String {
        let rows: Vec<Vec<String>> = self
            .answers
            .iter()
            .filter_map(|answer| {
                answer
                    .result
                    .as_ref()
                    .ok()
                    .map(|containers| (&answer.context, containers))
            })
            .flat_map(|(context, containers)| {
                containers.iter().map(move |container| {
                    vec![
                        context.to_string(),
                        container.names().next().unwrap_or_default().to_owned(),
                        container.image.clone(),
                        container.status.clone(),
                        container.ports.clone(),
                    ]
                })
            })
            .collect();
        table::render(&["HOST", "NAME", "IMAGE", "STATUS", "PORTS"], &rows)
    }
}

/// True when `reference` can be the start of a container ID, which is
/// hexadecimal.
fn is_id_start(reference: &str) -> bool {
    !reference.is_empty()
        && reference
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

/// Words for the answer of `docker inspect --format STATE_FORMAT`: the state,
/// and the exit status when the container stopped, as in `exited 0` or
/// `running`.
///
/// It is `None` when `answer` is not in that form.
pub fn describe_state(answer: &str) -> Option<String> {
    let mut words = answer.split_whitespace();
    let state = words.next()?;
    let code = words.next()?;
    if words.next().is_some() {
        return None;
    }
    if matches!(state, "exited" | "dead") {
        Some(format!("{state} {code}"))
    } else {
        Some(state.to_owned())
    }
}

/// The first of `candidates` that `taken` does not refuse.
fn first_free(
    candidates: impl IntoIterator<Item = String>,
    taken: impl Fn(&str) -> bool,
) -> Option<String> {
    candidates.into_iter().find(|candidate| !taken(candidate))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One row of `docker ps --format '{{json .}}'`.
    pub(crate) fn row(id: &str, name: &str, image: &str, status: &str) -> String {
        format!(
            "{{\"Command\":\"\\\"sh\\\"\",\"ID\":\"{id}\",\"Image\":\"{image}\",\
             \"Labels\":\"{MANAGED_LABEL}\",\"Names\":\"{name}\",\"Ports\":\"\",\
             \"State\":\"running\",\"Status\":\"{status}\"}}"
        )
    }

    pub(crate) fn container(id: &str, names: &str) -> Container {
        Container {
            id: id.to_owned(),
            names: names.to_owned(),
            image: "alpine".to_owned(),
            status: "Up 5 minutes".to_owned(),
            ports: String::new(),
        }
    }

    pub(crate) fn answer(name: &str, containers: Vec<Container>) -> Answer<Vec<Container>> {
        Answer {
            context: ContextName::parse(name).unwrap(),
            result: Ok(containers),
        }
    }

    #[test]
    fn rows_of_docker_ps_become_containers() {
        let text = format!(
            "{}\n{}\n",
            row("3f2a", "job-1", "alpine", "Up 5 minutes"),
            row("9c1d", "job-2", "nginx:1.27", "Exited (0) 2 hours ago")
        );
        let containers = parse_rows(&text).unwrap();
        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].id, "3f2a");
        assert_eq!(containers[1].image, "nginx:1.27");
        assert_eq!(containers[1].status, "Exited (0) 2 hours ago");
    }

    #[test]
    fn no_rows_is_no_containers() {
        assert_eq!(parse_rows(""), Ok(Vec::new()));
        assert_eq!(parse_rows("\n"), Ok(Vec::new()));
    }

    #[test]
    fn a_row_that_is_not_a_container_is_refused() {
        let error = parse_rows("Cannot connect\n").unwrap_err();
        assert!(error.contains("Cannot connect"), "{error}");
    }

    #[test]
    fn a_name_is_found_on_every_host_that_has_it() {
        let inventory = Inventory {
            answers: vec![
                answer(
                    "box-a",
                    vec![container("1", "job-1"), container("2", "web")],
                ),
                answer("box-b", vec![container("3", "other")]),
                answer("box-c", vec![container("4", "/legacy,job-1")]),
            ],
        };
        let hosts: Vec<&str> = inventory
            .hosts_with_name("job-1")
            .into_iter()
            .map(ContextName::as_str)
            .collect();
        assert_eq!(hosts, ["box-a", "box-c"]);
        assert!(inventory.hosts_with_name("job").is_empty());
    }

    #[test]
    fn the_first_free_candidate_is_the_name() {
        let candidates = ["a-b-1", "a-b-2", "a-b-3"].map(str::to_owned);
        assert_eq!(
            first_free(candidates.clone(), |name| name != "a-b-3"),
            Some("a-b-3".to_owned())
        );
        assert_eq!(first_free(candidates, |_| true), None);
    }

    #[test]
    fn a_fresh_name_is_not_in_use_and_has_the_numbered_form() {
        let inventory = Inventory {
            answers: vec![answer("box-a", vec![container("1", "job-1")])],
        };
        let name = inventory.fresh_name().unwrap();
        assert_numbered_form(&name);
        assert!(inventory.hosts_with_name(&name).is_empty());
    }

    /// Asserts that `name` has the numbered form of a fresh name.
    ///
    /// Counting the parts of a split on `-` is wrong here. Some words of the
    /// `names` crate contain `-`, for example `bite-sized` and `good-bye`.
    fn assert_numbered_form(name: &str) {
        assert_eq!(name.split('-').count(), 3, "{name}");
    }

    #[test]
    fn the_numbered_form_allows_a_word_that_contains_a_hyphen() {
        assert_numbered_form("bite-sized-goose-0042");
        assert_numbered_form("ancient-good-bye-1234");
    }

    #[test]
    #[should_panic(expected = "goose-0042")]
    fn the_numbered_form_rejects_a_name_without_an_adjective() {
        assert_numbered_form("goose-0042");
    }

    #[test]
    #[should_panic(expected = "bite-sized-goose-42")]
    fn the_numbered_form_rejects_a_number_of_two_digits() {
        assert_numbered_form("bite-sized-goose-42");
    }

    #[test]
    #[should_panic(expected = "bite-sized-goose-12a4")]
    fn the_numbered_form_rejects_a_number_with_a_letter() {
        assert_numbered_form("bite-sized-goose-12a4");
    }

    fn two_hosts() -> Inventory {
        Inventory {
            answers: vec![
                answer(
                    "box-a",
                    vec![container("aaa111", "job-a"), container("abc999", "web")],
                ),
                answer("box-b", vec![container("bbb111", "job-b")]),
            ],
        }
    }

    fn located(inventory: &Inventory, reference: &str) -> Result<String, LocateError> {
        inventory
            .locate(reference)
            .map(|context| context.as_str().to_owned())
    }

    #[test]
    fn a_container_is_located_by_its_name() {
        let inventory = two_hosts();
        assert_eq!(located(&inventory, "job-a"), Ok("box-a".to_owned()));
        assert_eq!(located(&inventory, "job-b"), Ok("box-b".to_owned()));
    }

    #[test]
    fn a_container_is_located_by_its_id_or_the_start_of_its_id() {
        let inventory = two_hosts();
        assert_eq!(located(&inventory, "bbb111"), Ok("box-b".to_owned()));
        assert_eq!(located(&inventory, "bbb"), Ok("box-b".to_owned()));
    }

    #[test]
    fn a_name_wins_over_the_start_of_an_id_on_another_host() {
        let inventory = Inventory {
            answers: vec![
                answer("box-a", vec![container("fff000", "abc")]),
                answer("box-b", vec![container("abc123", "job-b")]),
            ],
        };
        assert_eq!(located(&inventory, "abc"), Ok("box-a".to_owned()));
    }

    #[test]
    fn a_name_with_one_leading_slash_locates_the_container_as_docker_does() {
        let inventory = two_hosts();
        assert_eq!(located(&inventory, "/job-a"), Ok("box-a".to_owned()));
        assert_eq!(located(&inventory, "/job-b"), Ok("box-b".to_owned()));
        let multi_byte = Inventory {
            answers: vec![answer("box-a", vec![container("aaa111", "café-日本語")])],
        };
        assert_eq!(located(&multi_byte, "/café-日本語"), Ok("box-a".to_owned()));
    }

    #[test]
    fn a_reference_with_a_leading_slash_never_matches_an_id() {
        let inventory = two_hosts();
        for reference in ["/bbb", "/bbb111", "/aaa111"] {
            assert!(
                matches!(
                    inventory.locate(reference),
                    Err(LocateError::NotFound { .. })
                ),
                "{reference}"
            );
        }
    }

    #[test]
    fn a_hexadecimal_name_with_a_leading_slash_beats_an_id_that_starts_with_it() {
        let inventory = Inventory {
            answers: vec![
                answer("box-a", vec![container("fff000", "abc")]),
                answer("box-b", vec![container("abc123", "job-b")]),
            ],
        };
        assert_eq!(located(&inventory, "/abc"), Ok("box-a".to_owned()));
    }

    #[test]
    fn a_reference_with_two_leading_slashes_matches_no_name() {
        let error = two_hosts().locate("//job-a").unwrap_err();
        assert_eq!(
            error,
            LocateError::NotFound {
                reference: "//job-a".to_owned(),
                unreachable: Vec::new()
            }
        );
    }

    #[test]
    fn a_container_on_no_host_is_not_found() {
        let error = two_hosts().locate("ghost").unwrap_err();
        assert_eq!(
            error,
            LocateError::NotFound {
                reference: "ghost".to_owned(),
                unreachable: Vec::new()
            }
        );
        assert_eq!(error.to_string(), "no host has a container named ghost");
    }

    #[test]
    fn not_found_names_the_hosts_that_did_not_answer() {
        let mut inventory = two_hosts();
        inventory.answers.push(Answer {
            context: ContextName::parse("box-c").unwrap(),
            result: Err(HostFailure::TimedOut(std::time::Duration::from_secs(10))),
        });
        let message = inventory.locate("ghost").unwrap_err().to_string();
        assert!(message.contains("box-c (no answer in 10 s)"), "{message}");
    }

    /// The message of the refusal when `reference` matches more than one
    /// container.
    fn ambiguity(inventory: &Inventory, reference: &str) -> String {
        let error = inventory
            .locate(reference)
            .expect_err("the reference matches more than one container");
        assert!(matches!(error, LocateError::Ambiguous { .. }), "{error:?}");
        error.to_string()
    }

    #[test]
    fn a_name_on_two_hosts_is_ambiguous_and_each_container_is_named() {
        let inventory = Inventory {
            answers: vec![
                answer("box-a", vec![container("aaa111", "job-1")]),
                answer("box-b", vec![container("bbb111", "job-1")]),
            ],
        };
        let message = ambiguity(&inventory, "job-1");
        assert!(message.contains("job-1 (aaa111) on box-a"), "{message}");
        assert!(message.contains("job-1 (bbb111) on box-b"), "{message}");
    }

    #[test]
    fn the_start_of_two_ids_on_one_host_is_ambiguous() {
        let inventory = Inventory {
            answers: vec![answer(
                "box-a",
                vec![container("abc111", "one"), container("abc222", "two")],
            )],
        };
        let message = ambiguity(&inventory, "abc");
        assert!(message.contains("one (abc111) on box-a"), "{message}");
        assert!(message.contains("two (abc222) on box-a"), "{message}");
    }

    #[test]
    fn the_start_of_an_id_on_each_of_two_hosts_is_ambiguous() {
        let inventory = Inventory {
            answers: vec![
                answer("box-a", vec![container("abc111", "one")]),
                answer("box-b", vec![container("abc999", "two")]),
            ],
        };
        let message = ambiguity(&inventory, "abc");
        assert!(message.contains("one (abc111) on box-a"), "{message}");
        assert!(message.contains("two (abc999) on box-b"), "{message}");
    }

    #[test]
    fn an_ambiguity_shows_the_first_12_characters_of_each_id() {
        let inventory = Inventory {
            answers: vec![answer(
                "box-a",
                vec![
                    container("abc111222333444555", "one"),
                    container("abc999888777666555", "two"),
                ],
            )],
        };
        let message = ambiguity(&inventory, "abc");
        assert!(message.contains("one (abc111222333) on box-a"), "{message}");
        assert!(!message.contains("abc111222333444"), "{message}");
    }

    #[test]
    fn the_table_shows_the_containers_of_every_host_with_the_host() {
        let mut inventory = two_hosts();
        inventory.answers.push(Answer {
            context: ContextName::parse("box-c").unwrap(),
            result: Err(HostFailure::TimedOut(std::time::Duration::from_secs(10))),
        });
        let table = inventory.table();
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 4, "{table}");
        assert!(
            lines[0].starts_with("HOST") && lines[0].contains("NAME"),
            "{table}"
        );
        assert!(
            lines[1].starts_with("box-a") && lines[1].contains("job-a"),
            "{table}"
        );
        assert!(
            lines[2].starts_with("box-a") && lines[2].contains("web"),
            "{table}"
        );
        assert!(
            lines[3].starts_with("box-b") && lines[3].contains("job-b"),
            "{table}"
        );
    }

    #[test]
    fn the_state_shows_the_exit_status_only_when_the_container_stopped() {
        assert_eq!(describe_state("exited 0\n"), Some("exited 0".to_owned()));
        assert_eq!(describe_state("dead 137"), Some("dead 137".to_owned()));
        assert_eq!(describe_state("running 0"), Some("running".to_owned()));
        assert_eq!(describe_state("created 0"), Some("created".to_owned()));
        assert_eq!(describe_state(""), None);
        assert_eq!(describe_state("exited"), None);
    }
}
