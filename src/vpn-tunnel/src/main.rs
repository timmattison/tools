mod cities;
mod credential;
mod generator;

use anyhow::{bail, Context, Result};
use buildinfo::version_string;
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const DEFAULT_OP_PATH: &str = "op://Private/ProtonVPN WireGuard key";
const DEFAULT_GLUETUN_VERSION: &str = "v3.40";
const DEFAULT_CONTAINER_PREFIX: &str = "vpn";
const CREDENTIAL_FIELD_PREFIX: &str = "credential";

#[derive(Parser)]
#[command(
    name = "vpn-tunnel",
    about = "Docker-based VPN tunnel using gluetun + ProtonVPN + WireGuard",
    version = version_string!()
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate docker-compose.yml and helper scripts
    Generate {
        /// Output directory for generated files
        #[arg(long, default_value = "./vpn")]
        output_dir: PathBuf,

        /// ProtonVPN city (omit for IP diversity across all US servers)
        #[arg(long)]
        city: Option<String>,

        /// Container name prefix
        #[arg(long, default_value = DEFAULT_CONTAINER_PREFIX)]
        container_prefix: String,

        /// 1Password item that holds the WireGuard credentials in fields named
        /// credential, credential-2, and so on (op://vault/item)
        #[arg(long, default_value = DEFAULT_OP_PATH)]
        op_path: String,

        /// Gluetun image tag
        #[arg(long, default_value = DEFAULT_GLUETUN_VERSION)]
        gluetun_version: String,

        /// Additional ports to expose (e.g., "8080:8080,9515:9515")
        #[arg(long)]
        extra_ports: Option<String>,

        /// List available ProtonVPN cities and exit
        #[arg(long)]
        list_cities: bool,
    },
    /// Start the VPN tunnel (docker compose up)
    Up {
        /// Directory containing docker-compose.yml
        #[arg(long, default_value = "./vpn")]
        dir: PathBuf,
    },
    /// Stop the VPN tunnel (docker compose down)
    Down {
        /// Directory containing docker-compose.yml
        #[arg(long, default_value = "./vpn")]
        dir: PathBuf,
    },
    /// View VPN tunnel logs
    Logs {
        /// Directory containing docker-compose.yml
        #[arg(long, default_value = "./vpn")]
        dir: PathBuf,
    },
    /// Show VPN tunnel status and IP
    Status {
        /// Directory containing docker-compose.yml
        #[arg(long, default_value = "./vpn")]
        dir: PathBuf,
    },
    /// Restart the VPN tunnel
    Restart {
        /// Directory containing docker-compose.yml
        #[arg(long, default_value = "./vpn")]
        dir: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("{} {e:#}", "error:".red().bold());
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Generate {
            output_dir,
            city,
            container_prefix,
            op_path,
            gluetun_version,
            extra_ports,
            list_cities,
        } => {
            if list_cities {
                println!("Available ProtonVPN cities (gluetun-supported):\n");
                for city in cities::PROTONVPN_US_CITIES {
                    println!("  {city}");
                }
                return Ok(());
            }

            // Validate city if provided
            if let Some(ref city_name) = city {
                if !cities::is_valid_city(city_name) {
                    let suggestions = cities::suggest_cities(city_name);
                    let mut msg =
                        format!("unknown ProtonVPN city: \"{city_name}\"\n\nAvailable cities:");
                    for c in cities::PROTONVPN_US_CITIES {
                        msg.push_str(&format!("\n  {c}"));
                    }
                    if !suggestions.is_empty() {
                        msg.push_str("\n\nDid you mean:");
                        for s in suggestions {
                            msg.push_str(&format!("\n  {s}"));
                        }
                    }
                    bail!("{msg}");
                }
            }

            // Check docker is available
            which::which("docker").context("docker not found in PATH — install Docker Desktop")?;

            // Fetch available WireGuard credentials via op-cache
            let op_path_validated =
                op_cache::OpPath::new(&op_path).map_err(|e| anyhow::anyhow!("{e}"))?;
            let cache = op_cache::OpCache::new().map_err(|e| anyhow::anyhow!("{e}"))?;

            let available_fields = cache
                .read_item_fields(&op_path_validated, CREDENTIAL_FIELD_PREFIX)
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            // Check which credentials running containers already hold.
            // A docker failure stops the command: handing out a credential on
            // unknown state risks reusing a key a running tunnel already holds.
            let running_tunnels = credential::find_running_tunnels()
                .context("could not determine which credentials running tunnels hold")?;

            // A directory that was generated before names its credential in
            // its own .env. A second run keeps that credential, because a new
            // selection can hand this directory the key of a directory that
            // nobody has started. The exception is a running tunnel of another
            // name that holds the key. Then `select_credential` selects a free
            // credential and names that tunnel. A directory that was generated
            // before the `CREDENTIAL_FIELD` line existed names no credential.
            // Then `select_credential` keeps the credential whose key the
            // running tunnel of this directory holds.
            let existing_field = read_credential_field(&output_dir);
            let gluetun_name = generator::gluetun_container_name(&container_prefix);

            let selected = credential::select_credential(
                &available_fields,
                &running_tunnels,
                existing_field.as_deref(),
                &gluetun_name,
            )
            .map_err(|err| anyhow::anyhow!("{}", all_in_use_message(&err, &op_path)))?;

            let wg_key = &selected.key;
            let credential_field = &selected.field_label;

            let extra_port_list: Vec<String> = extra_ports
                .map(|p| p.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default();

            generator::generate(
                &output_dir,
                city.as_deref(),
                &container_prefix,
                &gluetun_version,
                wg_key,
                credential_field,
                &extra_port_list,
            )?;

            println!(
                "\n{} Generated VPN tunnel in {}",
                "done:".green().bold(),
                output_dir.display()
            );
            // The count is the number of credentials that a running tunnel holds.
            let held = if selected.in_use == 1 {
                "1 held by a running tunnel".to_string()
            } else {
                format!("{} held by running tunnels", selected.in_use)
            };
            println!(
                "Using credential: {} ({} available, {held})",
                credential_field.cyan(),
                selected.total
            );
            if let Some(note) = selection_note(&selected, &output_dir) {
                println!("{note}");
            }
            println!("\nNext steps:");
            println!("  cd {} && ./start.sh", output_dir.display());
            println!("  ./status.sh          # check VPN IP");
            println!("  ./logs.sh            # view logs");
            println!("  ./stop.sh            # tear down");
            println!("\nTo route a container through the VPN, add to your docker-compose.yml:");
            println!("  network_mode: \"service:{gluetun_name}\"");
            println!("  depends_on:");
            println!("    {gluetun_name}:");
            println!("      condition: service_healthy");
        }
        Commands::Up { dir } => {
            ensure_compose_exists(&dir)?;
            println!("{}", "Starting VPN tunnel...".blue());
            docker_compose(&dir, &["up", "-d"])?;
            println!("{}", "Waiting for VPN to become healthy...".blue());
            wait_for_healthy(&dir)?;
            show_vpn_ip(&dir)?;
        }
        Commands::Down { dir } => {
            ensure_compose_exists(&dir)?;
            docker_compose(&dir, &["down"])?;
            println!("{}", "VPN tunnel stopped.".green());
        }
        Commands::Logs { dir } => {
            ensure_compose_exists(&dir)?;
            docker_compose(&dir, &["logs", "-f"])?;
        }
        Commands::Status { dir } => {
            ensure_compose_exists(&dir)?;
            docker_compose(&dir, &["ps"])?;
            if let Some(field) = read_credential_field(&dir) {
                println!("Credential: {}", field.cyan());
            }
            show_vpn_ip(&dir)?;
        }
        Commands::Restart { dir } => {
            ensure_compose_exists(&dir)?;
            docker_compose(&dir, &["down"])?;
            docker_compose(&dir, &["up", "-d"])?;
            println!("{}", "Waiting for VPN to become healthy...".blue());
            wait_for_healthy(&dir)?;
            show_vpn_ip(&dir)?;
        }
    }

    Ok(())
}

