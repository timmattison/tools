mod cities;
mod generator;

use anyhow::{bail, Context, Result};
use buildinfo::version_string;
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_OP_PATH: &str = "op://Private/ProtonVPN WireGuard key/credential";
const DEFAULT_GLUETUN_VERSION: &str = "v3.40";
const DEFAULT_CONTAINER_PREFIX: &str = "vpn";

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

        /// 1Password path for WireGuard private key
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
            which::which("docker")
                .context("docker not found in PATH — install Docker Desktop")?;

            // Fetch WireGuard key via op-cache
            let op_path_validated =
                op_cache::OpPath::new(&op_path).map_err(|e| anyhow::anyhow!("{e}"))?;
            let cache = op_cache::OpCache::new().map_err(|e| anyhow::anyhow!("{e}"))?;
            let wg_key = cache
                .read(&op_path_validated, Some("WIREGUARD_PRIVATE_KEY"))
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let extra_port_list: Vec<String> = extra_ports
                .map(|p| p.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default();

            generator::generate(
                &output_dir,
                city.as_deref(),
                &container_prefix,
                &gluetun_version,
                &wg_key,
                &extra_port_list,
            )?;

            println!(
                "\n{} Generated VPN tunnel in {}",
                "done:".green().bold(),
                output_dir.display()
            );
            println!("\nNext steps:");
            println!("  cd {} && ./start.sh", output_dir.display());
            println!("  ./status.sh          # check VPN IP");
            println!("  ./logs.sh            # view logs");
            println!("  ./stop.sh            # tear down");
            println!(
                "\nTo route a container through the VPN, add to your docker-compose.yml:"
            );
            println!("  network_mode: \"service:{container_prefix}-gluetun\"");
            println!("  depends_on:");
            println!("    {container_prefix}-gluetun:");
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
    let status = Command::new("docker")
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
    let output = Command::new("docker")
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
        let health = Command::new("docker")
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
    let output = Command::new("docker")
        .args(["compose", "ps", "--format", "{{.Name}}"])
        .current_dir(dir)
        .output()?;

    let containers = String::from_utf8_lossy(&output.stdout);
    let gluetun_name = containers
        .lines()
        .find(|l| l.contains("gluetun"))
        .unwrap_or("gluetun");

    let ip_output = Command::new("docker")
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
