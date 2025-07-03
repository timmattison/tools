use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use toml;
use which::which;

/// Rust Cross Compiler helper - simplifies cross-compilation setup
#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// Parse uname string to determine target architecture
    #[clap(long, conflicts_with = "target")]
    uname: Option<String>,

    /// Specify the target triple directly
    #[clap(long)]
    target: Option<String>,

    /// Build in release mode
    #[clap(long)]
    release: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct CrossConfig {
    #[serde(flatten)]
    targets: toml::Table,
}

#[derive(Debug, Serialize, Deserialize)]
struct TargetConfig {
    image: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Check if cross is installed
    check_cross_installed()?;

    // Determine the target architecture
    let target = determine_target(&args)?;

    // Handle Cross.toml
    let cross_toml_path = Path::new("Cross.toml");
    let final_target = if !cross_toml_path.exists() {
        match target {
            Some(t) => {
                create_cross_toml(&t)?;
                t
            }
            None => {
                anyhow::bail!(
                    "No Cross.toml found in current directory. You need to specify the architecture:\n\
                     - Use --target to specify a target triple directly (e.g., --target aarch64-unknown-linux-gnu)\n\
                     - Use --uname with the output from 'uname -a' on your target host"
                );
            }
        }
    } else {
        // Cross.toml exists
        match target {
            Some(t) => t,
            None => get_target_from_cross_toml()?
        }
    };

    execute_cross_build(&final_target, args.release)?;

    Ok(())
}

fn check_cross_installed() -> Result<()> {
    if which("cross").is_err() {
        anyhow::bail!(
            "cross is not installed. Please install it by running:\n\
             cargo install cross --git https://github.com/cross-rs/cross"
        );
    }
    Ok(())
}

fn determine_target(args: &Args) -> Result<Option<String>> {
    if let Some(target) = &args.target {
        return Ok(Some(target.clone()));
    }

    if let Some(uname_str) = &args.uname {
        return parse_uname_string(uname_str).map(Some);
    }

    Ok(None)
}

fn parse_uname_string(uname: &str) -> Result<String> {
    // Example: "Linux DreamMachinePro 4.19.152-ui-alpine #4.19.152 SMP Thu May 15 13:28:41 CST 2025 aarch64 GNU/Linux"
    let parts: Vec<&str> = uname.split_whitespace().collect();
    
    // Find the architecture
    let arch = parts.iter()
        .find(|&&p| matches!(p, "aarch64" | "x86_64" | "armv7l" | "i686"))
        .ok_or_else(|| anyhow::anyhow!("Could not find architecture in uname string"))?;
    
    // Determine if it's musl or gnu
    let libc = if uname.to_lowercase().contains("alpine") {
        "musl"
    } else {
        "gnu"
    };
    
    // Build the target triple
    let target = match *arch {
        "aarch64" => format!("aarch64-unknown-linux-{}", libc),
        "x86_64" => format!("x86_64-unknown-linux-{}", libc),
        "armv7l" => format!("armv7-unknown-linux-{}eabihf", libc),
        "i686" => format!("i686-unknown-linux-{}", libc),
        _ => anyhow::bail!("Unsupported architecture: {}", arch),
    };
    
    Ok(target)
}

fn create_cross_toml(target: &str) -> Result<()> {
    let content = format!(
        "[target.{}]\nimage = \"ghcr.io/cross-rs/{}:edge\"\n",
        target, target
    );
    
    fs::write("Cross.toml", content)
        .context("Failed to create Cross.toml")?;
    
    println!("Created Cross.toml for target: {}", target);
    Ok(())
}

fn get_target_from_cross_toml() -> Result<String> {
    let content = fs::read_to_string("Cross.toml")
        .context("Failed to read Cross.toml")?;
    
    let config: toml::Table = toml::from_str(&content)
        .context("Failed to parse Cross.toml")?;
    
    // Look for the "target" section
    let target_section = config.get("target")
        .ok_or_else(|| anyhow::anyhow!("No [target] section found in Cross.toml"))?;
    
    let target_table = target_section.as_table()
        .ok_or_else(|| anyhow::anyhow!("Invalid [target] section in Cross.toml"))?;
    
    // Get all target architecture names
    let targets: Vec<String> = target_table.keys()
        .map(|k| k.to_string())
        .collect();
    
    match targets.len() {
        0 => anyhow::bail!("No targets found in Cross.toml"),
        1 => Ok(targets[0].clone()),
        _ => {
            eprintln!("Multiple targets found in Cross.toml:");
            for target in &targets {
                eprintln!("  - {}", target);
            }
            anyhow::bail!(
                "Please specify which target to use with --target <target>"
            );
        }
    }
}

fn execute_cross_build(target: &str, release: bool) -> Result<()> {
    let mut cmd = Command::new("cross");
    cmd.arg("build")
        .arg("--target")
        .arg(target);
    
    if release {
        cmd.arg("--release");
    }
    
    println!("Executing: cross build --target {} {}", 
             target, 
             if release { "--release" } else { "" });
    
    let status = cmd.status()
        .context("Failed to execute cross build")?;
    
    if !status.success() {
        anyhow::bail!("Cross build failed");
    }
    
    Ok(())
}