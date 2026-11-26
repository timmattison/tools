#![warn(clippy::unused_async)]
// Standard output carries the document the user asked for, and nothing else:
// under `--output json` a program reads that stream, and a sentence on it is a
// line the program cannot parse. Every note, status sentence, question and hint
// goes to standard error, in both output formats. `crate::output` prints the
// document. Any other print to standard output states its reason at the site.
#![deny(clippy::print_stdout)]

mod chooser;
mod client;
mod commands;
mod config;
mod device_helper;
mod discovery;
mod http;
mod listing;
mod models;
mod output;
mod pagination;
mod prompt;
mod secret;
mod site_helper;
mod site_manager;
#[cfg(test)]
mod spec_conformance;
#[cfg(test)]
mod test_server;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod test_vault;
mod text;

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::{Parser, Subcommand};
use client::UnifiClient;
use commands::*;
use config::{CachedOnePassword, Config, KeyStore, ResolvedKey};
use reqwest::StatusCode;
use secret::Secret;
use std::future::Future;
use std::io::Write;
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
    ///
    /// # Arguments
    ///
    /// * `config` - The configuration file.
    /// * `keys` - The store that reads a 1Password reference.
    ///
    /// # Returns
    ///
    /// The key, and the reference it came from when it came from one.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration file names no key, and if the
    /// reference it names cannot be read.
    fn resolve(self, config: &Config, keys: &impl KeyStore) -> Result<ResolvedKey> {
        match self {
            Self::Controller => config.resolve_api_key(keys),
            Self::SiteManager => config.resolve_site_manager_api_key(keys),
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
/// makes clap fall back to the variable (including one loaded from the settings
/// file) when the flag is absent.
///
/// A supplied value that holds only blank space names no credential, so it is
/// read as absent: `export UNIFI_API_KEY=` and `--api-key ""` fall through to
/// the configuration file, and then to the advice below, instead of sending a
/// blank key to the controller for a 401.
///
/// "Configured but unreadable" and "not configured anywhere" are different
/// problems and get different answers: a configured credential that fails to
/// resolve reports the underlying cause (1Password CLI missing, prompt denied,
/// item renamed), because re-running the setup wizard cannot fix any of that.
///
/// # Arguments
///
/// * `credential` - The credential to resolve.
/// * `supplied` - What the command line and the environment supplied.
/// * `config` - The configuration file, when there is one.
/// * `keys` - The store that reads a 1Password reference.
///
/// # Returns
///
/// The key, and the 1Password reference it came from when it came from one. A
/// supplied key names no reference.
///
/// # Errors
///
/// Returns an error if no source names the credential, and if the source that
/// names it cannot be read.
fn resolve_credential(
    credential: Credential,
    supplied: Option<Secret>,
    config: Option<&Config>,
    keys: &impl KeyStore,
) -> Result<ResolvedKey> {
    if let Some(key) = supplied.filter(|key| !config::is_blank(key.expose())) {
        return Ok(ResolvedKey::without_reference(key));
    }

    match config {
        Some(config) if credential.is_configured(config) => credential
            .resolve(config, keys)
            .with_context(|| format!("Failed to read the configured {}", credential.description())),
        _ => anyhow::bail!(credential.missing_message()),
    }
}

/// Run a command with a key, and run it once more when the API refused a copy
/// of the key that 1Password no longer holds.
///
/// op-cache answers each read of a 1Password reference from the copy it kept,
/// and it asks 1Password only when it holds no copy. So after the user rotates
/// a key in 1Password, the copy is the old key, and the API refuses it. The
/// error of that refusal tells the user to check the key or to make a new one,
/// which is what the user just did.
///
/// A refusal is a 401 or a 403. After a refusal of a key that came from a
/// 1Password reference, the store forgets its copy and reads the key from
/// 1Password again:
///
/// - A different key: one note says so, and the command runs once more with
///   the new key. The command runs two times at most.
/// - The same key: the key is wrong, not stale, and the first error comes
///   back. The cache now holds the copy that 1Password just gave.
/// - A read that fails: the first error comes back, and one note gives the
///   reason of the failed read.
///
/// A 403 can also refuse a key that is current but has limited permissions.
/// For such a key, each command that gets a 403 costs one more read of
/// 1Password, which can show a prompt of 1Password, and the first error comes
/// back.
///
/// Any other failure, and a failure with a key that came from no reference,
/// comes back as it is, and 1Password gets no request.
///
/// The second run is safe. Every request carries the key, so a stale key
/// fails the first request of a command, and the first run changed nothing.
///
/// # Arguments
///
/// * `credential` - The credential the command sends.
/// * `resolved` - The key, and the 1Password reference it came from when it
///   came from one.
/// * `keys` - The store that reads the key again.
/// * `notes` - Where the notes for the user go.
/// * `run` - Runs the command with the key it gets.
///
/// # Returns
///
/// What the last run of the command returned.
///
/// # Errors
///
/// Returns the error of the last run. When the command does not run again
/// after a refusal, that is the error of the first run.
async fn run_with_the_current_key<F, Fut>(
    credential: Credential,
    resolved: ResolvedKey,
    keys: &impl KeyStore,
    notes: &mut impl Write,
    mut run: F,
) -> Result<()>
where
    F: FnMut(Secret) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let error = match run(resolved.key().clone()).await {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };

    let Some(reference) = resolved.reference() else {
        return Err(error);
    };
    if !matches!(
        http::answered_status(&error),
        Some(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
    ) {
        return Err(error);
    }

    match config::read_again_from_1password(keys, reference) {
        Ok(current) if current.expose() != resolved.key().expose() => {
            write_note(
                notes,
                &format!(
                    "note: The {} in 1Password changed since ufa cached it. \
                     ufa runs the command again with the new key.",
                    credential.description()
                ),
            );
            run(current).await
        }
        Ok(_) => Err(error),
        Err(reread) => {
            write_note(
                notes,
                &format!(
                    "note: The API refused the {}, and ufa could not read it again from \
                     1Password: {reread:#}",
                    credential.description()
                ),
            );
            Err(error)
        }
    }
}

/// Write one note for the user.
///
/// A note that cannot be written does not change the result of the command.
/// The result is the answer the user asked for, and a note that is lost costs
/// less than an answer that is lost.
///
/// # Arguments
///
/// * `notes` - Where the note goes.
/// * `note` - The note, on one line.
fn write_note(notes: &mut impl Write, note: &str) {
    let _ = writeln!(notes, "{note}");
}

/// Said before the first request whenever certificate verification is off.
///
/// The wizard asks about the setting once, at `config.rs`. Every other route
/// to it — the flag, `UNIFI_INSECURE`, the settings file, or a value already
/// sitting in the configuration file — used to say nothing at all, so a choice
/// made once for one self-signed controller stayed silent on every run after
/// it.
const INSECURE_WARNING: &str = "warning: TLS certificate verification is off. \
     ufa sends the API key to the controller over a connection that nobody \
     checked. To turn the check back on, drop --insecure, UNIFI_INSECURE, and \
     `insecure` in the configuration file.";

/// Resolve whether to skip TLS certificate verification, and say so when the
/// answer is yes.
///
/// `supplied` is whatever clap parsed for `--insecure`, which already carries
/// the flag's precedence over `UNIFI_INSECURE` and over the settings file. The
/// configuration file answers when the command line says nothing, and
/// verification stays on when neither does.
///
/// The warning follows the *resolved* value, so it does not matter which
/// source set it. This is the one place the value is decided, which is what
/// makes that true: a caller cannot reach the setting without passing the
/// notice.
///
/// A warning that cannot be written fails the run rather than vanishing. The
/// alternative is to send the user's API key over an unverified connection
/// after the one notice about it was lost.
///
/// # Arguments
///
/// * `supplied` - What the command line and the environment supplied.
/// * `config` - The configuration file, when there is one.
/// * `warnings` - Where the notice goes.
///
/// # Returns
///
/// Whether the client skips certificate verification.
fn resolve_insecure(
    supplied: Option<bool>,
    config: Option<&Config>,
    warnings: &mut impl Write,
) -> Result<bool> {
    let insecure = supplied
        .or_else(|| config.and_then(|config| config.insecure))
        .unwrap_or(false);

    if insecure {
        writeln!(warnings, "{INSECURE_WARNING}")
            .context("Failed to warn that TLS certificate verification is off")?;
    }

    Ok(insecure)
}

/// The advice shown when no source names the URL of the controller.
const MISSING_URL_MESSAGE: &str = "UniFi URL not provided. Set it via --url, UNIFI_URL environment variable, or run 'ufa config setup' to create a configuration file.";

/// Resolve the URL of the controller, preferring what the command line
/// supplied over the configuration file.
///
/// `supplied` is what clap parsed for `--url`, which already falls back to
/// `UNIFI_URL`.
///
/// A value that holds only blank space names no URL, as a blank key names no
/// key in [`resolve_credential`]. So `export UNIFI_URL=` and `--url ""` fall
/// through to the configuration file, and a blank `url` field falls through
/// to the advice.
///
/// # Arguments
///
/// * `supplied` - What the command line and the environment supplied.
/// * `config` - The configuration file, when there is one.
///
/// # Returns
///
/// The URL, exactly as its source gave it.
///
/// # Errors
///
/// Returns the advice of [`MISSING_URL_MESSAGE`] if no source names a URL.
fn resolve_url(supplied: Option<String>, config: Option<&Config>) -> Result<String> {
    let names_a_url = |url: &String| !config::is_blank(url);
    supplied
        .filter(names_a_url)
        .or_else(|| config.and_then(|config| config.url.clone().filter(names_a_url)))
        .context(MISSING_URL_MESSAGE)
}

// The connection and output options are `global = true`, so they are accepted
// at any position: `ufa devices list --output json` and `ufa --output json
// devices list` are the same command. Nothing about the leading position was
// discoverable from the help text, and `--output` is the flag people reach for
// while scripting, so requiring it to come first made the natural spelling
// fail. `global` also lists the options in every subcommand's `--help`.
//
// This is a plain comment rather than a doc comment on purpose: a second
// paragraph on `Args` would become clap's `long_about` and print this rationale
// to users running `ufa --help`.
/// UniFi API CLI tool for managing UniFi Network applications
#[derive(Parser, Debug)]
#[clap(author, version = version_string!(), about)]
struct Args {
    /// UniFi controller URL (e.g., https://192.168.1.1)
    #[clap(long, global = true, env = "UNIFI_URL")]
    url: Option<String>,

    /// API key for authentication (generate in Settings -> Control Plane -> Integrations)
    //
    // `hide_env_values` for the same reason `Secret` redacts: clap otherwise
    // prints what the variable holds beside the flag, so `ufa --help` prints
    // the key of every user who exported one. The variable's *name* stays in
    // the help text, because that is what tells the user where the value came
    // from.
    #[clap(long, global = true, env = "UNIFI_API_KEY", hide_env_values = true)]
    api_key: Option<Secret>,

    /// Skip TLS certificate verification
    #[clap(long, global = true, env = "UNIFI_INSECURE", value_parser = parse_bool_env)]
    insecure: Option<bool>,

    /// Output format
    #[clap(long, global = true, value_enum, default_value = "table")]
    output: output::OutputFormat,

    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug, Clone)]
enum Commands {
    /// List all sites
    Sites {
        #[clap(flatten)]
        args: sites::SitesArgs,
    },

    /// Manage devices
    Devices {
        /// Site ID (if omitted: the only site, else a choice at a terminal)
        #[clap(long)]
        site_id: Option<Uuid>,

        #[clap(subcommand)]
        command: devices::DevicesCommand,
    },

    /// Manage clients
    Clients {
        /// Site ID (if omitted: the only site, else a choice at a terminal)
        #[clap(long)]
        site_id: Option<Uuid>,

        #[clap(subcommand)]
        command: clients::ClientsCommand,
    },

    /// Manage hotspot vouchers
    Vouchers {
        /// Site ID (if omitted: the only site, else a choice at a terminal)
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
        // Hidden for the same reason the controller key is: see `Args`.
        #[clap(long, env = "UNIFI_SITE_MANAGER_API_KEY", hide_env_values = true)]
        site_manager_api_key: Option<Secret>,

        #[clap(subcommand)]
        command: site_manager::CloudCommand,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum ConfigCommand {
    /// Interactive configuration setup
    Setup,
    /// Show current configuration file path
    Path,
    /// Setup cloud/Site Manager API credentials
    Cloud,
}

/// The report of `ufa config path`.
///
/// The path is the whole document, with no label, so `$(ufa config path)`
/// gives a shell the path itself. The name of the command says what the
/// line is.
///
/// # Arguments
///
/// * `path` - Where the configuration file is, or goes when it is written.
///
/// # Returns
///
/// The report that gives the path.
fn config_path_report(path: &std::path::Path) -> output::Report {
    output::Report::of_document(path.display().to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    // The settings file sits beside the configuration file, and it is read
    // before the parse because clap reads the UNIFI_* settings during the
    // parse. A system that names no configuration directory holds no settings
    // file either, and `Config::load` reports that below for the commands that
    // need one — so `ufa --version` still answers on such a system.
    if let Ok(directory) = Config::config_dir() {
        if let Some(path) = config::load_environment_file(&directory)? {
            eprintln!("Loaded settings from {}", path.display());
        }
    }

    let args = Args::parse();

    // Handle config commands first (they don't need API connection)
    if let Commands::Config { command } = &args.command {
        match command {
            ConfigCommand::Setup => {
                Config::setup().await?;
                return Ok(());
            }
            ConfigCommand::Path => {
                config_path_report(&Config::config_file_path()?).print();
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
        let keys = CachedOnePassword::beside_the_configuration()?;

        let sm_api_key = resolve_credential(
            Credential::SiteManager,
            site_manager_api_key.clone(),
            file_config.as_ref(),
            &keys,
        )?;

        let output = args.output;
        return run_with_the_current_key(
            Credential::SiteManager,
            sm_api_key,
            &keys,
            &mut std::io::stderr(),
            |key| {
                let command = command.clone();
                async move {
                    let sm_client = site_manager::SiteManagerClient::new(key.expose())?;
                    site_manager::handle_cloud_command(command, &sm_client, output).await
                }
            },
        )
        .await;
    }

    // Load configuration from file
    let file_config = Config::load()?;
    let keys = CachedOnePassword::beside_the_configuration()?;

    // Determine final configuration values. Each `args` field already resolves
    // its flag against the matching UNIFI_* variable via clap's `env`
    // attribute, so what remains here is the fall-through to the config file.
    let url = resolve_url(args.url, file_config.as_ref())?;

    let api_key = resolve_credential(
        Credential::Controller,
        args.api_key,
        file_config.as_ref(),
        &keys,
    )?;

    let insecure = resolve_insecure(args.insecure, file_config.as_ref(), &mut std::io::stderr())?;

    let output = args.output;
    let command = args.command;
    let url = url.as_str();
    run_with_the_current_key(
        Credential::Controller,
        api_key,
        &keys,
        &mut std::io::stderr(),
        |key| {
            let command = command.clone();
            async move {
                let client = UnifiClient::new(url, key.expose(), insecure)?;
                run_controller_command(command, &client, output).await
            }
        },
    )
    .await
}

/// Run a controller command against a controller.
///
/// # Arguments
///
/// * `command` - The command the user gave.
/// * `client` - The client of the controller.
/// * `output` - The output format the user asked for.
///
/// # Returns
///
/// What the command returned.
///
/// # Errors
///
/// Returns the error of the command.
///
/// # Panics
///
/// Panics if `command` is a configuration command or a cloud command. `main`
/// runs those before it builds a controller client.
async fn run_controller_command(
    command: Commands,
    client: &UnifiClient,
    output: output::OutputFormat,
) -> Result<()> {
    match command {
        Commands::Sites { args } => sites::handle_sites_command(args, client, output).await,
        Commands::Devices { site_id, command } => {
            devices::handle_devices_command(command, site_id, client, output).await
        }
        Commands::Clients { site_id, command } => {
            clients::handle_clients_command(command, site_id, client, output).await
        }
        Commands::Vouchers { site_id, command } => {
            vouchers::handle_vouchers_command(command, site_id, client, output).await
        }
        Commands::Info => info::handle_info_command(client, output).await,
        Commands::Config { .. } => unreachable!("Config commands handled above"),
        Commands::Cloud { .. } => unreachable!("Cloud commands handled above"),
    }
}

/// Pins the claim that clap — not hand-written `std::env::var` fallbacks —
/// is what turns `UNIFI_*` environment variables into parsed arguments.
///
/// The settings file is loaded before `Args::parse()` and does nothing but
/// write into the process environment, so a value from that file is
/// indistinguishable from an exported one by the time clap looks. Covering the
/// environment therefore covers the settings file too, and
/// `config::load_environment_file` covers which file that is.
#[cfg(test)]
mod environment_tests {
    use super::{Commands, Secret};
    use crate::test_support::{parse_args_for_test, ScopedVar};

    #[test]
    fn unifi_url_reaches_the_parsed_arguments() {
        let _var = ScopedVar::set("UNIFI_URL", "https://controller.example");

        let args = parse_args_for_test(["ufa", "info"]).expect("ufa info must parse");

        assert_eq!(
            args.url.as_deref(),
            Some("https://controller.example"),
            "UNIFI_URL must reach --url without a hand-written fallback"
        );
    }

    #[test]
    fn unifi_api_key_reaches_the_parsed_arguments() {
        let _var = ScopedVar::set("UNIFI_API_KEY", "key-from-the-environment");

        let args = parse_args_for_test(["ufa", "info"]).expect("ufa info must parse");

        assert_eq!(
            args.api_key.as_ref().map(Secret::expose),
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

            let args = parse_args_for_test(["ufa", "info"]).expect("ufa info must parse");

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

        let error = parse_args_for_test(["ufa", "info"])
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

        let args = parse_args_for_test(["ufa", "cloud", "hosts"]).expect("ufa cloud hosts parses");

        let Commands::Cloud {
            site_manager_api_key,
            ..
        } = args.command
        else {
            panic!("`ufa cloud hosts` must parse as the cloud command");
        };
        assert_eq!(
            site_manager_api_key.as_ref().map(Secret::expose),
            Some("cloud-key-from-the-environment"),
            "UNIFI_SITE_MANAGER_API_KEY must reach --site-manager-api-key"
        );
    }

    /// An explicit flag still beats the environment.
    #[test]
    fn a_command_line_flag_overrides_the_environment() {
        let _var = ScopedVar::set("UNIFI_URL", "https://from-the-environment");

        let args = parse_args_for_test(["ufa", "--url", "https://from-the-flag", "info"])
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
    use super::{Args, Commands, Secret};
    use crate::output::OutputFormat;
    use crate::test_support::parse_args_for_test;

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
        parse_args_for_test(argv)
            .unwrap_or_else(|error| panic!("`{}` must parse, got {error}", argv.join(" ")))
    }

    fn is_json(format: OutputFormat) -> bool {
        matches!(format, OutputFormat::Json)
    }

    #[test]
    fn output_is_accepted_after_the_subcommand() {
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
        let args = parse(&["ufa", "--output", "json", "devices", "list", "--limit", "5"]);

        assert!(
            is_json(args.output),
            "a leading --output must survive a subcommand that takes further flags"
        );
    }

    #[test]
    fn output_defaults_to_table_in_both_positions() {
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
        assert_eq!(
            args.api_key.as_ref().map(Secret::expose),
            Some("trailing-key")
        );
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

/// Pins the claim that each list command reads the whole collection when the
/// user gives `--all`, and refuses `--all` beside a flag that names one page.
#[cfg(test)]
mod all_pages_tests {
    use crate::client::UnifiClient;
    use crate::test_server::{json_response, Answer, TestServer};
    use crate::test_support::parse_args_for_test;

    /// The filter the walk must send with each request. It holds only ASCII
    /// letters, so the query does not percent-encode it.
    const FILTER: &str = "nameMatches";

    /// The site the argv of each case names, so no site lookup is needed.
    const SITE: &str = "00000000-0000-0000-0000-0000000000aa";

    /// The API key a test hands the client. Nothing reads it back.
    const API_KEY: &str = "an-api-key";

    /// One list command: its name, its argv, whether the argv holds the
    /// filter, and one item of its collection as the API writes it.
    struct Case {
        name: &'static str,
        argv: Vec<&'static str>,
        filtered: bool,
        item: &'static str,
    }

    const SITE_ITEM: &str = r#"{"id":"00000000-0000-0000-0000-000000000004","internalReference":"default","name":"Home"}"#;
    const DEVICE_ITEM: &str = r#"{"id":"00000000-0000-0000-0000-000000000001","name":"ap-lr","model":"U6-LR","macAddress":"00:11:22:33:44:55","ipAddress":"192.168.1.2","state":"ONLINE","features":[],"interfaces":[]}"#;
    const CLIENT_ITEM: &str = r#"{"type":"WIRED","id":"00000000-0000-0000-0000-000000000002","name":"nas","ipAddress":"192.168.1.10","macAddress":"00:11:22:33:44:10","uplinkDeviceId":"00000000-0000-0000-0000-000000000003","access":{"type":"DEFAULT"}}"#;
    const VOUCHER_ITEM: &str = r#"{"id":"00000000-0000-0000-0000-000000000005","createdAt":"2026-07-21T00:00:00Z","name":"lobby","code":"1234567890","authorizedGuestCount":0,"expired":false,"timeLimitMinutes":60}"#;

    /// The four list commands, each with `--all`.
    fn cases() -> Vec<Case> {
        vec![
            Case {
                name: "sites",
                argv: vec!["ufa", "sites", "--all", "--filter", FILTER],
                filtered: true,
                item: SITE_ITEM,
            },
            Case {
                name: "devices",
                argv: vec!["ufa", "devices", "--site-id", SITE, "list", "--all"],
                filtered: false,
                item: DEVICE_ITEM,
            },
            Case {
                name: "clients",
                argv: vec![
                    "ufa",
                    "clients",
                    "--site-id",
                    SITE,
                    "list",
                    "--all",
                    "--filter",
                    FILTER,
                ],
                filtered: true,
                item: CLIENT_ITEM,
            },
            Case {
                name: "vouchers",
                argv: vec![
                    "ufa",
                    "vouchers",
                    "--site-id",
                    SITE,
                    "list",
                    "--all",
                    "--filter",
                    FILTER,
                ],
                filtered: true,
                item: VOUCHER_ITEM,
            },
        ]
    }

    /// Run one case against a two-page collection of three items.
    ///
    /// # Returns
    ///
    /// The problem with the run, or `None` when the run was right.
    async fn problem_with(case: &Case) -> Option<String> {
        let item = case.item;
        let first = format!(
            r#"{{"offset":0,"limit":200,"count":2,"totalCount":3,"data":[{item},{item}]}}"#
        );
        let second =
            format!(r#"{{"offset":2,"limit":200,"count":1,"totalCount":3,"data":[{item}]}}"#);
        let routes: Vec<(String, Answer)> = vec![
            ("offset=0".to_string(), json_response(&first).into()),
            ("offset=2".to_string(), json_response(&second).into()),
        ];
        let server = TestServer::routing(&routes, &json_response("{}")).await;
        let client = UnifiClient::new(server.origin(), API_KEY, false).expect("a loopback URL");

        let args = match parse_args_for_test(&case.argv) {
            Ok(args) => args,
            Err(error) => {
                return Some(format!("`{}` must parse, got {error}", case.argv.join(" ")))
            }
        };
        if let Err(error) = crate::run_controller_command(args.command, &client, args.output).await
        {
            return Some(format!("the command failed: {error:#}"));
        }

        let lines: Vec<String> = server
            .requests()
            .iter()
            .map(|request| request.request_line().to_string())
            .filter(|line| line.contains("offset="))
            .collect();
        let offsets_are_right = matches!(
            lines.as_slice(),
            [first, second] if first.contains("offset=0") && second.contains("offset=2")
        );
        if !offsets_are_right {
            return Some(format!(
                "expected a GET with offset=0, then one with offset=2, got {lines:?}"
            ));
        }
        let wanted = format!("filter={FILTER}");
        if case.filtered && lines.iter().any(|line| !line.contains(&wanted)) {
            return Some(format!("each GET must hold {wanted}, got {lines:?}"));
        }
        None
    }

    /// `--all` on each list command walks every page: a GET with offset 0,
    /// then a GET with the offset where the first page ended, and the filter
    /// rides on each GET.
    #[tokio::test]
    async fn all_reads_every_page_of_each_list_command() {
        let mut failures = Vec::new();
        for case in cases() {
            if let Some(problem) = problem_with(&case).await {
                failures.push(format!("{}: {problem}", case.name));
            }
        }
        assert!(failures.is_empty(), "failed cases: {failures:#?}");
    }

    /// `--all` beside `--limit` or `--offset` is a usage error of the kind
    /// that names two arguments that exclude each other.
    #[test]
    fn all_conflicts_with_limit_and_offset_on_each_list_command() {
        let mut failures = Vec::new();
        for case in cases() {
            for extra in [["--limit", "5"], ["--offset", "5"]] {
                let mut argv = case.argv.clone();
                argv.extend(extra);
                match parse_args_for_test(&argv) {
                    Err(error) if error.kind() == clap::error::ErrorKind::ArgumentConflict => {}
                    Err(error) => failures.push(format!(
                        "`{}`: expected ArgumentConflict, got {:?}",
                        argv.join(" "),
                        error.kind()
                    )),
                    Ok(_) => failures.push(format!("`{}` must not parse", argv.join(" "))),
                }
            }
        }
        assert!(failures.is_empty(), "failed cases: {failures:#?}");
    }
}

/// Pins the claim that a run with certificate verification off says so,
/// whatever turned it off.
///
/// The wizard asks about the setting once. Every other route to it is silent
/// unless the notice follows the *resolved* value, so each source below gets
/// its own case: the flag, the environment variable, and the configuration
/// file. A notice that only one of them earns is the defect, not the fix.
#[cfg(test)]
mod tls_warning_tests {
    use super::{resolve_insecure, Config, INSECURE_WARNING};
    use crate::test_support::{parse_args_for_test, ScopedVar};

    /// Resolve `supplied` against `config` and hand back what the user is
    /// told.
    ///
    /// # Arguments
    ///
    /// * `supplied` - What the command line and the environment supplied.
    /// * `config` - The configuration file, when the case has one.
    ///
    /// # Returns
    ///
    /// Whether verification is skipped, and everything written for the user.
    fn resolve_and_capture(supplied: Option<bool>, config: Option<&Config>) -> (bool, String) {
        let mut warnings = Vec::new();

        let insecure = resolve_insecure(supplied, config, &mut warnings)
            .expect("a warning written to a buffer must succeed");

        (
            insecure,
            String::from_utf8(warnings).expect("the warning must be text"),
        )
    }

    /// A configuration file that turns verification off.
    fn config_with_verification_off() -> Config {
        Config {
            insecure: Some(true),
            ..Config::default()
        }
    }

    #[test]
    fn the_flag_earns_the_warning() {
        let args = parse_args_for_test(["ufa", "info", "--insecure", "true"])
            .expect("--insecure true must parse");

        let (insecure, warnings) = resolve_and_capture(args.insecure, None);

        assert!(insecure, "--insecure true must turn verification off");
        assert!(
            warnings.contains(INSECURE_WARNING),
            "--insecure must earn the warning, got {warnings:?}"
        );
    }

    #[test]
    fn the_environment_variable_earns_the_warning() {
        let _var = ScopedVar::set("UNIFI_INSECURE", "yes");
        let args = parse_args_for_test(["ufa", "info"]).expect("ufa info must parse");

        let (insecure, warnings) = resolve_and_capture(args.insecure, None);

        assert!(insecure, "UNIFI_INSECURE=yes must turn verification off");
        assert!(
            warnings.contains(INSECURE_WARNING),
            "UNIFI_INSECURE must earn the warning, got {warnings:?}"
        );
    }

    /// The quietest route of the three, and the one that lasts: a choice made
    /// once during setup sits in the configuration file and applies to every
    /// run after it.
    #[test]
    fn the_configuration_file_earns_the_warning() {
        let (insecure, warnings) = resolve_and_capture(None, Some(&config_with_verification_off()));

        assert!(
            insecure,
            "`insecure = true` in the configuration file must turn verification off"
        );
        assert!(
            warnings.contains(INSECURE_WARNING),
            "the configuration file must earn the warning, got {warnings:?}"
        );
    }

    /// The notice follows the resolved value and nothing else, so a flag that
    /// turns verification back on takes the notice with it.
    #[test]
    fn a_flag_that_overrides_the_configuration_file_takes_the_warning_with_it() {
        let (insecure, warnings) =
            resolve_and_capture(Some(false), Some(&config_with_verification_off()));

        assert!(
            !insecure,
            "--insecure false must beat the configuration file"
        );
        assert!(
            warnings.is_empty(),
            "a verified connection must say nothing, got {warnings:?}"
        );
    }

    #[test]
    fn a_verified_connection_says_nothing() {
        for (supplied, config) in [
            (None, None),
            (Some(false), None),
            (None, Some(Config::default())),
            (
                None,
                Some(Config {
                    insecure: Some(false),
                    ..Config::default()
                }),
            ),
        ] {
            let (insecure, warnings) = resolve_and_capture(supplied, config.as_ref());

            assert!(!insecure, "verification must stay on");
            assert!(
                warnings.is_empty(),
                "a verified connection must say nothing, got {warnings:?}"
            );
        }
    }

    /// The user has to understand the risk, so the notice names both halves of
    /// it: the check that is off, and what travels over the connection that
    /// nobody checked.
    #[test]
    fn the_warning_names_the_check_and_the_key() {
        assert!(
            INSECURE_WARNING.contains("certificate verification is off"),
            "the notice must say the check is off, got {INSECURE_WARNING:?}"
        );
        assert!(
            INSECURE_WARNING.contains("API key"),
            "the notice must say what travels over the connection, got {INSECURE_WARNING:?}"
        );
    }
}

#[cfg(test)]
mod credential_tests {
    use super::{resolve_credential, CachedOnePassword, Config, Credential};
    use crate::test_support::{parse_args_for_test, ScopedVar};
    use crate::test_vault::{TempConfigDir, TestVault};

    /// The store for a test whose key does not come from 1Password.
    ///
    /// The vault cannot be read and no cache file exists, so a test that
    /// reaches the store by mistake gets an error, and no call to `op`.
    fn no_1password() -> TestVault {
        TestVault::new("no-1password", &[], None)
    }

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
        let temp = TempConfigDir::new("unreadable-reference");
        let keys = CachedOnePassword::in_directory(&temp.dir);

        let error = resolve_credential(Credential::Controller, None, Some(&config), &keys)
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
        let error = resolve_credential(Credential::Controller, None, None, &no_1password())
            .expect_err("no key anywhere must not resolve");

        assert!(
            format!("{error:#}").contains("ufa config setup"),
            "an unconfigured key must point at setup, got {error:#}"
        );

        let error = resolve_credential(
            Credential::SiteManager,
            None,
            Some(&Config::default()),
            &no_1password(),
        )
        .expect_err("an empty config must not resolve a cloud key");
        assert!(
            format!("{error:#}").contains("ufa config cloud"),
            "an unconfigured cloud key must point at cloud setup, got {error:#}"
        );
    }

    /// A config that holds a real key in its plaintext field.
    fn config_with_a_plaintext_key() -> Config {
        Config {
            api_key: Some("key-from-the-configuration-file".into()),
            ..Config::default()
        }
    }

    /// `export UNIFI_API_KEY=` and `--api-key ""` say nothing, they do not say
    /// "authenticate with an empty key". A blank value used to win over the
    /// configuration file and go to the controller, which answered 401.
    #[test]
    fn a_blank_supplied_key_falls_through_to_the_configuration_file() {
        for blank in ["", "   ", "\n"] {
            assert_eq!(
                resolve_credential(
                    Credential::Controller,
                    Some(blank.into()),
                    Some(&config_with_a_plaintext_key()),
                    &no_1password(),
                )
                .expect("a blank command line value must fall through to the file")
                .key()
                .expose(),
                "key-from-the-configuration-file",
                "{blank:?} names no key, so the configured key must answer"
            );
        }
    }

    /// The same value, as clap reads it out of the environment.
    #[test]
    fn a_blank_environment_key_falls_through_to_the_configuration_file() {
        for blank in ["", "   "] {
            let _var = ScopedVar::set("UNIFI_API_KEY", blank);
            let args = parse_args_for_test(["ufa", "info"]).expect("ufa info must parse");

            assert_eq!(
                resolve_credential(
                    Credential::Controller,
                    args.api_key,
                    Some(&config_with_a_plaintext_key()),
                    &no_1password(),
                )
                .expect("a blank UNIFI_API_KEY must fall through to the file")
                .key()
                .expose(),
                "key-from-the-configuration-file",
                "UNIFI_API_KEY={blank:?} names no key, so the configured key must answer"
            );
        }
    }

    /// With nothing configured either, the user gets the crate's own advice
    /// rather than a 401 from the controller.
    #[test]
    fn a_blank_supplied_key_with_nothing_configured_reports_the_advice() {
        let error = resolve_credential(
            Credential::Controller,
            Some("  ".into()),
            None,
            &no_1password(),
        )
        .expect_err("a blank key and no configuration must not resolve");

        assert!(
            format!("{error:#}").contains("ufa config setup"),
            "a blank key must earn the advice a missing one earns, got {error:#}"
        );
    }

    /// The emptiness test trims. The value does not: a key the user supplied
    /// is theirs, and altering what goes to the controller would make a
    /// working key fail for a reason nothing states.
    #[test]
    fn a_supplied_key_that_holds_more_than_blank_space_is_passed_through_untouched() {
        assert_eq!(
            resolve_credential(
                Credential::Controller,
                Some(" padded-key\n".into()),
                Some(&config_with_a_plaintext_key()),
                &no_1password(),
            )
            .expect("a key that is not blank must resolve")
            .key()
            .expose(),
            " padded-key\n",
            "the supplied key must reach the controller byte for byte"
        );
    }

    /// What the command line supplied — the flag itself, or the UNIFI_* value
    /// clap resolved for it — beats the config file, and beats it without
    /// touching 1Password: a config whose reference cannot be read still
    /// resolves.
    #[test]
    fn a_supplied_key_wins_over_the_config_file() {
        let config = config_with_unreadable_reference();
        let temp = TempConfigDir::new("supplied-wins");
        let keys = CachedOnePassword::in_directory(&temp.dir);

        assert_eq!(
            resolve_credential(
                Credential::Controller,
                Some("from-the-command-line".into()),
                Some(&config),
                &keys,
            )
            .expect("a supplied key must resolve")
            .key()
            .expose(),
            "from-the-command-line"
        );
    }
}

#[cfg(test)]
mod url_tests {
    use super::{resolve_url, Config, MISSING_URL_MESSAGE};
    use crate::test_support::{parse_args_for_test, ScopedVar};

    /// The URL that the configuration file of these tests names.
    const CONFIGURED_URL: &str = "https://from-the-configuration-file";

    /// A configuration file that names [`CONFIGURED_URL`].
    fn config_with_a_url() -> Config {
        Config {
            url: Some(CONFIGURED_URL.to_string()),
            ..Config::default()
        }
    }

    /// `export UNIFI_URL=` and `--url ""` say nothing, as a blank key does. A
    /// blank value used to win over the configuration file and fail as a URL
    /// that cannot be parsed.
    #[test]
    fn a_blank_supplied_url_falls_through_to_the_configuration_file() {
        for blank in ["", "   ", "\n"] {
            assert_eq!(
                resolve_url(Some(blank.to_string()), Some(&config_with_a_url()))
                    .expect("a blank command line value must fall through to the file"),
                CONFIGURED_URL,
                "{blank:?} names no URL, so the configured URL must answer"
            );
        }
    }

    /// The same value, as clap reads it out of the environment.
    #[test]
    fn a_blank_environment_url_falls_through_to_the_configuration_file() {
        for blank in ["", "   "] {
            let _var = ScopedVar::set("UNIFI_URL", blank);
            let args = parse_args_for_test(["ufa", "info"]).expect("ufa info must parse");

            assert_eq!(
                resolve_url(args.url, Some(&config_with_a_url()))
                    .expect("a blank UNIFI_URL must fall through to the file"),
                CONFIGURED_URL,
                "UNIFI_URL={blank:?} names no URL, so the configured URL must answer"
            );
        }
    }

    /// A blank `url` field names no URL either, so the user gets the advice
    /// rather than an error about a URL that cannot be parsed.
    #[test]
    fn a_blank_url_field_with_nothing_supplied_reports_the_advice() {
        let config = Config {
            url: Some("  ".to_string()),
            ..Config::default()
        };

        let error = resolve_url(None, Some(&config))
            .expect_err("a blank field and no supplied URL must not resolve");

        assert_eq!(format!("{error:#}"), MISSING_URL_MESSAGE);
    }

    /// A supplied URL that holds more than blank space wins over the file.
    #[test]
    fn a_supplied_url_wins_over_the_configuration_file() {
        assert_eq!(
            resolve_url(
                Some("https://supplied.example".to_string()),
                Some(&config_with_a_url())
            )
            .expect("a URL that is not blank must resolve"),
            "https://supplied.example"
        );
    }
}

/// Pins the claim that a key which 1Password rotated after op-cache kept a
/// copy of it costs the user no manual step.
///
/// op-cache answers every read of a reference from its copy, and it asks
/// 1Password only when it holds no copy. So after a rotation in 1Password,
/// every command sent the old key and failed on a 401. The error said to check
/// the key or to make a new one, which is what the user had just done.
///
/// Each case runs the real client against a local server, and the key reaches
/// the server in the real header. The store is a test vault in front of a
/// real cache file, so each forget runs the real `OpCache::invalidate` and no
/// read starts the real `op`.
#[cfg(test)]
mod key_rotation_tests {
    use super::{resolve_credential, run_with_the_current_key, Config, Credential, Secret};
    use crate::client::UnifiClient;
    use crate::config::ResolvedKey;
    use crate::http::answered_status;
    use crate::models::ApplicationInfo;
    use crate::site_manager::SiteManagerClient;
    use crate::test_server::{json_response, json_response_with_status, TestServer};
    use crate::test_vault::TestVault;
    use reqwest::StatusCode;
    use std::cell::Cell;

    /// The 1Password reference of the controller key.
    const REFERENCE: &str = "op://Private/ufa/key - 192.168.1.1 port 443";

    /// The 1Password reference of the Site Manager key.
    const SITE_MANAGER_REFERENCE: &str = "op://Private/ufa/site manager key";

    /// The key that 1Password held before the rotation. The cache file keeps
    /// it, and the API refuses it.
    const OLD_KEY: &str = "old-key-from-before-the-rotation-7Hc2Qx";

    /// The key that 1Password holds after the rotation. The API takes it.
    const NEW_KEY: &str = "new-key-from-after-the-rotation-3Lp9Vz";

    /// The header that carries the key. Stated here rather than read from the
    /// production constant, so a renamed header fails this test.
    const API_KEY_HEADER: &str = "x-api-key";

    /// An answer that refuses the key, with `status`.
    fn refusal(status: &str) -> String {
        json_response_with_status(status, r#"{"message":"refused"}"#)
    }

    /// The answer of a controller that takes the key.
    fn application_info() -> String {
        json_response(r#"{"applicationVersion":"9.3.45"}"#)
    }

    /// The answer of a Site Manager API that takes the key.
    fn no_hosts() -> String {
        json_response(
            r#"{"data":[],"httpStatusCode":200,"traceId":"a7dc15e0eb4527142d7823515b15f87d"}"#,
        )
    }

    /// The configuration of a user who keeps the controller key in 1Password.
    fn config_with_the_reference() -> Config {
        Config {
            op_path: Some(REFERENCE.to_string()),
            ..Config::default()
        }
    }

    /// What one command gave.
    struct Outcome {
        /// What the command returned in the end.
        result: anyhow::Result<()>,
        /// How many times the command ran.
        runs: usize,
        /// Everything written for the user.
        notes: String,
    }

    /// Which API a case sends its request to.
    #[derive(Clone, Copy)]
    enum Target {
        /// The controller: one request for its application info.
        Controller,
        /// The Site Manager API: one request for the host listing.
        SiteManager,
    }

    /// Run one command with `resolved`, the way `main` runs it.
    ///
    /// The command sends the new key to `accepting`, and any other key to
    /// `refusing`. That is how a real API behaves after a rotation: it takes
    /// the new key and refuses every other one.
    ///
    /// # Arguments
    ///
    /// * `target` - Which API the command talks to.
    /// * `credential` - The credential the command sends.
    /// * `resolved` - The key the command starts with.
    /// * `vault` - The store that reads the key again.
    /// * `refusing` - The server that gets every key but the new one.
    /// * `accepting` - The server that gets the new key.
    ///
    /// # Returns
    ///
    /// What the command gave.
    async fn run_command(
        target: Target,
        credential: Credential,
        resolved: ResolvedKey,
        vault: &TestVault,
        refusing: &TestServer,
        accepting: &TestServer,
    ) -> Outcome {
        let runs = Cell::new(0_usize);
        let mut notes = Vec::new();

        let result =
            run_with_the_current_key(credential, resolved, vault, &mut notes, |key: Secret| {
                runs.set(runs.get() + 1);
                let origin = if key.expose() == NEW_KEY {
                    accepting.origin()
                } else {
                    refusing.origin()
                };

                async move {
                    match target {
                        Target::Controller => {
                            let client = UnifiClient::new(origin, key.expose(), false)?;
                            client.get::<ApplicationInfo>("info").await.map(|_| ())
                        }
                        Target::SiteManager => SiteManagerClient::talking_to(origin, key.expose())
                            .get_hosts()
                            .await
                            .map(|_| ()),
                    }
                }
            })
            .await;

        Outcome {
            result,
            runs: runs.get(),
            notes: String::from_utf8(notes).expect("the notes are text"),
        }
    }

    /// Assert that one server got exactly one request, and that the request
    /// carried `key`.
    fn assert_one_request_with(server: &TestServer, key: &str, which: &str) {
        let requests = server.requests();

        assert_eq!(
            requests.len(),
            1,
            "{which} must get exactly one request, got {requests:?}"
        );
        assert_eq!(
            requests[0].header(API_KEY_HEADER),
            Some(key),
            "the request to {which} must carry {key:?}"
        );
    }

    /// Assert that the user got exactly one note, that the note says what
    /// happened, and that it carries no key.
    fn assert_one_note_about_the_new_key(notes: &str) {
        assert_eq!(
            notes.lines().count(),
            1,
            "the user must get exactly one note, got {notes:?}"
        );
        assert!(
            notes.contains("1Password"),
            "the note must say that the key in 1Password changed, got {notes:?}"
        );
        assert!(
            notes.contains("again"),
            "the note must say that the command runs again, got {notes:?}"
        );
        assert!(
            !notes.contains(OLD_KEY) && !notes.contains(NEW_KEY),
            "the note must carry no key, got {notes:?}"
        );
    }

    /// Resolve the controller key the way `main` does, with no key on the
    /// command line.
    fn resolve_from_the_reference(vault: &TestVault) -> ResolvedKey {
        let resolved = resolve_credential(
            Credential::Controller,
            None,
            Some(&config_with_the_reference()),
            vault,
        )
        .expect("the cached key must resolve");

        assert_eq!(
            resolved.key().expose(),
            OLD_KEY,
            "the command must start with the copy in the cache file"
        );
        resolved
    }

    /// The whole case of a rotation that the controller refuses with
    /// `status`.
    ///
    /// # Arguments
    ///
    /// * `label` - A word in the name of the directory of the vault.
    /// * `status` - The status line of the refusal.
    async fn a_rotated_key_refused_with(label: &str, status: &str) {
        let vault = TestVault::new(label, &[(REFERENCE, OLD_KEY)], Some(NEW_KEY));
        let refusing = TestServer::replying(&refusal(status)).await;
        let accepting = TestServer::replying(&application_info()).await;

        let resolved = resolve_from_the_reference(&vault);
        let outcome = run_command(
            Target::Controller,
            Credential::Controller,
            resolved,
            &vault,
            &refusing,
            &accepting,
        )
        .await;

        if let Err(error) = &outcome.result {
            panic!("after {status} the command must run again with the new key and pass, got {error:#}");
        }
        assert_eq!(
            outcome.runs, 2,
            "the command must run once more, and no more"
        );
        assert_one_request_with(&refusing, OLD_KEY, "the refusing controller");
        assert_one_request_with(&accepting, NEW_KEY, "the accepting controller");
        assert_eq!(vault.forgets(), 1, "the cached copy must be forgotten once");
        assert!(
            vault.caches(REFERENCE),
            "the cache file must hold the copy that 1Password gave"
        );
        assert!(
            vault.cache_file().contains(NEW_KEY),
            "the cache file must hold the new key"
        );
        assert!(
            !vault.cache_file().contains(OLD_KEY),
            "the cache file must not hold the key from before the rotation"
        );
        assert_one_note_about_the_new_key(&outcome.notes);
    }

    /// The case the user met: the key changed in 1Password, the copy did not,
    /// and the controller refused the copy with a 401.
    #[tokio::test]
    async fn a_key_that_1password_rotated_is_read_again_after_a_401() {
        a_rotated_key_refused_with("rotated-401", "401 Unauthorized").await;
    }

    /// A 403 refuses the key too. The controller answers 403 to a key it
    /// does not accept for the request.
    #[tokio::test]
    async fn a_key_that_1password_rotated_is_read_again_after_a_403() {
        a_rotated_key_refused_with("rotated-403", "403 Forbidden").await;
    }

    /// The Site Manager key comes from a reference of its own, and the Site
    /// Manager API refuses it the same way.
    #[tokio::test]
    async fn a_site_manager_key_that_1password_rotated_is_read_again() {
        let vault = TestVault::new(
            "rotated-site-manager",
            &[(SITE_MANAGER_REFERENCE, OLD_KEY)],
            Some(NEW_KEY),
        );
        let refusing = TestServer::replying(&refusal("401 Unauthorized")).await;
        let accepting = TestServer::replying(&no_hosts()).await;
        let config = Config {
            sm_op_path: Some(SITE_MANAGER_REFERENCE.to_string()),
            ..Config::default()
        };

        let resolved = resolve_credential(Credential::SiteManager, None, Some(&config), &vault)
            .expect("the cached Site Manager key must resolve");
        let outcome = run_command(
            Target::SiteManager,
            Credential::SiteManager,
            resolved,
            &vault,
            &refusing,
            &accepting,
        )
        .await;

        if let Err(error) = &outcome.result {
            panic!("the cloud command must run again with the new key and pass, got {error:#}");
        }
        assert_eq!(outcome.runs, 2, "the cloud command must run once more");
        assert_one_request_with(&refusing, OLD_KEY, "the refusing Site Manager API");
        assert_one_request_with(&accepting, NEW_KEY, "the accepting Site Manager API");
        assert!(
            vault.caches(SITE_MANAGER_REFERENCE),
            "the cache file must hold the copy that 1Password gave"
        );
        assert!(
            vault.cache_file().contains(NEW_KEY),
            "the cache file must hold the new key"
        );
        assert!(
            !vault.cache_file().contains(OLD_KEY),
            "the cache file must not hold the key from before the rotation"
        );
        assert_one_note_about_the_new_key(&outcome.notes);
    }

    /// 1Password still holds the key that the API refused. The key is wrong,
    /// not stale, and a second run gets the same answer, so the user gets the
    /// first error. The cache file holds the copy that 1Password just gave, which
    /// is the same key.
    #[tokio::test]
    async fn the_same_key_from_1password_gives_the_first_error_back() {
        let vault = TestVault::new("same-key", &[(REFERENCE, OLD_KEY)], Some(OLD_KEY));
        let refusing = TestServer::replying(&refusal("401 Unauthorized")).await;
        let accepting = TestServer::replying(&application_info()).await;

        let resolved = resolve_from_the_reference(&vault);
        let outcome = run_command(
            Target::Controller,
            Credential::Controller,
            resolved,
            &vault,
            &refusing,
            &accepting,
        )
        .await;

        let error = outcome
            .result
            .expect_err("a key that 1Password still holds is refused again");
        assert_eq!(
            answered_status(&error),
            Some(StatusCode::UNAUTHORIZED),
            "the user must get the first refusal, got {error:#}"
        );
        assert_eq!(outcome.runs, 1, "the same key must not be sent again");
        assert_eq!(vault.forgets(), 1, "the cached copy must be forgotten");
        assert!(
            vault.caches(REFERENCE),
            "the cache file must hold the copy that 1Password just gave"
        );
        assert!(
            vault.cache_file().contains(OLD_KEY),
            "the copy that 1Password gave is the same key"
        );
        assert_eq!(
            vault.vault_reads(),
            1,
            "the key must be read from 1Password once"
        );
        assert!(
            accepting.requests().is_empty(),
            "no request may reach the accepting controller"
        );
        assert!(
            outcome.notes.is_empty(),
            "nothing changed, so there is nothing to note, got {:?}",
            outcome.notes
        );
    }

    /// 1Password cannot be read after the refusal. The user gets the first
    /// error, and one note that says why the key was not read again.
    #[tokio::test]
    async fn a_second_read_that_fails_gives_the_first_error_back_with_a_note() {
        let vault = TestVault::new("unreadable-again", &[(REFERENCE, OLD_KEY)], None);
        let refusing = TestServer::replying(&refusal("401 Unauthorized")).await;
        let accepting = TestServer::replying(&application_info()).await;

        let resolved = resolve_from_the_reference(&vault);
        let outcome = run_command(
            Target::Controller,
            Credential::Controller,
            resolved,
            &vault,
            &refusing,
            &accepting,
        )
        .await;

        let error = outcome
            .result
            .expect_err("a key that cannot be read again gives no second run");
        assert_eq!(
            answered_status(&error),
            Some(StatusCode::UNAUTHORIZED),
            "the user must get the first refusal, got {error:#}"
        );
        assert_eq!(outcome.runs, 1, "the command must not run again");
        assert_eq!(
            vault.vault_reads(),
            1,
            "the key must be read from 1Password once"
        );
        assert_eq!(
            outcome.notes.lines().count(),
            1,
            "the user must get exactly one note, got {:?}",
            outcome.notes
        );
        assert!(
            outcome.notes.contains("cannot be read"),
            "the note must give the reason the second read failed, got {:?}",
            outcome.notes
        );
    }

    /// A key that came from no reference cannot be read again: nothing names
    /// where it lives. The user gets the refusal, and 1Password is not asked,
    /// even when the configuration file names a reference.
    #[tokio::test]
    async fn a_key_that_came_from_no_reference_is_not_read_again() {
        let supplied = || -> ResolvedKey {
            // A config file that names a reference: the supplied key wins.
            let vault = TestVault::new("unused", &[], None);
            resolve_credential(
                Credential::Controller,
                Some(OLD_KEY.into()),
                Some(&config_with_the_reference()),
                &vault,
            )
            .expect("a supplied key must resolve")
        };
        let from_the_plaintext_field = || -> ResolvedKey {
            let vault = TestVault::new("unused", &[], None);
            let config = Config {
                api_key: Some(OLD_KEY.into()),
                ..Config::default()
            };
            resolve_credential(Credential::Controller, None, Some(&config), &vault)
                .expect("a plaintext key must resolve")
        };

        for (case, resolved) in [
            ("a key from the command line", supplied()),
            ("a key from the plaintext field", from_the_plaintext_field()),
        ] {
            let vault = TestVault::new("no-reference", &[(REFERENCE, OLD_KEY)], Some(NEW_KEY));
            let refusing = TestServer::replying(&refusal("401 Unauthorized")).await;
            let accepting = TestServer::replying(&application_info()).await;

            let outcome = run_command(
                Target::Controller,
                Credential::Controller,
                resolved,
                &vault,
                &refusing,
                &accepting,
            )
            .await;

            let error = outcome
                .result
                .expect_err("a refused key that no reference names stays refused");
            assert_eq!(
                answered_status(&error),
                Some(StatusCode::UNAUTHORIZED),
                "{case}: the user must get the refusal, got {error:#}"
            );
            assert_eq!(outcome.runs, 1, "{case}: the command must not run again");
            assert_eq!(vault.forgets(), 0, "{case}: nothing may be forgotten");
            assert_eq!(
                vault.vault_reads(),
                0,
                "{case}: 1Password must not be asked"
            );
            assert!(
                vault.caches(REFERENCE),
                "{case}: the cache file must keep what it holds"
            );
            assert!(
                outcome.notes.is_empty(),
                "{case}: there is nothing to note, got {:?}",
                outcome.notes
            );
        }
    }

    /// Only a refusal of the key says the key is stale. A server fault, and
    /// a request that got no answer at all, say nothing about the key, so
    /// 1Password is not asked and the cache file keeps its copy.
    #[tokio::test]
    async fn a_failure_that_is_not_a_refusal_does_not_touch_1password() {
        let server_fault = TestServer::replying(&refusal("500 Internal Server Error")).await;
        let not_found = TestServer::replying(&refusal("404 Not Found")).await;
        let hanging_up = TestServer::hanging_up().await;

        for (case, failing) in [
            ("a 500", &server_fault),
            ("a 404", &not_found),
            ("no answer", &hanging_up),
        ] {
            let vault = TestVault::new("not-a-refusal", &[(REFERENCE, OLD_KEY)], Some(NEW_KEY));
            let accepting = TestServer::replying(&application_info()).await;

            let resolved = resolve_from_the_reference(&vault);
            let outcome = run_command(
                Target::Controller,
                Credential::Controller,
                resolved,
                &vault,
                failing,
                &accepting,
            )
            .await;

            assert!(outcome.result.is_err(), "{case}: the command must fail");
            assert_eq!(outcome.runs, 1, "{case}: the command must not run again");
            assert_eq!(vault.forgets(), 0, "{case}: nothing may be forgotten");
            assert_eq!(
                vault.vault_reads(),
                0,
                "{case}: 1Password must not be asked"
            );
            assert!(
                vault.caches(REFERENCE),
                "{case}: the cache file must keep its copy"
            );
            assert!(
                outcome.notes.is_empty(),
                "{case}: there is nothing to note, got {:?}",
                outcome.notes
            );
        }
    }
}

/// Pins the claim that no credential this crate holds reaches a debug dump.
///
/// Nothing formats any of these types with `{:?}` today, so nothing leaks
/// today. That is the whole hazard: the redaction has to be a property of the
/// types, because the leak arrives as one `eprintln!` somebody adds while
/// debugging, or as a panic message that carries the value with it, and
/// neither of those is reviewed against a rule nobody wrote down.
#[cfg(test)]
mod redaction_tests {
    use super::{Args, Commands};
    use crate::test_support::{
        parse_args_for_test, ScopedVar, FAKE_CLOUD_KEY, FAKE_CONTROLLER_KEY, REDACTED,
    };
    use clap::{Command, CommandFactory};

    /// The words that make the value of an environment variable a credential.
    ///
    /// Over-matching costs a flag one attribute and says so loudly. Matching
    /// too little reports clean, which is the answer that cannot be told from
    /// a guard doing real work.
    const CREDENTIAL_WORDS: [&str; 4] = ["KEY", "SECRET", "TOKEN", "PASSWORD"];

    /// Every flag of `command`, and of its subcommands, that reads a
    /// credential out of the environment without hiding what it read.
    ///
    /// The walk reads clap's own metadata rather than a list beside it, so a
    /// flag added tomorrow is covered without anybody remembering to write it
    /// down twice. The recursion is not decoration either:
    /// `UNIFI_SITE_MANAGER_API_KEY` is declared on `ufa cloud` rather than on
    /// the top-level command.
    fn credential_flags_that_show_their_value(command: &Command) -> Vec<String> {
        let mut found = Vec::new();
        collect_exposed_credential_flags(command, &mut found);
        found.sort();
        found
    }

    /// Add every offending flag of `command` and its subcommands to `found`.
    fn collect_exposed_credential_flags(command: &Command, found: &mut Vec<String>) {
        for argument in command.get_arguments() {
            let Some(setting) = argument.get_env() else {
                continue;
            };
            let setting = setting.to_string_lossy().into_owned();

            let holds_a_credential = CREDENTIAL_WORDS.iter().any(|word| setting.contains(word));
            if holds_a_credential && !argument.is_hide_env_values_set() {
                found.push(setting);
            }
        }
        for subcommand in command.get_subcommands() {
            collect_exposed_credential_flags(subcommand, found);
        }
    }

    /// `ufa --help` is the command a confused user runs, and the command whose
    /// output they paste into a bug report. clap prints the *current value* of
    /// every variable a flag reads beside that flag, so a user who exported
    /// their API key has it printed in full, with no debug build, no panic and
    /// no code change of any kind.
    #[test]
    fn the_help_text_does_not_print_the_api_key_from_the_environment() {
        let _var = ScopedVar::set("UNIFI_API_KEY", FAKE_CONTROLLER_KEY);

        let help = Args::command().render_long_help().to_string();

        assert!(
            !help.contains(FAKE_CONTROLLER_KEY),
            "the help text must not print the key the environment holds, got {help}"
        );
        assert!(
            help.contains("UNIFI_API_KEY"),
            "the help text must still name the variable the flag reads, got {help}"
        );
    }

    /// The cloud credential is declared on the subcommand, so `ufa cloud
    /// --help` is a second help text printing a second key.
    #[test]
    fn the_cloud_help_text_does_not_print_the_site_manager_key_from_the_environment() {
        let _var = ScopedVar::set("UNIFI_SITE_MANAGER_API_KEY", FAKE_CLOUD_KEY);

        let help = Args::command()
            .find_subcommand_mut("cloud")
            .expect("ufa must have a cloud subcommand")
            .render_long_help()
            .to_string();

        assert!(
            !help.contains(FAKE_CLOUD_KEY),
            "the cloud help text must not print the key the environment holds, got {help}"
        );
        assert!(
            help.contains("UNIFI_SITE_MANAGER_API_KEY"),
            "the cloud help text must still name the variable the flag reads, got {help}"
        );
    }

    /// The two tests above cover the two credential flags that exist today.
    /// This one covers the one somebody adds tomorrow, by asking clap which
    /// flags read a credential rather than trusting a list.
    #[test]
    fn every_credential_flag_hides_the_value_of_its_environment_variable() {
        let command = Args::command();

        assert!(
            credential_flags_that_show_their_value(&command).is_empty(),
            "these flags print the credential their variable holds into every \
             --help: {:?}",
            credential_flags_that_show_their_value(&command)
        );

        let probe = Command::new("probe").arg(
            clap::Arg::new("probe")
                .long("probe")
                .env("UNIFI_PROBE_API_KEY"),
        );
        assert_eq!(
            credential_flags_that_show_their_value(&probe),
            vec!["UNIFI_PROBE_API_KEY".to_string()],
            "the walk must report a credential flag that hides nothing, or it \
             reports clean for the wrong reason"
        );
    }

    /// The parsed arguments carry whatever `--api-key` or `UNIFI_API_KEY`
    /// supplied, and `Args` is the value most likely to be dumped whole: it is
    /// what `main` holds while it decides what to run.
    #[test]
    fn a_debug_dump_of_the_arguments_redacts_the_api_key() {
        let args = parse_args_for_test(["ufa", "--api-key", FAKE_CONTROLLER_KEY, "info"])
            .expect("ufa info must parse");

        let dump = format!("{args:?}");

        assert!(
            !dump.contains(FAKE_CONTROLLER_KEY),
            "the API key must not reach a debug dump, got {dump}"
        );
        assert!(
            dump.contains(REDACTED),
            "the API key must say {REDACTED} in place of what it holds, got {dump}"
        );
    }

    /// The cloud credential is declared on the subcommand rather than on the
    /// top-level command, so it is a second type holding a second key.
    #[test]
    fn a_debug_dump_of_the_cloud_command_redacts_the_site_manager_key() {
        let args = parse_args_for_test([
            "ufa",
            "cloud",
            "--site-manager-api-key",
            FAKE_CLOUD_KEY,
            "hosts",
        ])
        .expect("ufa cloud hosts must parse");

        let dump = format!("{:?}", args.command);

        assert!(
            matches!(args.command, Commands::Cloud { .. }),
            "the command under test must be the cloud one"
        );
        assert!(
            !dump.contains(FAKE_CLOUD_KEY),
            "the Site Manager key must not reach a debug dump, got {dump}"
        );
        assert!(
            dump.contains(REDACTED),
            "the Site Manager key must say {REDACTED} in place of what it holds, got {dump}"
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

#[cfg(test)]
mod help_text_tests {
    use super::Args;
    use clap::CommandFactory;

    /// The whole of what `ufa --help` says about the tool itself.
    ///
    /// Stated here rather than read from the `about` clap derives from the
    /// doc comment. A test that quotes the value under test proves only that
    /// the value equals itself. A reworded description changes what every
    /// user reads, so it is a deliberate edit to this line as well.
    const TOOL_DESCRIPTION: &str = "UniFi API CLI tool for managing UniFi Network applications";

    /// Where clap stops describing the tool and starts describing its use.
    const USAGE_HEADING: &str = "Usage:";

    /// Everything a rendered help says about the tool, before the usage line.
    ///
    /// # Arguments
    ///
    /// * `help` - A rendered help text.
    ///
    /// # Returns
    ///
    /// The description block, with the blank line after it removed.
    ///
    /// # Panics
    ///
    /// Panics if the help carries no usage line. The shape this reads would
    /// then have changed, and the assertions below would no longer read what
    /// they claim to read.
    fn description_block(help: &str) -> &str {
        let (description, _) = help
            .split_once(USAGE_HEADING)
            .unwrap_or_else(|| panic!("a rendered help must carry a usage line, got {help}"));

        description.trim()
    }

    /// `ufa --help` describes the tool and says nothing else.
    ///
    /// The note above `Args` records why the connection and output options
    /// are `global`. It is a plain comment because a second paragraph on
    /// `Args` becomes clap's `long_about`, and `--help` prints `long_about`
    /// in full. Nothing but that comment stands between the rationale and
    /// every user who asks for help.
    ///
    /// The assertion is an equality rather than a search for the sentence the
    /// note holds today. A search stops matching the day somebody rewords the
    /// note, and then passes for that reason forever. The rule is that the
    /// help describes the tool and carries no internal note of any wording,
    /// and an equality against the intended description is how that is
    /// written down.
    #[test]
    fn the_long_help_describes_the_tool_and_says_nothing_else() {
        let help = Args::command().render_long_help().to_string();

        assert_eq!(
            description_block(&help),
            TOOL_DESCRIPTION,
            "`ufa --help` must describe the tool and nothing else: a doc \
             comment on `Args` becomes clap's long_about, and internal \
             reasoning then reaches every user who asks for help"
        );
    }

    /// `ufa -h` is the other half, and clap treats the two differently: the
    /// short help prints `about` and the long help prints `long_about`. A
    /// test of one says nothing about the other.
    #[test]
    fn the_short_help_describes_the_tool_and_says_nothing_else() {
        let help = Args::command().render_help().to_string();

        assert_eq!(
            description_block(&help),
            TOOL_DESCRIPTION,
            "`ufa -h` must describe the tool and nothing else: everything the \
             first paragraph of the doc comment on `Args` holds is printed here"
        );
    }

    /// What `--help` says about `--site-id`.
    ///
    /// Stated here rather than read from the doc comment, for the reason that
    /// [`TOOL_DESCRIPTION`] gives.
    const SITE_ID_HELP: &str = "Site ID (if omitted: the only site, else a choice at a terminal)";

    /// The subcommands that take `--site-id`.
    const SITE_ID_SUBCOMMANDS: [&str; 3] = ["devices", "clients", "vouchers"];

    /// The help of `--site-id` states the rule for an omitted site.
    ///
    /// A controller with one site gives that site. A controller with several
    /// sites gives a choice only at a terminal, and a run with no terminal
    /// stops. Help that promises detection tells a script that it can omit
    /// the flag, and the script then stops on a controller with several sites.
    #[test]
    fn the_site_id_help_states_the_rule_for_an_omitted_site() {
        let command = Args::command();

        for name in SITE_ID_SUBCOMMANDS {
            let subcommand = command
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("ufa must have the subcommand {name}"));
            let site_id = subcommand
                .get_arguments()
                .find(|argument| argument.get_long() == Some("site-id"))
                .unwrap_or_else(|| panic!("ufa {name} must take --site-id"));
            let help = site_id
                .get_help()
                .map(ToString::to_string)
                .unwrap_or_default();

            assert_eq!(
                help, SITE_ID_HELP,
                "`ufa {name} --help` must state the rule for an omitted --site-id"
            );
        }
    }
}

#[cfg(test)]
mod config_path_tests {
    use super::config_path_report;
    use std::path::Path;

    /// A configuration file path that no test reads or writes.
    const A_CONFIG_PATH: &str = "/home/someone/.config/ufa/config.toml";

    /// `ufa config path` answers with the path and nothing else, so
    /// `$(ufa config path)` gives a shell the path itself. A label in front
    /// of it becomes part of every path a script reads.
    #[test]
    fn config_path_answers_with_the_path_alone() {
        let report = config_path_report(Path::new(A_CONFIG_PATH));

        assert_eq!(
            report.document(),
            Some(A_CONFIG_PATH),
            "standard output must hold the path and nothing else"
        );
    }
}
