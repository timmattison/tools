use anyhow::Result;
use clap::Subcommand;
use tabled::Tabled;
use uuid::Uuid;

use crate::{
    client::UnifiClient,
    models::{Page, Voucher, VoucherCreateRequest, VoucherCreateResponse, VoucherDeletionResults},
    output::{OutputFormat, print_vec_table, print_single_item},
    site_helper::get_site_id_or_prompt,
};

#[derive(Subcommand, Debug)]
pub enum VouchersCommand {
    /// List vouchers on a site
    List {
        /// Site ID (if not provided, will auto-detect)
        site_id: Option<Uuid>,

        /// Maximum number of vouchers to return
        #[clap(long, default_value = "100")]
        limit: u32,

        /// Offset for pagination
        #[clap(long, default_value = "0")]
        offset: u64,

        /// Filter expression
        #[clap(long)]
        filter: Option<String>,
    },

    /// Get voucher details
    Get {
        /// Site ID (if not provided, will auto-detect)
        site_id: Option<Uuid>,

        /// Voucher ID
        voucher_id: Uuid,
    },

    /// Create new vouchers
    Create {
        /// Site ID (if not provided, will auto-detect)
        site_id: Option<Uuid>,

        /// Number of vouchers to create
        #[clap(long, default_value = "1")]
        count: u32,

        /// Voucher name/note
        #[clap(long)]
        name: String,

        /// Time limit in minutes
        #[clap(long)]
        time_limit_minutes: u64,

        /// Maximum number of guests per voucher
        #[clap(long)]
        authorized_guest_limit: Option<u64>,

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

    /// Delete a specific voucher
    Delete {
        /// Site ID (if not provided, will auto-detect)
        site_id: Option<Uuid>,

        /// Voucher ID
        voucher_id: Uuid,
    },

    /// Delete vouchers by filter
    DeleteFiltered {
        /// Site ID (if not provided, will auto-detect)
        site_id: Option<Uuid>,

        /// Filter expression
        #[clap(long)]
        filter: String,
    },
}

#[derive(Tabled, serde::Serialize)]
struct VoucherRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Code")]
    code: String,
    #[tabled(rename = "Time Limit (min)")]
    time_limit_minutes: String,
    #[tabled(rename = "Guest Limit")]
    guest_limit: String,
    #[tabled(rename = "Guest Count")]
    guest_count: String,
    #[tabled(rename = "Expired")]
    expired: String,
    #[tabled(rename = "Created At")]
    created_at: String,
}

impl From<&Voucher> for VoucherRow {
    fn from(voucher: &Voucher) -> Self {
        Self {
            id: voucher.id.to_string(),
            name: voucher.name.clone(),
            code: voucher.code.clone(),
            time_limit_minutes: voucher.time_limit_minutes.to_string(),
            guest_limit: voucher.authorized_guest_limit.map(|l| l.to_string()).unwrap_or("unlimited".to_string()),
            guest_count: voucher.authorized_guest_count.to_string(),
            expired: voucher.expired.to_string(),
            created_at: voucher.created_at.clone(),
        }
    }
}

pub async fn handle_vouchers_command(
    command: VouchersCommand,
    client: &UnifiClient,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        VouchersCommand::List { site_id, limit, offset, filter } => {
            list_vouchers(client, site_id, limit, offset, filter, output_format).await
        }
        VouchersCommand::Get { site_id, voucher_id } => {
            get_voucher(client, site_id, voucher_id, output_format).await
        }
        VouchersCommand::Create { 
            site_id, 
            count, 
            name, 
            time_limit_minutes,
            authorized_guest_limit,
            data_usage_limit_mbytes,
            rx_rate_limit_kbps,
            tx_rate_limit_kbps,
        } => {
            create_vouchers(
                client, 
                site_id, 
                count, 
                name, 
                time_limit_minutes,
                authorized_guest_limit,
                data_usage_limit_mbytes,
                rx_rate_limit_kbps,
                tx_rate_limit_kbps,
                output_format,
            ).await
        }
        VouchersCommand::Delete { site_id, voucher_id } => {
            delete_voucher(client, site_id, voucher_id).await
        }
        VouchersCommand::DeleteFiltered { site_id, filter } => {
            delete_vouchers_filtered(client, site_id, filter).await
        }
    }
}

async fn list_vouchers(
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
    let path = format!("sites/{}/hotspot/vouchers", site_id);
    let page: Page<Voucher> = client.get_with_params(&path, &params).await?;

    match output_format {
        OutputFormat::Json => {
            print_single_item(&page, output_format)?;
        }
        OutputFormat::Table => {
            let rows: Vec<VoucherRow> = page.data.iter().map(VoucherRow::from).collect();
            print_vec_table(&rows, output_format)?;
        }
    }

    Ok(())
}

async fn get_voucher(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    voucher_id: Uuid,
    output_format: OutputFormat,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/hotspot/vouchers/{}", site_id, voucher_id);
    let voucher: Voucher = client.get(&path).await?;

    print_single_item(&voucher, output_format)?;
    Ok(())
}

async fn create_vouchers(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    count: u32,
    name: String,
    time_limit_minutes: u64,
    authorized_guest_limit: Option<u64>,
    data_usage_limit_mbytes: Option<u64>,
    rx_rate_limit_kbps: Option<u64>,
    tx_rate_limit_kbps: Option<u64>,
    output_format: OutputFormat,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/hotspot/vouchers", site_id);
    let request = VoucherCreateRequest {
        count,
        name,
        time_limit_minutes,
        authorized_guest_limit,
        data_usage_limit_mbytes,
        rx_rate_limit_kbps,
        tx_rate_limit_kbps,
    };

    let response: VoucherCreateResponse = client.post(&path, &request).await?;

    match output_format {
        OutputFormat::Json => {
            print_single_item(&response.vouchers, output_format)?;
        }
        OutputFormat::Table => {
            let rows: Vec<VoucherRow> = response.vouchers.iter().map(VoucherRow::from).collect();
            print_vec_table(&rows, output_format)?;
        }
    }

    Ok(())
}

async fn delete_voucher(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    voucher_id: Uuid,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/hotspot/vouchers/{}", site_id, voucher_id);
    let result: VoucherDeletionResults = client.delete(&path).await?;

    println!("Deleted {} voucher(s)", result.vouchers_deleted);
    Ok(())
}

async fn delete_vouchers_filtered(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    filter: String,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/hotspot/vouchers", site_id);
    let params: Vec<(&str, &dyn std::fmt::Display)> = vec![("filter", &filter)];
    
    let result: VoucherDeletionResults = client.delete_with_params(&path, &params).await?;

    println!("Deleted {} voucher(s)", result.vouchers_deleted);
    Ok(())
}