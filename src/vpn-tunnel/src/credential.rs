use op_cache::ItemField;
use std::io;
use std::process::{Command, Output};

/// A running gluetun container and its WireGuard key.
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
    /// Number of credentials currently in use
    pub in_use: usize,
}

/// A credential that is currently in use by a running container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialInUse {
    pub field_label: String,
    pub container_name: String,
}

/// Error when all credentials are in use.
#[derive(Debug)]
pub struct AllCredentialsInUse {
    pub usage: Vec<CredentialInUse>,
}

/// Selects the first unused credential from available fields.
///
/// Compares available credential values against keys used by running tunnels.
/// Returns the first credential whose value does not appear in any running tunnel.
///
/// # Errors
///
/// Returns `AllCredentialsInUse` if every credential is used by a running container.
pub fn select_credential(
    available: &[ItemField],
    running: &[RunningTunnel],
) -> Result<SelectedCredential, AllCredentialsInUse> {
    let mut first_free: Option<&ItemField> = None;
    let mut in_use_count = 0_usize;

    for field in available {
        if running.iter().any(|r| r.wireguard_key == field.value) {
            in_use_count += 1;
        } else if first_free.is_none() {
            first_free = Some(field);
        }
    }

    if let Some(field) = first_free {
        return Ok(SelectedCredential {
            field_label: field.label.clone(),
            key: field.value.clone(),
            total: available.len(),
            in_use: in_use_count,
        });
    }

    // All in use — build the usage list
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

/// The image repository every gluetun container runs.
const GLUETUN_IMAGE: &str = "qmcgaw/gluetun";

/// The environment variable that carries a container's WireGuard private key.
const WIREGUARD_KEY_ENV: &str = "WIREGUARD_PRIVATE_KEY=";

/// The docker template that reports one container per line, name then image.
const PS_FORMAT: &str = "{{.Names}}\t{{.Image}}";

/// The docker template that reports one environment entry per line.
const INSPECT_ENV_FORMAT: &str = "{{range .Config.Env}}{{println .}}{{end}}";

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

/// Whether a docker image reference names the gluetun repository.
///
/// The comparison is anchored at both ends of the repository name: a tag
/// (`:v3.40`, `:latest`), a digest (`@sha256:...`), or nothing at all may
/// follow it. So `evil/qmcgaw/gluetun` and `qmcgaw/gluetunnel` do not match,
/// and every tag of the real image does. Docker's own `ancestor=` filter
/// cannot do this: it resolves an untagged reference to `:latest`, which a
/// machine that only ever pulled a pinned tag does not have, and then matches
/// nothing while `docker ps` still exits 0.
fn is_gluetun_image(image: &str) -> bool {
    match image.strip_prefix(GLUETUN_IMAGE) {
        Some(rest) => rest.is_empty() || rest.starts_with(':') || rest.starts_with('@'),
        None => false,
    }
}

/// Names of the running containers whose image is a gluetun image.
///
/// Reads the stdout of `docker ps --format '{{.Names}}\t{{.Image}}'`. A blank
/// line, a row that carries no tab, and a row whose name or image is empty are
/// all skipped.
fn gluetun_container_names(ps_stdout: &str) -> Vec<&str> {
    ps_stdout
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(name, image)| (name.trim(), image.trim()))
        .filter(|(name, image)| !name.is_empty() && is_gluetun_image(image))
        .map(|(name, _)| name)
        .collect()
}

/// The WireGuard private key in a container's environment, if it carries one.
///
/// Reads the stdout of
/// `docker inspect --format '{{range .Config.Env}}{{println .}}{{end}}'`.
fn wireguard_key_from_env(inspect_stdout: &str) -> Option<&str> {
    inspect_stdout
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .find_map(|line| line.strip_prefix(WIREGUARD_KEY_ENV))
}

