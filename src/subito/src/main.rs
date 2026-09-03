//! `subito` — subscribe to AWS IoT Core topics and print every message.
//!
//! The binary reads the command line, asks AWS for the data endpoint of the
//! account when the command line names none, and then hands the connection to
//! [`subito::session::run_forever`]. The library holds every part that a test
//! can drive.
//!
//! The signature of an AWS IoT Core WebSocket covers the handshake alone, so
//! the connection this file gives the supervisor is a closure and not one set
//! of options. Each attempt reads the credentials again and signs the URL
//! again, at the time of the attempt. A closure that carried one set of
//! options would present a stale signature to every attempt after the first,
//! which is the failure the supervisor exists to repair.

#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]

use anyhow::{Context, Result};
use aws_credential_types::provider::ProvideCredentials as _;
use clap::Parser;
use rumqttc::{MqttOptions, Transport};
use std::process::ExitCode;
use std::time::SystemTime;
use subito::cli::Cli;
use subito::endpoint::describe_data_endpoint;
use subito::presign::presign_websocket_url;
use subito::session::{run_forever, SessionError};

/// The message the tool prints when the command line names no topic.
///
/// This is the message of the Go tool that this one replaces, character for
/// character.
const NO_TOPIC_MESSAGE: &str = "You must provide at least one AWS IoT topic to subscribe to";

/// The port AWS IoT Core listens on for MQTT over a WebSocket.
const WEBSOCKET_PORT: u16 = 443;

/// The first part of the MQTT client identifier of a run.
///
/// The rest is a random UUID. AWS IoT Core closes the older connection when
/// two clients present one identifier, so a fixed identifier would make two
/// copies of this tool close each other again and again.
const CLIENT_ID_PREFIX: &str = "subito-";

/// The word that starts every line the tool writes to standard error.
const REPORT_PREFIX: &str = "subito:";

/// The words that start every line of a cause under the first one.
const CAUSE_PREFIX: &str = "  caused by:";

/// What the tool says when the configuration names no region.
const NO_REGION_MESSAGE: &str = "no AWS region is set. Set AWS_REGION, or set region in the profile that AWS_PROFILE names, or give --endpoint and set the region of that endpoint";

/// What the tool says when the configuration gives no credentials provider.
const NO_CREDENTIALS_MESSAGE: &str = "the AWS configuration holds no credentials provider. Set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY, or name a profile with AWS_PROFILE";

/// What the tool says when the lookup of the data endpoint fails.
const ENDPOINT_LOOKUP_MESSAGE: &str =
    "could not ask AWS for the AWS IoT data endpoint of the account. Give --endpoint to skip the question";

/// What the tool says when it cannot read the credentials of an attempt.
const CREDENTIALS_MESSAGE: &str = "could not read the AWS credentials";

/// What the tool says when it cannot sign the WebSocket URL of an attempt.
const PRESIGN_MESSAGE: &str = "could not sign the AWS IoT WebSocket URL";

fn main() -> ExitCode {
    let cli = Cli::parse();

    // The topics come first, before the tool reads any configuration. A user
    // who forgets the topic must read this one line, and not the answer of a
    // credentials chain that had nothing to connect to.
    if cli.topics.is_empty() {
        eprintln!("{NO_TOPIC_MESSAGE}");
        return ExitCode::FAILURE;
    }

    match subscribe(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report(&error);
            ExitCode::FAILURE
        }
    }
}

/// Prints a failure and every failure under it, one to a line.
///
/// The message of the first error names the part that failed, and the message
/// of each error under it names the reason. A user needs the whole chain,
/// because the first line alone says which step failed and never why.
fn report(error: &anyhow::Error) {
    eprintln!("{REPORT_PREFIX} {error}");

    for cause in error.chain().skip(1) {
        eprintln!("{CAUSE_PREFIX} {cause}");
    }
}

/// Runs the tool, from the AWS configuration to the end of the session.
///
/// The function makes the Tokio runtime itself, and [`main`] stays a plain
/// function, so the path that refuses a command line with no topic starts no
/// runtime and reads no configuration.
///
/// # Errors
///
/// Gives a failure when the configuration names no region, when the
/// configuration holds no credentials provider, when the lookup of the data
/// endpoint fails, or when the session stops with a failure that no new
/// connection repairs.
#[tokio::main]
async fn subscribe(cli: &Cli) -> Result<()> {
    let configuration = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .load()
        .await;

    let region = configuration
        .region()
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!(NO_REGION_MESSAGE))?;

    let provider = configuration
        .credentials_provider()
        .ok_or_else(|| anyhow::anyhow!(NO_CREDENTIALS_MESSAGE))?;

    let endpoint = match &cli.endpoint {
        Some(endpoint) => endpoint.clone(),
        None => describe_data_endpoint(&configuration)
            .await
            .context(ENDPOINT_LOOKUP_MESSAGE)?,
    };

    let client_id = format!("{CLIENT_ID_PREFIX}{}", uuid::Uuid::new_v4());

    // Each name below is a reference the closure copies, so every attempt
    // builds its own options from the same inputs and owns nothing of them.
    let provider = &provider;
    let endpoint = endpoint.as_str();
    let region = region.as_str();
    let client_id = client_id.as_str();

    let connect = move || async move {
        let credentials = provider.provide_credentials().await.map_err(|failure| {
            connect_failure(&anyhow::Error::new(failure), CREDENTIALS_MESSAGE)
        })?;

        let url = presign_websocket_url(endpoint, region, &credentials, SystemTime::now())
            .map_err(|failure| connect_failure(&anyhow::Error::new(failure), PRESIGN_MESSAGE))?;

        let mut options = MqttOptions::new(client_id, url, WEBSOCKET_PORT);
        options.set_transport(Transport::wss_with_default_config());

        Ok(options)
    };

    run_forever(
        connect,
        &cli.topics,
        cli.mqtt_qos(),
        cli.json,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
        tokio::signal::ctrl_c(),
    )
    .await
    .map_err(anyhow::Error::new)
}

/// Turns a failure to build the options of an attempt into a session failure.
///
/// The supervisor reads a [`SessionError`], and it starts another attempt for
/// every failure that a new connection repairs. A credentials chain that timed
/// out and a clock that moved are both such failures, so this function gives
/// [`SessionError::connect`]: the tool reached no broker, and the next attempt
/// reads the credentials again.
///
/// The message of `failure` and of every failure under it stays in the answer,
/// because the supervisor prints the whole chain of what it retries.
fn connect_failure(failure: &anyhow::Error, what: &str) -> SessionError {
    SessionError::connect(format!("{what}: {failure:#}").into())
}
