mod client;
mod commands;
mod config;
mod device_helper;
mod discovery;
mod http;
mod models;
mod output;
mod pagination;
mod site_helper;
mod site_manager;

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::{Parser, Subcommand};
use client::UnifiClient;
use commands::*;
use config::Config;
use uuid::Uuid;

fn parse_bool_env(s: &str) -> Result<bool, String> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!(
            "Invalid boolean value: {}. Use true/false, 1/0, yes/no, or on/off",
            s
        )),
    }
}

/// A credential `ufa` needs, resolvable from a CLI flag, an environment
/// variable, or the configuration file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Credential {
    /// The UniFi controller API key.
    Controller,
    /// The UniFi Site Manager (cloud) API key.
    SiteManager,
}

impl Credential {
    /// Whether the configuration file names a source for this credential —
    /// either a 1Password reference or an in-file value.
    fn is_configured(self, config: &Config) -> bool {
        match self {
            Self::Controller => config.has_api_key(),
            Self::SiteManager => config.has_site_manager_key(),
        }
    }

    /// Read the credential from the configuration file, which may mean
    /// fetching it from 1Password.
    fn resolve(self, config: &Config) -> Result<String> {
        match self {
            Self::Controller => config.resolve_api_key(),
            Self::SiteManager => config.resolve_site_manager_api_key(),
        }
    }

    /// How the credential is named when a resolution failure is reported.
    fn description(self) -> &'static str {
        match self {
            Self::Controller => "API key",
            Self::SiteManager => "Site Manager API key",
        }
    }

    /// The advice shown when no source for this credential exists at all.
    fn missing_message(self) -> &'static str {
        match self {
            Self::Controller => "API key not provided. Set it via --api-key, UNIFI_API_KEY environment variable, or run 'ufa config setup' to create a configuration file.",
            Self::SiteManager => "Site Manager API key not provided. Set it via --site-manager-api-key, UNIFI_SITE_MANAGER_API_KEY environment variable, or run 'ufa config cloud' to set it up.",
        }
    }
}

/// Resolve a credential with CLI flag > environment variable > config file
/// precedence.
///
/// "Configured but unreadable" and "not configured anywhere" are different
/// problems and get different answers: a configured credential that fails to
/// resolve reports the underlying cause (1Password CLI missing, prompt denied,
/// item renamed), because re-running the setup wizard cannot fix any of that.
fn resolve_credential(
    credential: Credential,
    cli: Option<String>,
    env: Option<String>,
    config: Option<&Config>,
) -> Result<String> {
    if let Some(key) = cli.or(env) {
        return Ok(key);
    }

    match config {
        Some(config) if credential.is_configured(config) => credential
            .resolve(config)
            .with_context(|| format!("Failed to read the configured {}", credential.description())),
        _ => anyhow::bail!(credential.missing_message()),
    }
}

/// UniFi API CLI tool for managing UniFi Network applications
#[derive(Parser, Debug)]
#[clap(author, version = version_string!(), about)]
struct Args {
    /// UniFi controller URL (e.g., https://192.168.1.1)
    #[clap(long, env = "UNIFI_URL")]
    url: Option<String>,

    /// API key for authentication (generate in Settings -> Control Plane -> Integrations)
    #[clap(long, env = "UNIFI_API_KEY")]
    api_key: Option<String>,

    /// Skip TLS certificate verification
    #[clap(long, env = "UNIFI_INSECURE", value_parser = parse_bool_env)]
    insecure: Option<bool>,

    /// Output format
    #[clap(long, value_enum, default_value = "table")]
    output: output::OutputFormat,

    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List all sites
    Sites {
        /// Maximum number of sites to return
        #[clap(long, default_value = "25")]
        limit: u32,

        /// Offset for pagination
        #[clap(long, default_value = "0")]
        offset: u64,

        /// Filter expression
        #[clap(long)]
        filter: Option<String>,
    },

    /// Manage devices
    Devices {
        /// Site ID (if not provided, will auto-detect)
        #[clap(long)]
        site_id: Option<Uuid>,

        #[clap(subcommand)]
        command: devices::DevicesCommand,
    },

    /// Manage clients
    Clients {
        /// Site ID (if not provided, will auto-detect)
        #[clap(long)]
        site_id: Option<Uuid>,

        #[clap(subcommand)]
        command: clients::ClientsCommand,
    },