/// Runs one docker command and hands back its stdout, or the reason it failed.
fn docker_stdout<R>(run: &R, args: &[&str]) -> Result<String, DockerError>
where
    R: Fn(&[&str]) -> io::Result<Output>,
{
    let output = run(args).map_err(|e| DockerError::new(args, e.to_string()))?;

    if !output.status.success() {
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

/// Lists the running gluetun containers and the WireGuard key each one holds,
/// through the given docker runner.
fn find_running_tunnels_with<R>(run: R) -> Result<Vec<RunningTunnel>, DockerError>
where
    R: Fn(&[&str]) -> io::Result<Output>,
{
    let ps_stdout = docker_stdout(&run, &["ps", "--format", PS_FORMAT])?;
    let names: Vec<String> = gluetun_container_names(&ps_stdout)
        .into_iter()
        .map(ToString::to_string)
        .collect();

    let mut tunnels = Vec::new();
    for name in names {
        let env = docker_stdout(&run, &["inspect", "--format", INSPECT_ENV_FORMAT, &name])?;
        if let Some(key) = wireguard_key_from_env(&env) {
            tunnels.push(RunningTunnel {
                container_name: name,
                wireguard_key: key.to_string(),
            });
        }
    }

    Ok(tunnels)
}

/// Lists the running gluetun containers and the WireGuard key each one holds.
///
/// # Errors
///
/// Returns `DockerError` when docker cannot be started, or when a docker
/// command exits non-zero. A docker failure is never reported as an empty
/// list: a credential handed out on unknown state is worse than no credential.
pub fn find_running_tunnels() -> Result<Vec<RunningTunnel>, DockerError> {
    find_running_tunnels_with(|args| Command::new("docker").args(args).output())
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
    fn every_gluetun_tag_matches() {
        let ps = concat!(
            "pinned\tqmcgaw/gluetun:v3.40\n",
            "latest\tqmcgaw/gluetun:latest\n",
            "bare\tqmcgaw/gluetun\n",
            "digest\tqmcgaw/gluetun@sha256:0123456789abcdef\n",
        );
        assert_eq!(
            gluetun_container_names(ps),
            vec!["pinned", "latest", "bare", "digest"]
        );
    }

    #[test]
    fn an_image_that_is_not_gluetun_does_not_match() {
        let ps = concat!(
            "db\tpostgres:16\n",
            "impostor\tevil/qmcgaw/gluetun:v3.40\n",
            "lookalike\tqmcgaw/gluetunnel:v1\n",
        );
        assert_eq!(gluetun_container_names(ps), Vec::<&str>::new());
    }

    #[test]
    fn blank_and_malformed_rows_are_skipped() {
        let ps = concat!(
            "\n",
            "vpn-gluetun\tqmcgaw/gluetun:v3.40\n",
            "no-tab-row\n",
            "\tqmcgaw/gluetun:v3.40\n",
            "empty-image\t\n",
        );
        assert_eq!(gluetun_container_names(ps), vec!["vpn-gluetun"]);
    }

    #[test]
    fn the_wireguard_key_is_read_from_the_environment() {
        let env = "PATH=/usr/bin\nWIREGUARD_PRIVATE_KEY=abc123\nTZ=UTC\n";
        assert_eq!(wireguard_key_from_env(env), Some("abc123"));
    }

    #[test]
    fn a_container_without_a_wireguard_key_reads_as_none() {
        let env = "PATH=/usr/bin\nVPN_TYPE=openvpn\n";
        assert_eq!(wireguard_key_from_env(env), None);
    }

    #[test]
    fn a_base64_key_keeps_its_trailing_padding() {
        let env = "WIREGUARD_PRIVATE_KEY=cGFkZGVkK2tleQ==\n";
        assert_eq!(wireguard_key_from_env(env), Some("cGFkZGVkK2tleQ=="));
    }

    #[test]
    fn a_carriage_return_does_not_reach_the_key() {
        let env = "WIREGUARD_PRIVATE_KEY=abc123\r\n";
        assert_eq!(wireguard_key_from_env(env), Some("abc123"));
    }

    #[test]
    fn docker_ps_asks_for_names_and_images_without_an_ancestor_filter() {
        let calls: RefCell<Vec<Vec<String>>> = RefCell::new(Vec::new());
        let tunnels = find_running_tunnels_with(|args| {
            calls
                .borrow_mut()
                .push(args.iter().map(|a| (*a).to_string()).collect());
            Ok(ok_output(""))
        })
        .expect("docker succeeded");

        assert!(tunnels.is_empty());
        let calls = calls.into_inner();
        assert_eq!(calls.len(), 1, "expected one docker command: {calls:?}");
        assert_eq!(calls[0].first().map(String::as_str), Some("ps"));
        assert!(
            calls[0].iter().any(|a| a == "{{.Names}}\t{{.Image}}"),
            "docker ps must report the image of every container: {calls:?}"
        );
        assert!(
            !calls[0].iter().any(|a| a.starts_with("ancestor=")),
            "an ancestor filter resolves to :latest and matches nothing: {calls:?}"
        );
    }

    #[test]
    fn finds_gluetun_containers_and_their_keys() {
        let tunnels = find_running_tunnels_with(|args| {
            Ok(match args {
                ["ps", ..] => ok_output("vpn-gluetun\tqmcgaw/gluetun:v3.40\ndb\tpostgres:16\n"),
                ["inspect", .., "vpn-gluetun"] => {
                    ok_output("PATH=/usr/bin\nWIREGUARD_PRIVATE_KEY=key-1\n")
                }
                other => panic!("unexpected docker command: {other:?}"),
            })
        })
        .expect("docker succeeded");

        assert_eq!(tunnels.len(), 1, "{tunnels:?}");
        assert_eq!(tunnels[0].container_name, "vpn-gluetun");
        assert_eq!(tunnels[0].wireguard_key, "key-1");
    }

    #[test]
    fn a_gluetun_container_without_a_wireguard_key_is_skipped() {
        let tunnels = find_running_tunnels_with(|args| {
            Ok(match args {
                ["ps", ..] => ok_output(concat!(
                    "openvpn-gluetun\tqmcgaw/gluetun:v3.40\n",
                    "wg-gluetun\tqmcgaw/gluetun:v3.40\n",
                )),
                ["inspect", .., "openvpn-gluetun"] => ok_output("VPN_TYPE=openvpn\n"),
                ["inspect", .., "wg-gluetun"] => ok_output("WIREGUARD_PRIVATE_KEY=key-2\n"),
                other => panic!("unexpected docker command: {other:?}"),
            })
        })
        .expect("docker succeeded");

        assert_eq!(tunnels.len(), 1, "{tunnels:?}");
        assert_eq!(tunnels[0].container_name, "wg-gluetun");
        assert_eq!(tunnels[0].wireguard_key, "key-2");
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
                ["ps", ..] => ok_output("vpn-gluetun\tqmcgaw/gluetun:v3.40\n"),
                _ => failed_output(b"Error: No such object: vpn-gluetun"),
            })
        })
        .expect_err("a failed docker inspect must not silently drop a container");

        assert!(
            err.to_string().contains("No such object: vpn-gluetun"),
            "the error must carry the docker stderr: {err}"
        );
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
        let result = select_credential(&available, &running).unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.key, "key-1");
        assert_eq!(result.total, 1);
        assert_eq!(result.in_use, 0);
    }

    #[test]
    fn multiple_credentials_none_in_use_selects_first() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![];
        let result = select_credential(&available, &running).unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.key, "key-1");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 0);
    }

    #[test]
    fn multiple_credentials_first_in_use_selects_second() {
        let available = vec![field("credential", "key-1"), field("credential-2", "key-2")];
        let running = vec![tunnel("scraper-gluetun", "key-1")];
        let result = select_credential(&available, &running).unwrap();
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
        let err = select_credential(&available, &running).unwrap_err();
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
        let result = select_credential(&available, &running).unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.in_use, 0);
    }
}
