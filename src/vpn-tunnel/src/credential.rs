use op_cache::ItemField;
use std::io;
use std::process::Output;

/// A running container and the WireGuard key in its environment.
///
/// Any container that carries `WIREGUARD_PRIVATE_KEY` is a holder, whatever
/// its image.
#[derive(Debug, Clone)]
pub struct RunningTunnel {
    pub container_name: String,
    pub wireguard_key: String,
}

/// The result of credential selection.
#[derive(Debug, Clone)]
pub struct SelectedCredential {
    /// The field label (e.g., "credential-2")
    pub field_label: String,
    /// The WireGuard private key value
    pub key: String,
    /// Total number of available credentials
    pub total: usize,
    /// Number of credentials held by a running tunnel
    pub in_use: usize,
    /// How the selection came to this credential
    pub selection: Selection,
}

/// How [`select_credential`] came to the credential it selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// The first credential whose key no running tunnel holds.
    FirstFree,
    /// The credential that the existing `.env` names.
    Reused,
    /// The credential whose key the running tunnel of this directory holds,
    /// because the existing `.env` names no credential. A directory that was
    /// generated before the `CREDENTIAL_FIELD` line existed is such a
    /// directory. A label that no credential carries any more is another.
    OwnTunnel {
        /// The name of the running tunnel that holds the key. It is the name
        /// of the gluetun container that this run writes.
        holder: String,
    },
    /// The first free credential, because a running tunnel of another name
    /// holds the key of the credential that the existing `.env` names.
    Replaced {
        /// The label that the existing `.env` names.
        named_label: String,
        /// The name of the running tunnel that holds the key of that label.
        holder: String,
    },
}

/// A credential that a running container holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialInUse {
    pub field_label: String,
    pub container_name: String,
}

/// Error when a running tunnel holds every credential.
#[derive(Debug)]
pub struct AllCredentialsInUse {
    pub usage: Vec<CredentialInUse>,
}