fn ensure_compose_exists(dir: &Path) -> Result<()> {
    let compose_file = dir.join("docker-compose.yml");
    if !compose_file.exists() {
        bail!(
            "no docker-compose.yml found in {}\nRun 'vpn-tunnel generate' first.",
            dir.display()
        );
    }
    Ok(())
}

fn docker_compose(dir: &Path, args: &[&str]) -> Result<()> {
    let status = childenv::command("docker")
        .arg("compose")
        .args(args)
        .current_dir(dir)
        .status()
        .context("failed to run docker compose")?;

    if !status.success() {
        bail!("docker compose exited with status {status}");
    }
    Ok(())
}

fn wait_for_healthy(dir: &Path) -> Result<()> {
    // Find the gluetun container name from running containers
    let output = childenv::command("docker")
        .args(["compose", "ps", "--format", "{{.Name}}"])
        .current_dir(dir)
        .output()
        .context("failed to list containers")?;

    let containers = String::from_utf8_lossy(&output.stdout);
    let gluetun_name = containers
        .lines()
        .find(|l| l.contains("gluetun"))
        .unwrap_or("gluetun");

    for i in 0..30 {
        let health = childenv::command("docker")
            .args([
                "inspect",
                "--format",
                "{{.State.Health.Status}}",
                gluetun_name,
            ])
            .output();

        if let Ok(out) = health {
            let status = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if status == "healthy" {
                eprintln!();
                println!("{}", "VPN is healthy!".green().bold());
                return Ok(());
            }
        }

        if i < 29 {
            eprint!(".");
            std::io::stderr().flush().ok();
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    }
    eprintln!();
    bail!("VPN failed to become healthy within 90s — check logs with 'vpn-tunnel logs'");
}

fn show_vpn_ip(dir: &Path) -> Result<()> {
    let output = childenv::command("docker")
        .args(["compose", "ps", "--format", "{{.Name}}"])
        .current_dir(dir)
        .output()?;

    let containers = String::from_utf8_lossy(&output.stdout);
    let gluetun_name = containers
        .lines()
        .find(|l| l.contains("gluetun"))
        .unwrap_or("gluetun");

    let ip_output = childenv::command("docker")
        .args(["exec", gluetun_name, "wget", "-qO-", "icanhazip.com"])
        .output();

    match ip_output {
        Ok(out) if out.status.success() => {
            let ip = String::from_utf8_lossy(&out.stdout).trim().to_string();
            println!("VPN IP: {}", ip.cyan().bold());
        }
        _ => {
            eprintln!(
                "{}",
                "Could not determine VPN IP (container may not be running)".yellow()
            );
        }
    }
    Ok(())
}

/// Reads the name of the 1Password credential field that the tunnel in `dir`
/// was generated from.
///
/// The name comes off the first `CREDENTIAL_FIELD=` line of `<dir>/.env`, with
/// the surrounding whitespace taken off, because a hand-edited file carries a
/// trailing space or a carriage return that no reader of `status` can see.
/// A first line that holds no name after that returns [`None`] rather than an
/// empty name, and the search stops there: [`None`] is also the answer when the
/// file is missing, unreadable, or names no field at all.
fn read_credential_field(dir: &Path) -> Option<String> {
    let env_path = dir.join(".env");
    let content = fs::read_to_string(env_path).ok()?;
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("CREDENTIAL_FIELD=") {
            let value = value.trim();
            if value.is_empty() {
                return None;
            }
            return Some(value.to_string());
        }
    }
    None
}

