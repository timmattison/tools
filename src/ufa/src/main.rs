mod client;
mod models;
mod commands;
mod output;
mod site_helper;
mod device_helper;
mod config;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use client::UnifiClient;
use commands::*;
use uuid::Uuid;
use config::Config;

fn parse_bool_env(s: &str) -> Result<bool, String> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!("Invalid boolean value: {}. Use true/false, 1/0, yes/no, or on/off", s))
    }
}

/// UniFi API CLI tool for managing UniFi Network applications
#[derive(Parser, Debug)]
#[clap(author, version, about)]
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
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    /// Interactive configuration setup
    Setup,
    /// Show current configuration file path
    Path,
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
                Config::setup()?;
                return Ok(());
            }
            ConfigCommand::Path => {
                let path = Config::config_file_path()?;
                println!("Configuration file path: {}", path.display());
                return Ok(());
            }
        }
    }

    // Load configuration from file
    let file_config = Config::load()?;
    
    // Determine final configuration values (CLI args > env vars > config file)
    let url = args.url
        .or_else(|| std::env::var("UNIFI_URL").ok())
        .or_else(|| file_config.as_ref().and_then(|c| c.url.clone()))
        .context("UniFi URL not provided. Set it via --url, UNIFI_URL environment variable, or run 'ufa config setup' to create a configuration file.")?;
    
    let api_key = args.api_key
        .or_else(|| std::env::var("UNIFI_API_KEY").ok())
        .or_else(|| file_config.as_ref().and_then(|c| c.api_key.clone()))
        .context("API key not provided. Set it via --api-key, UNIFI_API_KEY environment variable, or run 'ufa config setup' to create a configuration file.")?;
    
    let insecure = args.insecure
        .or_else(|| std::env::var("UNIFI_INSECURE").ok().and_then(|v| parse_bool_env(&v).ok()))
        .or_else(|| file_config.as_ref().and_then(|c| c.insecure))
        .unwrap_or(false);

    let client = UnifiClient::new(
        &url,
        &api_key,
        insecure,
    ).await?;

    match args.command {
        Commands::Sites { limit, offset, filter } => {
            let cmd = sites::SitesCommand::List { limit, offset, filter };
            sites::handle_sites_command(cmd, &client, args.output).await
        },
        Commands::Devices { site_id, command } => devices::handle_devices_command(command, site_id, &client, args.output).await,
        Commands::Clients { site_id, command } => clients::handle_clients_command(command, site_id, &client, args.output).await,
        Commands::Vouchers { site_id, command } => vouchers::handle_vouchers_command(command, site_id, &client, args.output).await,
        Commands::Info => info::handle_info_command(&client, args.output).await,
        Commands::Config { .. } => unreachable!("Config commands handled above"),
    }
}