/// Selects the credential a tunnel is generated from.
///
/// A directory that already holds a `.env` names the credential its tunnel was
/// generated from, and `existing_label` carries that name. `own_container` is
/// the name of the gluetun container that this run writes.
///
/// This function selects the named credential again, whole label against
/// whole label, when no running tunnel of another name holds its key. So the
/// credential stays when a tunnel of `own_container` holds its key. Docker
/// refuses two containers of one name, so that tunnel is the tunnel of this
/// directory, or a tunnel that cannot run beside it. A second run into the
/// directory therefore does not move its tunnel to another key.
///
/// A running tunnel of another name that holds the named key is not the tunnel
/// that this run writes. Two tunnels must not carry one key, so this function
/// does not keep the named credential. It uses the ordinary rule, and
/// [`Selection::Replaced`] names the holder.
///
/// No label can name a credential. That is the case when `existing_label` is
/// [`None`], and when no credential carries it any more because the credential
/// is no longer in 1Password. A directory that was generated before the
/// `CREDENTIAL_FIELD` line existed has no label. In this case, this function
/// keeps the credential whose key a running tunnel of `own_container` holds,
/// when no running tunnel of another name holds that key. The argument is the
/// same one: Docker refuses two containers of one name. So the tunnel of this
/// directory does not move to another key. [`Selection::OwnTunnel`] names the
/// tunnel. This rule does not apply when a credential carries
/// `existing_label`.
///
/// The ordinary rule selects the first credential whose key no running tunnel
/// holds. This function uses it when no other rule applies.
///
/// `total` counts the available credentials and `in_use` counts the ones a
/// running tunnel holds, under each rule.
///
/// # Errors
///
/// Returns `AllCredentialsInUse` when a running tunnel holds every credential
/// and no rule above keeps one. That happens when no credential carries
/// `existing_label` and no tunnel of `own_container` holds a credential alone,
/// or when a running tunnel of another name holds the named key.
pub fn select_credential(
    available: &[ItemField],
    running: &[RunningTunnel],
    existing_label: Option<&str>,
    own_container: &str,
) -> Result<SelectedCredential, AllCredentialsInUse> {
    // The credential that `existing_label` names, and a running tunnel of
    // another name that holds its key.
    let mut named: Option<(&ItemField, Option<&RunningTunnel>)> = None;
    let mut first_free: Option<&ItemField> = None;
    // The first credential whose key a tunnel of `own_container` holds, and
    // no tunnel of another name holds.
    let mut own_only: Option<&ItemField> = None;
    let mut in_use_count = 0_usize;

    for field in available {
        if named.is_none() && existing_label == Some(field.label.as_str()) {
            let other_holder = running
                .iter()
                .find(|r| r.wireguard_key == field.value && r.container_name != own_container);
            named = Some((field, other_holder));
        }
        if running.iter().any(|r| r.wireguard_key == field.value) {
            in_use_count += 1;
            let held_by_own = running
                .iter()
                .any(|r| r.wireguard_key == field.value && r.container_name == own_container);
            let held_by_other = running
                .iter()
                .any(|r| r.wireguard_key == field.value && r.container_name != own_container);
            if own_only.is_none() && held_by_own && !held_by_other {
                own_only = Some(field);
            }
        } else if first_free.is_none() {
            first_free = Some(field);
        }
    }

    let chosen = match (named, first_free) {
        (Some((field, None)), _) => Some((field, Selection::Reused)),
        (Some((named_field, Some(holder))), Some(free)) => Some((
            free,
            Selection::Replaced {
                named_label: named_field.label.clone(),
                holder: holder.container_name.clone(),
            },
        )),
        (None, _) if own_only.is_some() => own_only.map(|field| {
            (
                field,
                Selection::OwnTunnel {
                    holder: own_container.to_string(),
                },
            )
        }),
        (None, Some(free)) => Some((free, Selection::FirstFree)),
        (_, None) => None,
    };

    if let Some((field, selection)) = chosen {
        return Ok(SelectedCredential {
            field_label: field.label.clone(),
            key: field.value.clone(),
            total: available.len(),
            in_use: in_use_count,
            selection,
        });
    }

    // Every credential is held — build the usage list
    let usage: Vec<CredentialInUse> = available
        .iter()
        .map(|field| {
            let container = running
                .iter()
                .find(|r| r.wireguard_key == field.value)
                .map(|r| r.container_name.clone())
                .unwrap_or_else(|| "unknown".to_string());
            CredentialInUse {
                field_label: field.label.clone(),
                container_name: container,
            }
        })
        .collect();

    Err(AllCredentialsInUse { usage })
}

/// The environment variable that carries a container's WireGuard private key.
const WIREGUARD_KEY_ENV: &str = "WIREGUARD_PRIVATE_KEY=";

/// The docker template that reports one container per line: its name, a tab,
/// and its environment, both as JSON.
///
/// JSON escapes a tab and a newline inside a value. So the literal tab is a
/// safe separator and each container takes exactly one line.
const INSPECT_FORMAT: &str = "{{json .Name}}\t{{json .Config.Env}}";

/// A docker command that could not be run, or that ran and reported a failure.
#[derive(Debug)]
pub struct DockerError {
    /// The docker arguments that were run, joined with spaces.
    command: String,
    /// What docker wrote to stderr, or why the process could not start.
    detail: String,
}

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "docker {} failed: {}", self.command, self.detail)
    }
}

impl std::error::Error for DockerError {}

impl DockerError {
    fn new(args: &[&str], detail: String) -> Self {
        Self {
            command: args.join(" "),
            detail,
        }
    }
}

/// The WireGuard private key in a container's environment, if it carries one.
///
/// The key is the text after the first `=`. A trailing `=` of base64 padding
/// stays. A trailing carriage return does not.
fn wireguard_key_from_env(env: &[String]) -> Option<&str> {
    env.iter()
        .map(|entry| entry.trim_end_matches('\r'))
        .find_map(|entry| entry.strip_prefix(WIREGUARD_KEY_ENV))
}