/// The line that says how `generate` came to the credential it selected.
///
/// A run that keeps the credential that the `.env` names says so. A run that
/// does not keep it names the running tunnel that holds its key. A run that
/// keeps the key of the own running tunnel, because the `.env` names no
/// credential, names that tunnel. For a run under the ordinary rule
/// ([`credential::Selection::FirstFree`]), this function returns [`None`].
///
/// The text carries no color, so a test of it does not depend on the terminal.
fn selection_note(selected: &credential::SelectedCredential, output_dir: &Path) -> Option<String> {
    match &selected.selection {
        credential::Selection::Reused => Some(format!(
            "Reused the credential that the .env in {} already names.",
            output_dir.display()
        )),
        credential::Selection::Replaced {
            named_label,
            holder,
        } => Some(format!(
            "The .env in {} names {named_label}, but the running tunnel {holder} holds its key. \
             This run selected {}.",
            output_dir.display(),
            selected.field_label
        )),
        credential::Selection::OwnTunnel { holder } => Some(format!(
            "Kept {}, because the running tunnel {holder} already holds its key.",
            selected.field_label
        )),
        credential::Selection::FirstFree => None,
    }
}

/// Builds the error text for a run in which a running tunnel holds each credential.
///
/// The text names the holder of each credential and the ways to go on.
/// The `op-cache` tool keeps the list of credentials with no expiry. Thus the text
/// names the `op-cache invalidate` command, with `op_path` quoted for a shell.
/// `op_path` is the `--op-path` value of the run. The text carries no color.
fn all_in_use_message(err: &credential::AllCredentialsInUse, op_path: &str) -> String {
    let mut msg = "every WireGuard credential is held by a running tunnel\n".to_string();
    for usage in &err.usage {
        msg.push_str(&format!(
            "\n  {:<20} -> held by {}",
            usage.field_label, usage.container_name
        ));
    }
    msg.push_str("\n\nAdd another credential to the item in 1Password, then run");
    msg.push_str(&format!(
        "\n  op-cache invalidate {}",
        shellquote::shell_quote(op_path)
    ));
    msg.push_str(
        "\nand run vpn-tunnel generate again. op-cache keeps the old list of credentials until then.",
    );
    msg.push_str("\nOr stop an existing tunnel with: vpn-tunnel down --dir <path>");
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_op_path_help_names_an_item_and_its_credential_fields() {
        use clap::CommandFactory;
        let cli = Cli::command();
        let help = cli
            .find_subcommand("generate")
            .and_then(|generate| generate.get_arguments().find(|a| a.get_id() == "op_path"))
            .and_then(clap::Arg::get_help)
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(help.contains("op://vault/item"), "help: {help}");
        assert!(help.contains("credential-2"), "help: {help}");
    }

    /// Builds the error that a run gets when two tunnels hold both credentials.
    fn two_in_use() -> credential::AllCredentialsInUse {
        credential::AllCredentialsInUse {
            usage: vec![
                credential::CredentialInUse {
                    field_label: "credential".to_string(),
                    container_name: "scraper-gluetun".to_string(),
                },
                credential::CredentialInUse {
                    field_label: "credential-2".to_string(),
                    container_name: "vpn-gluetun".to_string(),
                },
            ],
        }
    }

    #[test]
    fn the_all_in_use_error_lists_each_holder_under_its_header() {
        let text = all_in_use_message(&two_in_use(), "op://Private/ProtonVPN WireGuard key");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "every WireGuard credential is held by a running tunnel"
        );
        assert_eq!(lines[1], "");
        assert_eq!(
            lines[2],
            "  credential           -> held by scraper-gluetun"
        );
        assert_eq!(lines[3], "  credential-2         -> held by vpn-gluetun");
    }

    /// Writes a `.env` holding `content` into a fresh temporary directory.
    ///
    /// The directory is keyed by the operating system, so two copies of the
    /// same test running at once never read each other's fixture. The caller
    /// holds the returned handle, because the directory goes away when it
    /// drops.
    fn env_dir(content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".env"), content).unwrap();
        dir
    }

    #[test]
    fn plain_value_reads_back_unchanged() {
        let dir = env_dir("WIREGUARD_PRIVATE_KEY=k\nCREDENTIAL_FIELD=credential-2\n");
        assert_eq!(
            read_credential_field(dir.path()),
            Some("credential-2".to_string())
        );
    }

    #[test]
    fn trailing_space_is_dropped() {
        let dir = env_dir("CREDENTIAL_FIELD=credential-2  \n");
        assert_eq!(
            read_credential_field(dir.path()),
            Some("credential-2".to_string())
        );
    }

    #[test]
    fn trailing_carriage_return_is_dropped() {
        // `str::lines` takes a carriage return off a line only when a line feed
        // follows it, so a file whose last line carries no line feed keeps the
        // carriage return. A Windows editor writes that file.
        let dir = env_dir("WIREGUARD_PRIVATE_KEY=k\r\nCREDENTIAL_FIELD=credential-2\r");
        assert_eq!(
            read_credential_field(dir.path()),
            Some("credential-2".to_string())
        );
    }

    #[test]
    fn crlf_line_ending_leaves_no_carriage_return() {
        let dir = env_dir("CREDENTIAL_FIELD=credential-2\r\nWIREGUARD_PRIVATE_KEY=k\r\n");
        assert_eq!(
            read_credential_field(dir.path()),
            Some("credential-2".to_string())
        );
    }

    #[test]
    fn a_value_of_whitespace_is_no_field() {
        let dir = env_dir("CREDENTIAL_FIELD=   \n");
        assert_eq!(read_credential_field(dir.path()), None);
    }

    #[test]
    fn an_empty_value_is_no_field() {
        let dir = env_dir("CREDENTIAL_FIELD=\n");
        assert_eq!(read_credential_field(dir.path()), None);
    }

    #[test]
    fn the_first_line_decides_even_when_it_is_blank() {
        let dir = env_dir("CREDENTIAL_FIELD=\nCREDENTIAL_FIELD=credential-3\n");
        assert_eq!(read_credential_field(dir.path()), None);
    }

    #[test]
    fn a_file_without_the_line_has_no_field() {
        let dir = env_dir("WIREGUARD_PRIVATE_KEY=k\n");
        assert_eq!(read_credential_field(dir.path()), None);
    }

    /// A credential that `select_credential` selected in the given way.
    fn selected(
        field_label: &str,
        selection: credential::Selection,
    ) -> credential::SelectedCredential {
        credential::SelectedCredential {
            field_label: field_label.to_string(),
            key: "key".to_string(),
            total: 2,
            in_use: 1,
            selection,
        }
    }

    #[test]
    fn a_reused_credential_says_that_the_env_names_it() {
        let note = selection_note(
            &selected("credential-2", credential::Selection::Reused),
            Path::new("./vpn"),
        );
        assert_eq!(
            note.as_deref(),
            Some("Reused the credential that the .env in ./vpn already names.")
        );
    }

    #[test]
    fn an_own_tunnel_credential_says_that_the_running_tunnel_holds_its_key() {
        let selection = credential::Selection::OwnTunnel {
            holder: "vpn-gluetun".to_string(),
        };
        let note = selection_note(&selected("credential-2", selection), Path::new("./vpn"));
        assert_eq!(
            note.as_deref(),
            Some(
                "Kept credential-2, because the running tunnel vpn-gluetun already holds its key."
            )
        );
    }

    #[test]
    fn a_first_free_credential_gets_no_note() {
        let note = selection_note(
            &selected("credential", credential::Selection::FirstFree),
            Path::new("./vpn"),
        );
        assert_eq!(note, None);
    }

    #[test]
    fn a_replaced_credential_names_the_env_the_label_the_holder_and_the_selection() {
        let selection = credential::Selection::Replaced {
            named_label: "credential".to_string(),
            holder: "a-gluetun".to_string(),
        };
        let note = selection_note(&selected("credential-2", selection), Path::new("./b"));
        assert_eq!(
            note.as_deref(),
            Some(concat!(
                "The .env in ./b names credential, but the running tunnel a-gluetun ",
                "holds its key. This run selected credential-2."
            ))
        );
    }

    #[test]
    fn the_all_in_use_error_names_the_invalidate_command_with_the_quoted_item_path() {
        let text = all_in_use_message(&two_in_use(), "op://Private/ProtonVPN WireGuard key");
        assert!(
            text.lines()
                .any(|line| line == "  op-cache invalidate 'op://Private/ProtonVPN WireGuard key'"),
            "the text names no invalidate command:\n{text}"
        );
    }

    #[test]
    fn the_all_in_use_error_quotes_a_single_quote_in_the_item_path() {
        let text = all_in_use_message(&two_in_use(), "op://Private/Tim's key");
        assert!(
            text.lines()
                .any(|line| line == "  op-cache invalidate 'op://Private/Tim'\\''s key'"),
            "the path is not quoted:\n{text}"
        );
    }

    #[test]
    fn the_all_in_use_error_ends_with_the_steps_to_go_on() {
        let text = all_in_use_message(&two_in_use(), "op://Private/ProtonVPN WireGuard key");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[4..],
            [
                "",
                "Add another credential to the item in 1Password, then run",
                "  op-cache invalidate 'op://Private/ProtonVPN WireGuard key'",
                "and run vpn-tunnel generate again. op-cache keeps the old list of credentials until then.",
                "Or stop an existing tunnel with: vpn-tunnel down --dir <path>",
            ]
        );
    }
}
