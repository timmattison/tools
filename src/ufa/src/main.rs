mod client;
mod models;
mod commands;
mod output;

use anyhow::Result;
use clap::{Parser, Subcommand};
use client::UnifiClient;
use commands::*;

/// UniFi API CLI tool for managing UniFi Network applications
#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// UniFi controller URL (e.g., https://192.168.1.1)
    #[clap(long, env = "UNIFI_URL")]
    url: String,

    /// API key for authentication (generate in Settings -> Control Plane -> Integrations)
    #[clap(long, env = "UNIFI_API_KEY")]
    api_key: String,

    /// Skip TLS certificate verification
    #[clap(long)]
    insecure: bool,

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
        #[clap(subcommand)]
        command: devices::DevicesCommand,
    },
    
    /// Manage clients
    Clients {
        #[clap(subcommand)]
        command: clients::ClientsCommand,
    },
    
    /// Manage hotspot vouchers
    Vouchers {
        #[clap(subcommand)]
        command: vouchers::VouchersCommand,
    },
    
    /// Get application information
    Info,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if it exists (ignore errors if file doesn't exist)
    let _ = dotenvy::dotenv();
    
    let args = Args::parse();

    let client = UnifiClient::new(
        &args.url,
        &args.api_key,
        args.insecure,
    ).await?;

    match args.command {
        Commands::Sites { limit, offset, filter } => {
            let cmd = sites::SitesCommand::List { limit, offset, filter };
            sites::handle_sites_command(cmd, &client, args.output).await
        },
        Commands::Devices { command } => devices::handle_devices_command(command, &client, args.output).await,
        Commands::Clients { command } => clients::handle_clients_command(command, &client, args.output).await,
        Commands::Vouchers { command } => vouchers::handle_vouchers_command(command, &client, args.output).await,
        Commands::Info => info::handle_info_command(&client, args.output).await,
    }
}