/// Reads one line of the `docker inspect` answer: the name and the environment.
///
/// Docker prints the name with a leading `/`. This function removes it. A
/// container with no environment gives `None` for the environment. A line that
/// is not in the format gives `None`.
fn parse_inspect_line(line: &str) -> Option<(String, Option<Vec<String>>)> {
    let (name, env) = line.split_once('\t')?;
    let name: String = serde_json::from_str(name).ok()?;
    let env: Option<Vec<String>> = serde_json::from_str(env).ok()?;
    let name = match name.strip_prefix('/') {
        Some(rest) => rest.to_string(),
        None => name,
    };
    Some((name, env))
}

/// The line docker writes for each target of `docker inspect` that it cannot
/// find. Docker 29 writes it in lower case, and older clients capitalize it.
const NO_SUCH_OBJECT: &str = "error: no such object: ";

/// True when `stderr` holds at least one line, and each line says that docker
/// found no object for one of `ids`.
fn only_removed(stderr: &[u8], ids: &[&str]) -> bool {
    let stderr = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    !lines.is_empty()
        && lines.iter().all(|line| {
            line.get(..NO_SUCH_OBJECT.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(NO_SUCH_OBJECT))
                && line
                    .get(NO_SUCH_OBJECT.len()..)
                    .is_some_and(|id| ids.contains(&id))
        })
}

/// Runs one docker command and hands back its stdout, or the reason it failed.
///
/// A container in `may_be_removed` can stop and go away after `docker ps`
/// listed it. A failure that only reports such containers is a success, and
/// the stdout holds the other containers.
fn docker_stdout<R>(run: &R, args: &[&str], may_be_removed: &[&str]) -> Result<String, DockerError>
where
    R: Fn(&[&str]) -> io::Result<Output>,
{
    let output = run(args).map_err(|e| DockerError::new(args, e.to_string()))?;

    if !output.status.success() && !only_removed(&output.stderr, may_be_removed) {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("exited with status {}", output.status)
        } else {
            stderr
        };
        return Err(DockerError::new(args, detail));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Lists the running containers that hold a WireGuard key, and the key each
/// one holds, through the given docker runner.
///
/// The image of a container is not read. After a pull moves a tag, `docker ps`
/// shows the image ID in place of the image name, and a mirror or a renamed
/// image has another name. So one `docker inspect` reads the environment of
/// every running container, and the key decides.
fn find_running_tunnels_with<R>(run: R) -> Result<Vec<RunningTunnel>, DockerError>
where
    R: Fn(&[&str]) -> io::Result<Output>,
{
    let ps_stdout = docker_stdout(&run, &["ps", "-q", "--no-trunc"], &[])?;
    let ids: Vec<&str> = ps_stdout
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect();

    // `docker inspect` with no target fails, so ask nothing of it.
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut args = vec!["inspect", "--format", INSPECT_FORMAT];
    args.extend(&ids);
    let inspect_stdout = docker_stdout(&run, &args, &ids)?;

    let mut tunnels = Vec::new();
    for (index, line) in inspect_stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        // The line holds environment values, and a key is a secret. So the
        // error names the line and never shows it.
        let (name, env) = parse_inspect_line(line).ok_or_else(|| {
            DockerError::new(
                &args,
                format!("could not read line {} of the output", index + 1),
            )
        })?;
        if let Some(key) = env.as_deref().and_then(wireguard_key_from_env) {
            tunnels.push(RunningTunnel {
                container_name: name,
                wireguard_key: key.to_string(),
            });
        }
    }

    Ok(tunnels)
}