    /// Manage hotspot vouchers
    Vouchers {
        /// Site ID (if not provided, will auto-detect)
        #[clap(long)]
        site_id: Option<Uuid>,

        #[clap(subcommand)]
        command: vouchers::VouchersCommand,
    },

    /// Get application information
    Info,

    /// Configure ufa settings
    Config {
        #[clap(subcommand)]
        command: ConfigCommand,
    },

    /// Manage cloud-hosted UniFi consoles
    Cloud {
        /// Site Manager API key (generate at unifi.ui.com API section)
        #[clap(long, env = "UNIFI_SITE_MANAGER_API_KEY")]
        site_manager_api_key: Option<String>,

        #[clap(subcommand)]
        command: site_manager::CloudCommand,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    /// Interactive configuration setup
    Setup,
    /// Show current configuration file path
    Path,
    /// Setup cloud/Site Manager API credentials
    Cloud,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if it exists (ignore errors if file doesn't exist)
    let _ = dotenvy::dotenv();

    let args = Args::parse();

    // Handle config commands first (they don't need API connection)
    if let Commands::Config { command } = &args.command {
        match command {
            ConfigCommand::Setup => {
                Config::setup().await?;
                return Ok(());
            }
            ConfigCommand::Path => {
                let path = Config::config_file_path()?;
                println!("Configuration file path: {}", path.display());
                return Ok(());
            }
            ConfigCommand::Cloud => {
                Config::setup_site_manager()?;
                return Ok(());
            }
        }
    }

    // Handle cloud commands (they need Site Manager API key, not controller connection)
    if let Commands::Cloud {
        site_manager_api_key,
        command,
    } = &args.command
    {
        let file_config = Config::load()?;

        let sm_api_key = resolve_credential(
            Credential::SiteManager,
            site_manager_api_key.clone(),
            std::env::var("UNIFI_SITE_MANAGER_API_KEY").ok(),
            file_config.as_ref(),
        )?;

        let sm_client = site_manager::SiteManagerClient::new(&sm_api_key).await?;
        return site_manager::handle_cloud_command(command.clone(), &sm_client, args.output).await;
    }

    // Load configuration from file
    let file_config = Config::load()?;

    // Determine final configuration values (CLI args > env vars > config file)
    let url = args.url
        .or_else(|| std::env::var("UNIFI_URL").ok())
        .or_else(|| file_config.as_ref().and_then(|c| c.url.clone()))
        .context("UniFi URL not provided. Set it via --url, UNIFI_URL environment variable, or run 'ufa config setup' to create a configuration file.")?;

    let api_key = resolve_credential(
        Credential::Controller,
        args.api_key,
        std::env::var("UNIFI_API_KEY").ok(),
        file_config.as_ref(),
    )?;

    let insecure = args
        .insecure
        .or_else(|| {
            std::env::var("UNIFI_INSECURE")
                .ok()
                .and_then(|v| parse_bool_env(&v).ok())
        })
        .or_else(|| file_config.as_ref().and_then(|c| c.insecure))
        .unwrap_or(false);

    let client = UnifiClient::new(&url, &api_key, insecure).await?;

    match args.command {
        Commands::Sites {
            limit,
            offset,
            filter,
        } => {
            let cmd = sites::SitesCommand::List {
                limit,
                offset,
                filter,
            };
            sites::handle_sites_command(cmd, &client, args.output).await
        }
        Commands::Devices { site_id, command } => {
            devices::handle_devices_command(command, site_id, &client, args.output).await
        }
        Commands::Clients { site_id, command } => {
            clients::handle_clients_command(command, site_id, &client, args.output).await
        }
        Commands::Vouchers { site_id, command } => {
            vouchers::handle_vouchers_command(command, site_id, &client, args.output).await
        }
        Commands::Info => info::handle_info_command(&client, args.output).await,
        Commands::Config { .. } => unreachable!("Config commands handled above"),
        Commands::Cloud { .. } => unreachable!("Cloud commands handled above"),
    }
}

#[cfg(test)]
mod credential_tests {
    use super::{resolve_credential, Config, Credential};

    /// A config whose 1Password reference cannot be read.
    ///
    /// The reference is deliberately not an `op://` path: op-cache rejects it
    /// locally, so the test never shells out to the `op` CLI (no biometric
    /// prompt, no network, no dependency on the developer's vault).
    fn config_with_unreadable_reference() -> Config {
        Config {
            op_path: Some("not-an-op-path".to_string()),
            ..Config::default()
        }
    }

    /// When a key *is* configured but resolving it fails — 1Password CLI
    /// missing, biometric prompt denied, item renamed — the user must be told
    /// what actually went wrong. "API key not provided… run 'ufa config
    /// setup'" sends them to re-run a wizard that cannot fix any of that.
    #[test]
    fn resolution_failure_reports_the_underlying_cause() {
        let config = config_with_unreadable_reference();

        let error = resolve_credential(Credential::Controller, None, None, Some(&config))
            .expect_err("an unreadable 1Password reference must not resolve");
        let report = format!("{error:#}");

        assert!(
            report.contains("not-an-op-path"),
            "the failure must name the 1Password reference it could not read, got {report}"
        );
        assert!(
            !report.contains("not provided"),
            "a configured-but-unreadable key must not be reported as missing, got {report}"
        );
    }

    /// The friendly advice still applies when nothing is configured anywhere.
    #[test]
    fn missing_credential_reports_the_configuration_advice() {
        let error = resolve_credential(Credential::Controller, None, None, None)
            .expect_err("no key anywhere must not resolve");

        assert!(
            format!("{error:#}").contains("ufa config setup"),
            "an unconfigured key must point at setup, got {error:#}"
        );

        let error = resolve_credential(
            Credential::SiteManager,
            None,
            None,
            Some(&Config::default()),
        )
        .expect_err("an empty config must not resolve a cloud key");
        assert!(
            format!("{error:#}").contains("ufa config cloud"),
            "an unconfigured cloud key must point at cloud setup, got {error:#}"
        );
    }

    /// CLI flag beats the environment, which beats the config file.
    #[test]
    fn cli_argument_wins_over_environment_and_config_file() {
        let config = config_with_unreadable_reference();

        assert_eq!(
            resolve_credential(
                Credential::Controller,
                Some("from-cli".to_string()),
                Some("from-env".to_string()),
                Some(&config),
            )
            .expect("the CLI argument must resolve"),
            "from-cli"
        );
        assert_eq!(
            resolve_credential(
                Credential::Controller,
                None,
                Some("from-env".to_string()),
                Some(&config),
            )
            .expect("the environment variable must resolve"),
            "from-env"
        );
    }
}

#[cfg(test)]
mod version_tests {
    use super::Args;
    use clap::CommandFactory;

    /// CLAUDE.md mandates that every tool in this repository report its git
    /// commit hash and dirty status from `--version`/`-V`, in the form
    /// `toolname 0.1.0 (abc1234, clean)`.
    ///
    /// The assertion is structural rather than literal: hardcoding today's
    /// commit hash would make the test fail on every subsequent commit. It
    /// inspects the version clap will actually print, so it fails if the
    /// derive falls back to the bare `CARGO_PKG_VERSION`.
    #[test]
    fn version_reports_git_hash_and_dirty_status() {
        let version = Args::command()
            .get_version()
            .expect("ufa must declare a --version string")
            .to_string();

        let (package_version, suffix) = version.split_once(" (").unwrap_or_else(|| {
            panic!("--version must be `<version> (<hash>, <clean|dirty>)`, got {version:?}")
        });

        assert_eq!(
            package_version,
            env!("CARGO_PKG_VERSION"),
            "--version must lead with the package version, got {version:?}"
        );

        let suffix = suffix.strip_suffix(')').unwrap_or_else(|| {
            panic!("--version build-info suffix must be parenthesised, got {version:?}")
        });
        let (hash, status) = suffix.split_once(", ").unwrap_or_else(|| {
            panic!("--version suffix must be `(<hash>, <clean|dirty>)`, got {version:?}")
        });

        assert!(
            hash == "unknown" || (hash.len() == 7 && hash.chars().all(|c| c.is_ascii_hexdigit())),
            "--version must carry a 7-character git hash (or \"unknown\"), got {hash:?}"
        );
        assert!(
            matches!(status, "clean" | "dirty" | "unknown"),
            "--version must carry the working-tree status, got {status:?}"
        );
    }
}
