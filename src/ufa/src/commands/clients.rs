use anyhow::Result;
use clap::Subcommand;
use tabled::Tabled;
use uuid::Uuid;

use crate::{
    client::UnifiClient,
    models::{Client, ClientAction, Page},
    output::{OutputFormat, print_vec_table, print_single_item},
    site_helper::get_site_id_or_prompt,
};

#[derive(Subcommand, Debug)]
pub enum ClientsCommand {
    /// List connected clients on a site
    List {
        /// Site ID (if not provided, will auto-detect)
        site_id: Option<Uuid>,

        /// Maximum number of clients to return
        #[clap(long, default_value = "25")]
        limit: u32,

        /// Offset for pagination
        #[clap(long, default_value = "0")]
        offset: u64,

        /// Filter expression
        #[clap(long)]
        filter: Option<String>,
    },

    /// Get client details
    Get {
        /// Site ID (if not provided, will auto-detect)
        site_id: Option<Uuid>,

        /// Client ID
        client_id: Uuid,
    },

    /// Authorize guest access for a client
    AuthorizeGuest {
        /// Site ID (if not provided, will auto-detect)
        site_id: Option<Uuid>,

        /// Client ID
        client_id: Uuid,

        /// Time limit in minutes
        #[clap(long)]
        time_limit_minutes: Option<u64>,

        /// Data usage limit in megabytes
        #[clap(long)]
        data_usage_limit_mbytes: Option<u64>,

        /// Download rate limit in kilobits per second
        #[clap(long)]
        rx_rate_limit_kbps: Option<u64>,

        /// Upload rate limit in kilobits per second
        #[clap(long)]
        tx_rate_limit_kbps: Option<u64>,
    },

    /// Unauthorize guest access for a client
    UnauthorizeGuest {
        /// Site ID (if not provided, will auto-detect)
        site_id: Option<Uuid>,

        /// Client ID
        client_id: Uuid,
    },
}

#[derive(Tabled, serde::Serialize)]
struct ClientRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Type")]
    client_type: String,
    #[tabled(rename = "IP Address")]
    ip_address: String,
    #[tabled(rename = "MAC Address")]
    mac_address: String,
    #[tabled(rename = "Connected At")]
    connected_at: String,
}

fn client_to_row(client: &Client) -> ClientRow {
    match client {
        Client::Wired(c) => ClientRow {
            id: c.id.to_string(),
            name: c.name.clone(),
            client_type: "WIRED".to_string(),
            ip_address: c.ip_address.clone().unwrap_or_default(),
            mac_address: c.mac_address.clone(),
            connected_at: c.connected_at.clone().unwrap_or_default(),
        },
        Client::Wireless(c) => ClientRow {
            id: c.id.to_string(),
            name: c.name.clone(),
            client_type: "WIRELESS".to_string(),
            ip_address: c.ip_address.clone().unwrap_or_default(),
            mac_address: c.mac_address.clone(),
            connected_at: c.connected_at.clone().unwrap_or_default(),
        },
        Client::Vpn(c) => ClientRow {
            id: c.id.to_string(),
            name: c.name.clone(),
            client_type: "VPN".to_string(),
            ip_address: c.ip_address.clone().unwrap_or_default(),
            mac_address: "N/A".to_string(),
            connected_at: c.connected_at.clone().unwrap_or_default(),
        },
        Client::Teleport(c) => ClientRow {
            id: c.id.to_string(),
            name: c.name.clone(),
            client_type: "TELEPORT".to_string(),
            ip_address: c.ip_address.clone().unwrap_or_default(),
            mac_address: "N/A".to_string(),
            connected_at: c.connected_at.clone().unwrap_or_default(),
        },
    }
}

pub async fn handle_clients_command(
    command: ClientsCommand,
    client: &UnifiClient,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        ClientsCommand::List { site_id, limit, offset, filter } => {
            list_clients(client, site_id, limit, offset, filter, output_format).await
        }
        ClientsCommand::Get { site_id, client_id } => {
            get_client(client, site_id, client_id, output_format).await
        }
        ClientsCommand::AuthorizeGuest { 
            site_id, 
            client_id, 
            time_limit_minutes,
            data_usage_limit_mbytes,
            rx_rate_limit_kbps,
            tx_rate_limit_kbps,
        } => {
            authorize_guest(
                client, 
                site_id, 
                client_id, 
                time_limit_minutes,
                data_usage_limit_mbytes,
                rx_rate_limit_kbps,
                tx_rate_limit_kbps,
            ).await
        }
        ClientsCommand::UnauthorizeGuest { site_id, client_id } => {
            unauthorize_guest(client, site_id, client_id).await
        }
    }
}

async fn list_clients(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    limit: u32,
    offset: u64,
    filter: Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let limit_str = limit.to_string();
    let offset_str = offset.to_string();
    let mut params: Vec<(&str, &dyn std::fmt::Display)> = vec![
        ("limit", &limit_str),
        ("offset", &offset_str),
    ];

    if let Some(f) = &filter {
        params.push(("filter", f));
    }

    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/clients", site_id);
    let page: Page<Client> = client.get_with_params(&path, &params).await?;

    match output_format {
        OutputFormat::Json => {
            print_single_item(&page, output_format)?;
        }
        OutputFormat::Table => {
            let rows: Vec<ClientRow> = page.data.iter().map(client_to_row).collect();
            print_vec_table(&rows, output_format)?;
        }
    }

    Ok(())
}

async fn get_client(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    client_id: Uuid,
    output_format: OutputFormat,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/clients/{}", site_id, client_id);
    let client_details: Client = client.get(&path).await?;

    print_single_item(&client_details, output_format)?;
    Ok(())
}

async fn authorize_guest(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    client_id: Uuid,
    time_limit_minutes: Option<u64>,
    data_usage_limit_mbytes: Option<u64>,
    rx_rate_limit_kbps: Option<u64>,
    tx_rate_limit_kbps: Option<u64>,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/clients/{}/actions", site_id, client_id);
    let action = ClientAction::AuthorizeGuestAccess {
        time_limit_minutes,
        data_usage_limit_mbytes,
        rx_rate_limit_kbps,
        tx_rate_limit_kbps,
    };

    let _: serde_json::Value = client.post(&path, &action).await?;
    println!("Guest access authorized successfully");
    Ok(())
}

async fn unauthorize_guest(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    client_id: Uuid,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/clients/{}/actions", site_id, client_id);
    let action = ClientAction::UnauthorizeGuestAccess;

    let _: serde_json::Value = client.post(&path, &action).await?;
    println!("Guest access unauthorized successfully");
    Ok(())
}