/// Lists the running containers that hold a WireGuard key, and that key.
///
/// A holder is any running container whose environment carries
/// `WIREGUARD_PRIVATE_KEY`, whatever its image.
///
/// # Errors
///
/// Returns `DockerError` when docker cannot be started, when a docker
/// command exits non-zero, or when a line of its answer cannot be read. An
/// inspect that fails only because a listed container is gone is not an
/// error, because that container holds no key. A
/// docker failure is never reported as an empty list: a credential handed out
/// on unknown state is worse than no credential.
pub fn find_running_tunnels() -> Result<Vec<RunningTunnel>, DockerError> {
    find_running_tunnels_with(|args| childenv::command("docker").args(args).output())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// An exit status of the given code, on either platform family.
    #[cfg(unix)]
    fn status(code: i32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        // A wait status carries the exit code in its second byte.
        std::process::ExitStatus::from_raw(code << 8)
    }

    /// An exit status of the given code, on either platform family.
    #[cfg(not(unix))]
    fn status(code: i32) -> std::process::ExitStatus {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(u32::try_from(code).unwrap_or(1))
    }

    fn ok_output(stdout: &str) -> Output {
        Output {
            status: status(0),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    fn failed_output(stderr: &[u8]) -> Output {
        Output {
            status: status(1),
            stdout: Vec::new(),
            stderr: stderr.to_vec(),
        }
    }

    #[test]
    fn the_wireguard_key_is_read_from_the_environment() {
        let env = env(&["PATH=/usr/bin", "WIREGUARD_PRIVATE_KEY=abc123", "TZ=UTC"]);
        assert_eq!(wireguard_key_from_env(&env), Some("abc123"));
    }

    #[test]
    fn a_container_without_a_wireguard_key_reads_as_none() {
        let env = env(&["PATH=/usr/bin", "VPN_TYPE=openvpn"]);
        assert_eq!(wireguard_key_from_env(&env), None);
    }

    #[test]
    fn a_base64_key_keeps_its_trailing_padding() {
        let env = env(&["WIREGUARD_PRIVATE_KEY=cGFkZGVkK2tleQ=="]);
        assert_eq!(wireguard_key_from_env(&env), Some("cGFkZGVkK2tleQ=="));
    }

    #[test]
    fn a_carriage_return_does_not_reach_the_key() {
        let env = env(&["WIREGUARD_PRIVATE_KEY=abc123\r"]);
        assert_eq!(wireguard_key_from_env(&env), Some("abc123"));
    }

    fn env(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|e| (*e).to_string()).collect()
    }

    /// A fake docker that holds the given containers: (ID, inspect line).
    /// It records each call.
    fn fake_docker<'a>(
        containers: &'a [(&'a str, &'a str)],
        calls: &'a RefCell<Vec<Vec<String>>>,
    ) -> impl Fn(&[&str]) -> io::Result<Output> + 'a {
        move |args| {
            calls
                .borrow_mut()
                .push(args.iter().map(|a| (*a).to_string()).collect());
            Ok(match args {
                ["ps", "-q", "--no-trunc"] => {
                    let mut out = String::new();
                    for (id, _) in containers {
                        out.push_str(id);
                        out.push('\n');
                    }
                    ok_output(&out)
                }
                ["inspect", "--format", format, targets @ ..] if *format == INSPECT_FORMAT => {
                    let mut out = String::new();
                    for target in targets {
                        for (id, line) in containers {
                            if id == target {
                                out.push_str(line);
                                out.push('\n');
                            }
                        }
                    }
                    ok_output(&out)
                }
                other => panic!("unexpected docker command: {other:?}"),
            })
        }
    }

    #[test]
    fn no_running_container_means_no_inspect() {
        let calls = RefCell::new(Vec::new());
        let tunnels =
            find_running_tunnels_with(fake_docker(&[], &calls)).expect("docker succeeded");

        assert!(tunnels.is_empty());
        let calls = calls.into_inner();
        assert_eq!(calls.len(), 1, "expected one docker command: {calls:?}");
        assert_eq!(calls[0].first().map(String::as_str), Some("ps"));
    }

    #[test]
    fn one_inspect_reads_every_container() {
        let calls = RefCell::new(Vec::new());
        let containers = [
            ("id-aaa", r#""/one"	["WIREGUARD_PRIVATE_KEY=k1"]"#),
            ("id-bbb", r#""/two"	["WIREGUARD_PRIVATE_KEY=k2"]"#),
            ("id-ccc", r#""/three"	["WIREGUARD_PRIVATE_KEY=k3"]"#),
        ];
        let tunnels =
            find_running_tunnels_with(fake_docker(&containers, &calls)).expect("docker succeeded");

        assert_eq!(tunnels.len(), 3, "{tunnels:?}");
        let calls = calls.into_inner();
        assert_eq!(calls.len(), 2, "expected one ps and one inspect: {calls:?}");
        assert_eq!(calls[0].first().map(String::as_str), Some("ps"));
        assert_eq!(calls[1].first().map(String::as_str), Some("inspect"));
        assert_eq!(
            calls[1].get(3..).unwrap_or_default(),
            ["id-aaa", "id-bbb", "id-ccc"],
            "the inspect targets must be every ID: {calls:?}"
        );
    }

    #[test]
    fn a_container_of_any_image_is_found_by_its_key() {
        // The image is never read, so a GHCR mirror needs no special case.
        let calls = RefCell::new(Vec::new());
        let containers = [(
            "id-mirror",
            r#""/mirror-gluetun"	["PATH=/usr/bin","WIREGUARD_PRIVATE_KEY=key-m"]"#,
        )];
        let tunnels =
            find_running_tunnels_with(fake_docker(&containers, &calls)).expect("docker succeeded");

        assert_eq!(tunnels.len(), 1, "{tunnels:?}");
        assert_eq!(tunnels[0].container_name, "mirror-gluetun");
        assert_eq!(tunnels[0].wireguard_key, "key-m");
    }

    #[test]
    fn a_container_without_a_wireguard_key_is_skipped() {
        let calls = RefCell::new(Vec::new());
        let containers = [
            ("id-db", r#""/db"	["POSTGRES_PASSWORD=x"]"#),
            ("id-ovpn", r#""/ovpn"	["VPN_TYPE=openvpn"]"#),
            ("id-none", r#""/no-env"	null"#),
            ("id-wg", r#""/wg-gluetun"	["WIREGUARD_PRIVATE_KEY=key-2"]"#),
        ];
        let tunnels =
            find_running_tunnels_with(fake_docker(&containers, &calls)).expect("docker succeeded");

        assert_eq!(tunnels.len(), 1, "{tunnels:?}");
        assert_eq!(tunnels[0].container_name, "wg-gluetun");
        assert_eq!(tunnels[0].wireguard_key, "key-2");
    }

    #[test]
    fn the_leading_slash_of_the_name_is_removed() {
        let (name, env) = parse_inspect_line(r#""/vpn-gluetun"	["A=b"]"#).expect("a valid line");
        assert_eq!(name, "vpn-gluetun");
        assert_eq!(env, Some(vec!["A=b".to_string()]));
    }

    #[test]
    fn a_line_that_cannot_be_read_is_an_error_that_hides_the_key() {
        let calls = RefCell::new(Vec::new());
        let containers = [
            ("id-ok", r#""/ok"	["A=b"]"#),
            (
                "id-bad",
                r#""/bad"	["WIREGUARD_PRIVATE_KEY=secret-key-value""#,
            ),
        ];
        let err = find_running_tunnels_with(fake_docker(&containers, &calls))
            .expect_err("an unreadable line must not be skipped");

        let text = err.to_string();
        assert!(
            text.contains("line 2"),
            "the error must name the line: {text}"
        );
        assert!(
            !text.contains("secret-key-value"),
            "the error must not show the key: {text}"
        );
    }

    #[test]
    fn a_failed_docker_ps_is_an_error_not_an_empty_list() {
        let err = find_running_tunnels_with(|_| {
            Ok(failed_output(
                b"Cannot connect to the Docker daemon at unix:///var/run/docker.sock",
            ))
        })
        .expect_err("a failed docker ps must not read as no tunnels running");

        assert!(
            err.to_string()
                .contains("Cannot connect to the Docker daemon"),
            "the error must carry the docker stderr: {err}"
        );
    }

    #[test]
    fn a_docker_that_cannot_be_started_is_an_error() {
        let err = find_running_tunnels_with(|_| {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "docker: no such file or directory",
            ))
        })
        .expect_err("a docker that cannot be started must not read as no tunnels running");

        assert!(
            err.to_string().contains("no such file or directory"),
            "the error must say why docker could not be started: {err}"
        );
    }

    #[test]
    fn a_failed_docker_inspect_is_an_error() {
        let err = find_running_tunnels_with(|args| {
            Ok(match args {
                ["ps", ..] => ok_output("3fbc63216742aaaa\n"),
                _ => failed_output(
                    b"permission denied while trying to connect to the Docker daemon socket",
                ),
            })
        })
        .expect_err("a failed docker inspect must not silently drop a container");

        assert!(
            err.to_string().contains("permission denied"),
            "the error must carry the docker stderr: {err}"
        );
    }

    /// A docker whose `ps` lists `id-gone` and `id-wg`, and whose inspect
    /// prints the line of `id-wg`, writes `stderr`, and exits 1. This is what
    /// docker does when `id-gone` is removed between the two calls.
    fn docker_that_lost_a_container(
        stderr: &'static str,
    ) -> impl Fn(&[&str]) -> io::Result<Output> {
        move |args| {
            Ok(match args {
                ["ps", ..] => ok_output("id-gone\nid-wg\n"),
                _ => Output {
                    status: status(1),
                    stdout: b"\"/wg-gluetun\"\t[\"WIREGUARD_PRIVATE_KEY=key-wg\"]\n".to_vec(),
                    stderr: stderr.as_bytes().to_vec(),
                },
            })
        }
    }

    #[test]
    fn a_container_removed_before_the_inspect_is_skipped() {
        // Docker 29 writes the error in lower case, and older clients do not.
        for stderr in [
            "error: no such object: id-gone\n",
            "Error: No such object: id-gone\n",
        ] {
            let tunnels = find_running_tunnels_with(docker_that_lost_a_container(stderr))
                .unwrap_or_else(|e| panic!("a removed container holds no key ({stderr:?}): {e}"));

            assert_eq!(tunnels.len(), 1, "{stderr:?}: {tunnels:?}");
            assert_eq!(tunnels[0].container_name, "wg-gluetun");
            assert_eq!(tunnels[0].wireguard_key, "key-wg");
        }
    }

    #[test]
    fn an_inspect_error_that_is_not_a_removed_listed_container_is_still_an_error() {
        for stderr in [
            "error: no such object: id-gone\npermission denied\n",
            "error: no such object: id-never-listed\n",
        ] {
            let result = find_running_tunnels_with(docker_that_lost_a_container(stderr));

            assert!(result.is_err(), "{stderr:?} must stay an error: {result:?}");
        }
    }

    #[test]
    fn stderr_that_is_not_utf8_still_reports_an_error() {
        let err =
            find_running_tunnels_with(|_| Ok(failed_output(&[0xff, 0xfe, b'b', b'o', b'o', b'm'])))
                .expect_err("undecodable stderr must not read as no tunnels running");

        assert!(
            err.to_string().contains("boom"),
            "the error must carry what could be decoded: {err}"
        );
    }

    fn field(label: &str, value: &str) -> ItemField {
        ItemField {
            label: label.to_string(),
            value: value.to_string(),
        }
    }

    fn tunnel(name: &str, key: &str) -> RunningTunnel {
        RunningTunnel {
            container_name: name.to_string(),
            wireguard_key: key.to_string(),
        }
    }

    #[test]
    fn single_credential_none_in_use() {
        let available = vec![field("credential", "key-1")];
        let running = vec![];
        let result = select_credential(&available, &running, None, "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.key, "key-1");
        assert_eq!(result.total, 1);
        assert_eq!(result.in_use, 0);
    }

    #[test]
    fn multiple_credentials_none_in_use_selects_first() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![];
        let result = select_credential(&available, &running, None, "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.key, "key-1");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 0);
    }

    #[test]
    fn multiple_credentials_first_in_use_selects_second() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![tunnel("scraper-gluetun", "key-1")];
        let result = select_credential(&available, &running, None, "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(result.key, "key-2");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 1);
    }

    #[test]
    fn all_credentials_in_use_returns_error() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![
            tunnel("scraper-gluetun", "key-1"),
            tunnel("vpn-gluetun", "key-2"),
        ];
        let err = select_credential(&available, &running, None, "c-gluetun").unwrap_err();
        assert_eq!(err.usage.len(), 2);
        assert_eq!(
            err.usage[0],
            CredentialInUse {
                field_label: "credential".to_string(),
                container_name: "scraper-gluetun".to_string(),
            }
        );
        assert_eq!(
            err.usage[1],
            CredentialInUse {
                field_label: "credential-2".to_string(),
                container_name: "vpn-gluetun".to_string(),
            }
        );
    }

    #[test]
    fn running_tunnel_with_unknown_key_does_not_block() {
        let available = vec![field("credential", "key-1")];
        let running = vec![tunnel("other-gluetun", "different-key")];
        let result = select_credential(&available, &running, None, "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.in_use, 0);
    }

    #[test]
    fn a_call_without_a_label_selects_by_the_first_free_rule() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![tunnel("scraper-gluetun", "key-1")];
        let result = select_credential(&available, &running, None, "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(result.selection, Selection::FirstFree);
    }

    #[test]
    fn the_label_an_existing_env_names_is_selected_over_the_first_free_one() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![];
        let result =
            select_credential(&available, &running, Some("credential-2"), "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(result.key, "key-2");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 0);
        assert_eq!(result.selection, Selection::Reused);
    }

    #[test]
    fn a_named_label_is_kept_while_its_own_tunnel_runs() {
        // A second run into a directory whose tunnel is up must not move that
        // tunnel to another key. The tunnel that holds the key carries the
        // name that this run writes. Docker refuses two containers of one
        // name, so the two can never run with one key at the same time.
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![tunnel("vpn-gluetun", "key-2")];
        let result =
            select_credential(&available, &running, Some("credential-2"), "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(result.key, "key-2");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 1);
        assert_eq!(result.selection, Selection::Reused);
    }

    #[test]
    fn a_named_label_that_its_own_tunnel_holds_never_reaches_the_all_in_use_error() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![
            tunnel("scraper-gluetun", "key-1"),
            tunnel("vpn-gluetun", "key-2"),
        ];
        let result =
            select_credential(&available, &running, Some("credential"), "scraper-gluetun").unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.key, "key-1");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 2);
        assert_eq!(result.selection, Selection::Reused);
    }

    #[test]
    fn a_named_label_held_by_a_tunnel_of_another_name_gives_way_to_a_free_one() {
        // generate ./a, generate ./b, up ./a, generate ./b again. Both .env
        // files name `credential`, and the tunnel of ./a now holds its key.
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![tunnel("a-gluetun", "key-1")];
        let result =
            select_credential(&available, &running, Some("credential"), "b-gluetun").unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(result.key, "key-2");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 1);
        assert_eq!(
            result.selection,
            Selection::Replaced {
                named_label: "credential".to_string(),
                holder: "a-gluetun".to_string(),
            }
        );
    }

    #[test]
    fn a_replacement_skips_every_held_key_and_names_the_holder_of_the_named_one() {
        // The first running tunnel does not hold the named key. A note that
        // names the first running tunnel therefore names the wrong one.
        let available = vec![
            field("credential", "key-1"),
            field("credential-2", "key-2"),
            field("credential-3", "key-3"),
        ];
        let running = vec![
            tunnel("scraper-gluetun", "key-2"),
            tunnel("a-gluetun", "key-1"),
        ];
        let result =
            select_credential(&available, &running, Some("credential"), "b-gluetun").unwrap();
        assert_eq!(result.field_label, "credential-3");
        assert_eq!(result.key, "key-3");
        assert_eq!(result.total, 3);
        assert_eq!(result.in_use, 2);
        assert_eq!(
            result.selection,
            Selection::Replaced {
                named_label: "credential".to_string(),
                holder: "a-gluetun".to_string(),
            }
        );
    }

    #[test]
    fn a_named_label_held_by_a_tunnel_of_another_name_reaches_the_all_in_use_error() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![
            tunnel("a-gluetun", "key-1"),
            tunnel("scraper-gluetun", "key-2"),
        ];
        let err = select_credential(&available, &running, Some("credential"), "b-gluetun")
            .expect_err("a tunnel of another name holds the named key and no credential is free");
        assert_eq!(
            err.usage,
            vec![
                CredentialInUse {
                    field_label: "credential".to_string(),
                    container_name: "a-gluetun".to_string(),
                },
                CredentialInUse {
                    field_label: "credential-2".to_string(),
                    container_name: "scraper-gluetun".to_string(),
                },
            ]
        );
    }

    #[test]
    fn a_named_label_held_by_tunnels_of_both_names_is_not_kept() {
        // A tunnel of this run's name holds the key, and a tunnel of another
        // name holds it too. The tunnel of another name decides. It comes
        // second, so a check of the first holder alone keeps the label.
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![tunnel("b-gluetun", "key-1"), tunnel("a-gluetun", "key-1")];
        let result =
            select_credential(&available, &running, Some("credential"), "b-gluetun").unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(result.key, "key-2");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 1);
        assert_eq!(
            result.selection,
            Selection::Replaced {
                named_label: "credential".to_string(),
                holder: "a-gluetun".to_string(),
            }
        );
    }

    #[test]
    fn a_label_no_credential_carries_any_more_falls_back_to_the_first_free_one() {
        // The credential was removed from 1Password since the directory was
        // generated. That is not a failure: pick a free one as usual.
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![tunnel("scraper-gluetun", "key-1")];
        let result =
            select_credential(&available, &running, Some("credential-9"), "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(result.key, "key-2");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 1);
        assert_eq!(result.selection, Selection::FirstFree);
    }

    #[test]
    fn a_missing_label_still_reaches_the_all_in_use_error() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![
            tunnel("scraper-gluetun", "key-1"),
            tunnel("vpn-gluetun", "key-2"),
        ];
        let err =
            select_credential(&available, &running, Some("credential-9"), "c-gluetun").unwrap_err();
        assert_eq!(err.usage.len(), 2);
    }

    #[test]
    fn a_directory_without_a_label_keeps_the_key_its_own_tunnel_holds() {
        let available = vec![field("credential", "key-1")];
        let running = vec![tunnel("vpn-gluetun", "key-1")];
        let result = select_credential(&available, &running, None, "vpn-gluetun")
            .expect("the own tunnel of the directory holds the only key");
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.key, "key-1");
        assert_eq!(result.total, 1);
        assert_eq!(result.in_use, 1);
        assert_eq!(
            result.selection,
            Selection::OwnTunnel {
                holder: "vpn-gluetun".to_string()
            }
        );
    }

    #[test]
    fn an_own_tunnel_on_the_second_key_stays_there_when_the_label_is_none() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![tunnel("vpn-gluetun", "key-2")];
        let result = select_credential(&available, &running, None, "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(result.key, "key-2");
        assert_eq!(result.in_use, 1);
        assert_eq!(
            result.selection,
            Selection::OwnTunnel {
                holder: "vpn-gluetun".to_string()
            }
        );
    }

    #[test]
    fn an_own_tunnel_stays_on_its_key_when_the_label_is_stale() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![tunnel("vpn-gluetun", "key-2")];
        let result =
            select_credential(&available, &running, Some("credential-9"), "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(
            result.selection,
            Selection::OwnTunnel {
                holder: "vpn-gluetun".to_string()
            }
        );
    }

    #[test]
    fn a_key_that_another_tunnel_also_holds_is_not_kept_for_the_own_tunnel() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![tunnel("vpn-gluetun", "key-1"), tunnel("a-gluetun", "key-1")];
        let result = select_credential(&available, &running, None, "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(result.selection, Selection::FirstFree);
    }

    #[test]
    fn a_label_that_a_credential_carries_beats_the_own_tunnel_rule() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![tunnel("vpn-gluetun", "key-2")];
        let result =
            select_credential(&available, &running, Some("credential"), "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.selection, Selection::Reused);
    }

    #[test]
    fn the_named_label_matches_a_whole_label_and_not_a_prefix_of_one() {
        let available = vec![
            field("credential-20", "key-20"),
            field("credential-2", "key-2"),
        ];
        let running = vec![];
        let result =
            select_credential(&available, &running, Some("credential-2"), "vpn-gluetun").unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(result.key, "key-2");
    }
}
