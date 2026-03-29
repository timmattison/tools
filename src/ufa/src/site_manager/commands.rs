use anyhow::Result;
use clap::Subcommand;
use crate::output::{OutputFormat, print_output};
use crate::site_manager::SiteManagerClient;

#[derive(Subcommand, Debug, Clone)]
pub enum CloudCommand {
    /// List all cloud-managed hosts/consoles
    Hosts,
    
    /// Get details for a specific host/console
    Host {
        /// Host/Console ID
        id: String,
    },
}

pub async fn handle_cloud_command(
    command: CloudCommand,
    client: &SiteManagerClient,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        CloudCommand::Hosts => {
            let hosts = client.get_hosts().await?;
            
            if hosts.is_empty() {
                println!("No cloud-managed hosts found.");
                return Ok(());
            }

            match output_format {
                OutputFormat::Json => {
                    print_output(&hosts, output_format)?;
                }
                OutputFormat::Table => {
                    // Create a simplified table view
                    println!("{:<64} {:<15} {:<15} {:<10} {:<15} {:<8} {:<6} {:<20}",
                        "ID", "Name", "Model", "Firmware", "IP Address", "Type", "Owner", "Last Seen");
                    println!("{}", "-".repeat(150));
                    
                    for host in &hosts {
                        let name = host.reported_state.as_ref()
                            .and_then(|s| s.name.as_ref())
                            .map(|s| s.as_str())
                            .unwrap_or("Unknown");
                        let model = host.reported_state.as_ref()
                            .and_then(|s| s.model.as_ref())
                            .map(|s| s.as_str())
                            .unwrap_or("Unknown");
                        let firmware = host.reported_state.as_ref()
                            .and_then(|s| s.firmware_version.as_ref())
                            .map(|s| s.as_str())
                            .unwrap_or("Unknown");
                        let ip = host.ip_address.as_ref()
                            .map(|s| s.as_str())
                            .unwrap_or("N/A");
                        let last_seen = host.last_connection_state_change.as_ref()
                            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                            .unwrap_or_else(|| "N/A".to_string());
                        
                        // Truncate ID for display
                        let display_id = if host.id.len() > 60 {
                            format!("{}...", &host.id[..60])
                        } else {
                            host.id.clone()
                        };
                        
                        println!("{:<64} {:<15} {:<15} {:<10} {:<15} {:<8} {:<6} {:<20}",
                            display_id, name, model, firmware, ip, host.host_type, host.owner, last_seen);
                    }
                    
                    println!("\nTotal hosts: {}", hosts.len());
                    println!("\nTo get details for a specific host, use: ufa cloud host <id>");
                }
            }
        }
        
        CloudCommand::Host { id } => {
            let host = client.get_host(&id).await?;
            
            print_output(&host, output_format)?;
            
            if matches!(output_format, OutputFormat::Table) {
                println!("\nCloud Console URL:");
                println!("https://unifi.ui.com/consoles/{}/network/default/dashboard", host.id);
            }
        }
    }

    Ok(())
}