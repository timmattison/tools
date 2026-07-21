#![warn(clippy::unused_async)]

mod chooser;
mod client;
mod commands;
mod config;
mod device_helper;
mod discovery;
mod http;
mod models;
mod output;
mod pagination;
mod prompt;
mod site_helper;
mod site_manager;
mod text;

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

/// Resolve a credential, preferring what the command line supplied over the
/// configuration file.
///
/// `supplied` is whatever clap parsed for the credential's flag, which already
/// carries CLI-over-environment precedence: the flag's `env = "..."` attribute
/// makes clap fall back to the variable (including one loaded from `.env`)
/// when the flag is absent.
///
/// "Configured but unreadable" and "not configured anywhere" are different
/// problems and get different answers: a configured credential that fails to
/// resolve reports the underlying cause (1Password CLI missing, prompt denied,
/// item renamed), because re-running the setup wizard cannot fix any of that.
fn resolve_credential(
    credential: Credential,
    supplied: Option<String>,
    config: Option<&Config>,
) -> Result<String> {
    if let Some(key) = supplied {
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
///
/// The connection and output options are `global`, so they are accepted at any
/// position: `ufa devices list --output json` and `ufa --output json devices
/// list` are the same command. Nothing about the leading position is
/// discoverable from the help text, and `--output` in particular is the flag
/// people reach for while scripting, so requiring it to come first would make
/// the natural spelling fail.
#[derive(Parser, Debug)]
#[clap(author, version = version_string!(), about)]
struct Args {
    /// UniFi controller URL (e.g., https://192.168.1.1)
    #[clap(long, global = true, env = "UNIFI_URL")]
    url: Option<String>,

    /// API key for authentication (generate in Settings -> Control Plane -> Integrations)
    #[clap(long, global = true, env = "UNIFI_API_KEY")]
    api_key: Option<String>,

    /// Skip TLS certificate verification
    #[clap(long, global = true, env = "UNIFI_INSECURE", value_parser = parse_bool_env)]
    insecure: Option<bool>,

    /// Output format
    #[clap(long, global = true, value_enum, default_value = "table")]
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
            file_config.as_ref(),
        )?;

        let sm_client = site_manager::SiteManagerClient::new(&sm_api_key)?;
        return site_manager::handle_cloud_command(command.clone(), &sm_client, args.output).await;
    }

    // Load configuration from file
    let file_config = Config::load()?;

    // Determine final configuration values. Each `args` field already resolves
    // its flag against the matching UNIFI_* variable via clap's `env`
    // attribute, so what remains here is the fall-through to the config file.
    let url = args.url
        .or_else(|| file_config.as_ref().and_then(|c| c.url.clone()))
        .context("UniFi URL not provided. Set it via --url, UNIFI_URL environment variable, or run 'ufa config setup' to create a configuration file.")?;

    let api_key = resolve_credential(Credential::Controller, args.api_key, file_config.as_ref())?;

    let insecure = args
        .insecure
        .or_else(|| file_config.as_ref().and_then(|c| c.insecure))
        .unwrap_or(false);

    let client = UnifiClient::new(&url, &api_key, insecure)?;

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

/// Pins the claim that clap — not hand-written `std::env::var` fallbacks —
/// is what turns `UNIFI_*` environment variables into parsed arguments.
///
/// `dotenvy::dotenv()` runs before `Args::parse()` and does nothing but write
/// into the process environment, so a value from a `.env` file is
/// indistinguishable from an exported one by the time clap looks. Covering the
/// environment therefore covers `.env` too.
#[cfg(test)]
mod environment_tests {
    use super::{Args, Commands};
    use clap::Parser;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// The process environment is shared by every thread in the test binary,
    /// so the tests that write to it take turns.
    ///
    /// Tests that merely *read* the environment — anything calling
    /// `Args::try_parse_from`, since clap consults `UNIFI_*` on every parse —
    /// take the same lock, so a `ScopedVar` set by one test cannot bleed into
    /// another test's parse.
    pub(super) fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Sets an environment variable for as long as it is held, then removes
    /// it — so a failing assertion cannot leak state into the next test.
    struct ScopedVar {
        name: &'static str,
        _guard: MutexGuard<'static, ()>,
    }

    impl ScopedVar {
        fn set(name: &'static str, value: &str) -> Self {
            let guard = env_lock();
            std::env::set_var(name, value);
            Self {
                name,
                _guard: guard,
            }
        }
    }

    impl Drop for ScopedVar {
        fn drop(&mut self) {
            std::env::remove_var(self.name);
        }
    }

    #[test]
    fn unifi_url_reaches_the_parsed_arguments() {
        let _var = ScopedVar::set("UNIFI_URL", "https://controller.example");

        let args = Args::try_parse_from(["ufa", "info"]).expect("ufa info must parse");

        assert_eq!(
            args.url.as_deref(),
            Some("https://controller.example"),
            "UNIFI_URL must reach --url without a hand-written fallback"
        );
    }

    #[test]
    fn unifi_api_key_reaches_the_parsed_arguments() {
        let _var = ScopedVar::set("UNIFI_API_KEY", "key-from-the-environment");

        let args = Args::try_parse_from(["ufa", "info"]).expect("ufa info must parse");

        assert_eq!(
            args.api_key.as_deref(),
            Some("key-from-the-environment"),
            "UNIFI_API_KEY must reach --api-key without a hand-written fallback"
        );
    }

    /// The environment spelling of a boolean goes through the same
    /// `parse_bool_env` value parser as the flag, so `on`/`yes`/`1` all work.
    #[test]
    fn unifi_insecure_reaches_the_parsed_arguments() {
        for (spelling, expected) in [("yes", true), ("1", true), ("off", false)] {
            let _var = ScopedVar::set("UNIFI_INSECURE", spelling);

            let args = Args::try_parse_from(["ufa", "info"]).expect("ufa info must parse");

            assert_eq!(
                args.insecure,
                Some(expected),
                "UNIFI_INSECURE={spelling} must reach --insecure without a hand-written fallback"
            );
        }
    }

    /// Unlike the hand-written fallback it replaces, clap *rejects* a value it
    /// cannot parse instead of silently falling through to the config file.
    #[test]
    fn an_unparseable_unifi_insecure_is_rejected_rather_than_ignored() {
        let _var = ScopedVar::set("UNIFI_INSECURE", "maybe");

        let error = Args::try_parse_from(["ufa", "info"])
            .expect_err("an unparseable UNIFI_INSECURE must not be silently discarded");

        assert!(
            error.to_string().contains("maybe"),
            "the failure must name the offending value, got {error}"
        );
    }

    #[test]
    fn unifi_site_manager_api_key_reaches_the_parsed_arguments() {
        let _var = ScopedVar::set(
            "UNIFI_SITE_MANAGER_API_KEY",
            "cloud-key-from-the-environment",
        );

        let args = Args::try_parse_from(["ufa", "cloud", "hosts"]).expect("ufa cloud hosts parses");

        let Commands::Cloud {
            site_manager_api_key,
            ..
        } = args.command
        else {
            panic!("`ufa cloud hosts` must parse as the cloud command");
        };
        assert_eq!(
            site_manager_api_key.as_deref(),
            Some("cloud-key-from-the-environment"),
            "UNIFI_SITE_MANAGER_API_KEY must reach --site-manager-api-key"
        );
    }

    /// An explicit flag still beats the environment.
    #[test]
    fn a_command_line_flag_overrides_the_environment() {
        let _var = ScopedVar::set("UNIFI_URL", "https://from-the-environment");

        let args = Args::try_parse_from(["ufa", "--url", "https://from-the-flag", "info"])
            .expect("ufa info must parse");

        assert_eq!(args.url.as_deref(), Some("https://from-the-flag"));
    }
}

/// Pins the claim that the connection and output options are accepted
/// *anywhere* on the command line, not only ahead of the subcommand.
///
/// `ufa devices list --output json` is the shape everyone reaches for when
/// scripting, and the position of a global-looking flag is not discoverable
/// from the help text — so rejecting the trailing spelling is a usability
/// defect, not a style preference.
#[cfg(test)]
mod flag_position_tests {
    use super::{environment_tests::env_lock, Args, Commands};
    use crate::output::OutputFormat;
    use clap::Parser;

    /// Every argv spelling that must yield `--output json`, with the flag
    /// placed after the subcommand it applies to.
    const TRAILING_OUTPUT_ARGV: &[&[&str]] = &[
        &["ufa", "cloud", "hosts", "--output", "json"],
        &["ufa", "devices", "list", "--output", "json"],
        &["ufa", "sites", "--output", "json"],
        &["ufa", "info", "--output", "json"],
        &["ufa", "clients", "list", "--output", "json"],
    ];

    fn parse(argv: &[&str]) -> Args {
        Args::try_parse_from(argv)
            .unwrap_or_else(|error| panic!("`{}` must parse, got {error}", argv.join(" ")))
    }

    fn is_json(format: OutputFormat) -> bool {
        matches!(format, OutputFormat::Json)
    }

    #[test]
    fn output_is_accepted_after_the_subcommand() {
        let _guard = env_lock();

        for argv in TRAILING_OUTPUT_ARGV {
            let args = parse(argv);
            assert!(
                is_json(args.output),
                "`{}` must select JSON output",
                argv.join(" ")
            );
        }
    }

    #[test]
    fn output_is_still_accepted_before_the_subcommand() {
        let _guard = env_lock();

        for argv in [
            ["ufa", "--output", "json", "cloud", "hosts"].as_slice(),
            ["ufa", "--output", "json", "devices", "list"].as_slice(),
            ["ufa", "--output", "json", "sites"].as_slice(),
        ] {
            let args = parse(argv);
            assert!(
                is_json(args.output),
                "`{}` must select JSON output",
                argv.join(" ")
            );
        }
    }

    /// The classic global-argument pitfall: the subcommand's own copy of the
    /// option carries the `table` default, which can silently overwrite the
    /// value the user gave earlier on the line.
    #[test]
    fn a_leading_output_is_not_overwritten_by_the_default() {
        let _guard = env_lock();

        let args = parse(&["ufa", "--output", "json", "devices", "list", "--limit", "5"]);

        assert!(
            is_json(args.output),
            "a leading --output must survive a subcommand that takes further flags"
        );
    }

    #[test]
    fn output_defaults_to_table_in_both_positions() {
        let _guard = env_lock();

        for argv in [
            ["ufa", "sites"].as_slice(),
            ["ufa", "devices", "list"].as_slice(),
            ["ufa", "cloud", "hosts"].as_slice(),
        ] {
            let args = parse(argv);
            assert!(
                matches!(args.output, OutputFormat::Table),
                "`{}` must fall back to the table default",
                argv.join(" ")
            );
        }
    }

    #[test]
    fn connection_flags_are_accepted_after_the_subcommand() {
        let _guard = env_lock();

        let args = parse(&[
            "ufa",
            "devices",
            "list",
            "--url",
            "https://controller.example",
            "--api-key",
            "trailing-key",
            "--insecure",
            "true",
        ]);

        assert_eq!(args.url.as_deref(), Some("https://controller.example"));
        assert_eq!(args.api_key.as_deref(), Some("trailing-key"));
        assert_eq!(args.insecure, Some(true));
        assert!(
            matches!(args.command, Commands::Devices { .. }),
            "the trailing connection flags must not disturb the parsed subcommand"
        );
    }

    /// A subcommand nested two levels deep still sees the options, and a
    /// subcommand-specific flag alongside them still binds to the subcommand.
    #[test]
    fn connection_flags_are_accepted_beside_subcommand_flags() {
        let _guard = env_lock();

        let args = parse(&[
            "ufa",
            "devices",
            "list",
            "--limit",
            "7",
            "--insecure",
            "false",
            "--output",
            "json",
        ]);

        assert_eq!(args.insecure, Some(false));
        assert!(is_json(args.output));

        let Commands::Devices { command, .. } = args.command else {
            panic!("`ufa devices list` must parse as the devices command");
        };
        assert!(
            matches!(
                command,
                crate::commands::devices::DevicesCommand::List { limit: 7, .. }
            ),
            "the subcommand's own --limit must still bind to the subcommand"
        );
    }

    #[test]
    fn connection_flags_are_still_accepted_before_the_subcommand() {
        let _guard = env_lock();

        let args = parse(&[
            "ufa",
            "--url",
            "https://leading.example",
            "--insecure",
            "true",
            "sites",
        ]);

        assert_eq!(args.url.as_deref(), Some("https://leading.example"));
        assert_eq!(args.insecure, Some(true));
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

        let error = resolve_credential(Credential::Controller, None, Some(&config))
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
        let error = resolve_credential(Credential::Controller, None, None)
            .expect_err("no key anywhere must not resolve");

        assert!(
            format!("{error:#}").contains("ufa config setup"),
            "an unconfigured key must point at setup, got {error:#}"
        );

        let error = resolve_credential(Credential::SiteManager, None, Some(&Config::default()))
            .expect_err("an empty config must not resolve a cloud key");
        assert!(
            format!("{error:#}").contains("ufa config cloud"),
            "an unconfigured cloud key must point at cloud setup, got {error:#}"
        );
    }

    /// What the command line supplied — the flag itself, or the UNIFI_* value
    /// clap resolved for it — beats the config file, and beats it without
    /// touching 1Password: a config whose reference cannot be read still
    /// resolves.
    #[test]
    fn a_supplied_key_wins_over_the_config_file() {
        let config = config_with_unreadable_reference();

        assert_eq!(
            resolve_credential(
                Credential::Controller,
                Some("from-the-command-line".to_string()),
                Some(&config),
            )
            .expect("a supplied key must resolve"),
            "from-the-command-line"
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
