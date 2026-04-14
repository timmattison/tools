use anyhow::{Context, Result};
use dirs::config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use crate::discovery::{discover_controllers, validate_user_url};
use crate::client::UnifiClient;

/// A controller discovered from the 1Password `ufa` item.
#[derive(Debug, Clone)]
pub struct OpController {
    pub host: String,
    pub port: u16,
    pub op_path: String,
}

impl OpController {
    pub fn url(&self) -> String {
        format!("https://{}:{}", self.host, self.port)
    }
}

/// Discover controllers stored in the 1Password `Private/ufa` item.
///
/// Fields with labels matching `key - <host> port <port>` are parsed.
pub fn discover_op_controllers() -> Result<Vec<OpController>> {
    let output = std::process::Command::new("op")
        .args(["item", "get", "ufa", "--vault", "Private", "--format", "json"])
        .output()
        .context("Failed to run 'op' CLI — is 1Password CLI installed?")?;

    if !output.status.success() {
        anyhow::bail!(
            "op item get failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse 1Password item JSON")?;

    let fields = json["fields"]
        .as_array()
        .context("No fields array in 1Password item")?;

    let mut controllers = Vec::new();
    for field in fields {
        let label = match field["label"].as_str() {
            Some(l) => l,
            None => continue,
        };

        if let Some(rest) = label.strip_prefix("key - ") {
            if let Some((host, port_str)) = rest.rsplit_once(" port ") {
                if let Ok(port) = port_str.parse::<u16>() {
                    let op_path = format!("op://Private/ufa/{label}");
                    controllers.push(OpController {
                        host: host.to_string(),
                        port,
                        op_path,
                    });
                }
            }
        }
    }

    Ok(controllers)
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Config {
    pub url: Option<String>,
    /// Deprecated: use op_path instead. Kept for backward compatibility.
    pub api_key: Option<String>,
    pub insecure: Option<bool>,
    pub site_manager_api_key: Option<String>,
    /// 1Password path to the API key (e.g. "op://Private/ufa/key - 192.168.0.1 port 443")
    pub op_path: Option<String>,
}

impl Config {
    /// Get the OS-specific configuration directory
    pub fn config_dir() -> Result<PathBuf> {
        let base_dir = config_dir()
            .context("Could not determine configuration directory for your OS")?;
        Ok(base_dir.join("ufa"))
    }

    /// Get the configuration file path
    pub fn config_file_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    /// Load configuration from the default location
    pub fn load() -> Result<Option<Self>> {
        let config_path = Self::config_file_path()?;
        
        if !config_path.exists() {
            return Ok(None);
        }

        let contents = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;
        
        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;
        
        Ok(Some(config))
    }

    /// Read the API key from 1Password via op-cache, falling back to the
    /// plaintext `api_key` field for backward compatibility.
    pub fn resolve_api_key(&self) -> Result<String> {
        if let Some(op_path) = &self.op_path {
            let cache =
                op_cache::OpCache::new().map_err(|e| anyhow::anyhow!("{e}"))?;
            let path =
                op_cache::OpPath::new(op_path).map_err(|e| anyhow::anyhow!("{e}"))?;
            let key = cache.read(&path, None).map_err(|e| anyhow::anyhow!("{e}"))?;
            return Ok(key);
        }
        if let Some(key) = &self.api_key {
            return Ok(key.clone());
        }
        anyhow::bail!("No API key configured. Run 'ufa config setup'.")
    }

    /// Save configuration to the default location
    pub fn save(&self) -> Result<()> {
        let config_dir = Self::config_dir()?;
        fs::create_dir_all(&config_dir)
            .with_context(|| format!("Failed to create config directory: {}", config_dir.display()))?;
        
        let config_path = Self::config_file_path()?;
        let contents = toml::to_string_pretty(self)
            .context("Failed to serialize configuration")?;
        
        fs::write(&config_path, contents)
            .with_context(|| format!("Failed to write config file: {}", config_path.display()))?;
        
        println!("Configuration saved to: {}", config_path.display());
        Ok(())
    }

    /// Interactive configuration setup
    pub async fn setup() -> Result<()> {
        println!("🚀 UniFi API Configuration Setup");
        println!("================================\n");

        // Step 1: Try 1Password first, then fall back to network discovery
        let op_controllers = match discover_op_controllers() {
            Ok(c) if !c.is_empty() => c,
            Ok(_) => {
                println!("No controllers found in 1Password (Private/ufa).\n");
                Vec::new()
            }
            Err(e) => {
                println!("Could not query 1Password: {e}\n");
                Vec::new()
            }
        };

        #[derive(Debug)]
        enum Selection {
            Op(OpController),
            Network(String),
        }

        let selection = if !op_controllers.is_empty() {
            println!("Found {} controller(s) in 1Password:", op_controllers.len());
            for (i, c) in op_controllers.iter().enumerate() {
                println!("  {}. {}", i + 1, c.url());
            }
            let manual_idx = op_controllers.len() + 1;
            let network_idx = op_controllers.len() + 2;
            println!("  {manual_idx}. Enter URL manually");
            println!("  {network_idx}. Search network instead\n");

            loop {
                print!("Select a controller [1-{network_idx}]: ");
                io::stdout().flush()?;

                let mut input = String::new();
                io::stdin().read_line(&mut input)?;

                if let Ok(choice) = input.trim().parse::<usize>() {
                    if choice > 0 && choice <= op_controllers.len() {
                        break Selection::Op(op_controllers[choice - 1].clone());
                    } else if choice == manual_idx {
                        break Selection::Network(get_manual_controller_url().await?);
                    } else if choice == network_idx {
                        break Selection::Network(
                            network_discover_and_select().await?,
                        );
                    }
                }
                println!("Invalid choice. Please try again.");
            }
        } else {
            Selection::Network(network_discover_and_select().await?)
        };

        let (controller_url, op_path, api_key) = match selection {
            Selection::Op(c) => {
                // Read the key via op-cache to verify it works
                let cache =
                    op_cache::OpCache::new().map_err(|e| anyhow::anyhow!("{e}"))?;
                let path =
                    op_cache::OpPath::new(&c.op_path).map_err(|e| anyhow::anyhow!("{e}"))?;
                let key = cache
                    .read(&path, None)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                (c.url(), Some(c.op_path), key)
            }
            Selection::Network(url) => {
                let key = prompt_for_api_key(&url)?;
                (url, None, key)
            }
        };

        // Ask about certificate verification
        print!("\nSkip TLS certificate verification? (needed for self-signed certs) [y/N]: ");
        io::stdout().flush()?;

        let mut insecure_input = String::new();
        io::stdin().read_line(&mut insecure_input)?;
        let insecure = matches!(insecure_input.trim().to_lowercase().as_str(), "y" | "yes");

        // Test the connection
        println!("\n🔍 Testing connection...");
        match UnifiClient::new(&controller_url, &api_key, insecure).await {
            Ok(client) => {
                match client.get::<crate::models::ApplicationInfo>("info").await {
                    Ok(info) => {
                        println!("✅ Successfully connected to UniFi controller!");
                        println!("   Version: {}", info.application_version);
                    }
                    Err(e) => {
                        println!("⚠️  Connected but couldn't fetch info: {e}");
                        println!(
                            "   This might be normal if the API key has limited permissions."
                        );
                    }
                }
            }
            Err(e) => {
                println!("❌ Failed to connect: {e}");
                print!("\nSave configuration anyway? [y/N]: ");
                io::stdout().flush()?;

                let mut save_anyway = String::new();
                io::stdin().read_line(&mut save_anyway)?;
                if !matches!(save_anyway.trim().to_lowercase().as_str(), "y" | "yes") {
                    anyhow::bail!("Configuration not saved");
                }
            }
        }

        // Site Manager API Key (optional)
        println!("\n\nOptional: UniFi Site Manager (Cloud) Configuration");
        print!("Site Manager API Key (from unifi.ui.com API section) [skip]: ");
        io::stdout().flush()?;
        let mut sm_api_key = String::new();
        io::stdin().read_line(&mut sm_api_key)?;
        let sm_api_key = sm_api_key.trim();

        // Save configuration
        let config = Config {
            url: Some(controller_url),
            api_key: None,
            insecure: Some(insecure),
            site_manager_api_key: if sm_api_key.is_empty() {
                None
            } else {
                Some(sm_api_key.to_string())
            },
            op_path,
        };

        config.save()?;

        println!("\n🎉 Configuration complete!");
        println!("You can now use ufa commands without specifying connection details.");
        if config.site_manager_api_key.is_some() {
            println!("Cloud commands are available: try 'ufa cloud hosts'");
        }

        Ok(())
    }
}

/// Run network discovery (mDNS + common IPs) and let the user pick.
async fn network_discover_and_select() -> Result<String> {
    let controllers = discover_controllers().await?;

    if controllers.is_empty() {
        println!("No UniFi controllers found on the network.\n");
        return get_manual_controller_url().await;
    }

    println!("\nFound {} controller(s) on the network:", controllers.len());
    for (i, c) in controllers.iter().enumerate() {
        println!(
            "  {}. {} {}",
            i + 1,
            c.url(),
            if c.is_verified { "✓" } else { "" }
        );
    }
    let manual_idx = controllers.len() + 1;
    println!("  {manual_idx}. Enter URL manually\n");

    loop {
        print!("Select a controller [1-{manual_idx}]: ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if let Ok(choice) = input.trim().parse::<usize>() {
            if choice > 0 && choice <= controllers.len() {
                return Ok(controllers[choice - 1].url());
            } else if choice == manual_idx {
                return get_manual_controller_url().await;
            }
        }
        println!("Invalid choice. Please try again.");
    }
}

async fn get_manual_controller_url() -> Result<String> {
    loop {
        print!("Enter your UniFi controller URL (e.g., https://192.168.1.1): ");
        io::stdout().flush()?;

        let mut url = String::new();
        io::stdin().read_line(&mut url)?;
        let url = url.trim();

        if url.is_empty() {
            println!("URL cannot be empty. Please try again.");
            continue;
        }

        let url = if !url.starts_with("http://") && !url.starts_with("https://") {
            format!("https://{url}")
        } else {
            url.to_string()
        };

        print!("Validating controller...");
        io::stdout().flush()?;

        match validate_user_url(&url).await {
            Ok(controller) => {
                println!(" ✓");
                return Ok(controller.url());
            }
            Err(e) => {
                println!(" ✗");
                println!("Failed to validate controller: {e}");
                print!("Use this URL anyway? [y/N]: ");
                io::stdout().flush()?;

                let mut use_anyway = String::new();
                io::stdin().read_line(&mut use_anyway)?;
                if matches!(use_anyway.trim().to_lowercase().as_str(), "y" | "yes") {
                    return Ok(url);
                }
            }
        }
    }
}

/// Prompt for an API key when no 1Password entry exists.
fn prompt_for_api_key(controller_url: &str) -> Result<String> {
    let settings_url = if controller_url.ends_with('/') {
        format!("{controller_url}settings/control-plane/integrations")
    } else {
        format!("{controller_url}/settings/control-plane/integrations")
    };

    println!("\n📋 To generate an API key:");
    println!("1. Go to: {settings_url}");
    println!("2. Click 'Add Integration'");
    println!("3. Give it a name (e.g., 'ufa CLI')");
    println!("4. Copy the generated API key\n");

    if open::that(&settings_url).is_ok() {
        println!("✓ Opening browser...");
    } else {
        println!("Could not open browser automatically. Please visit the URL above.");
    }

    print!("\nPaste your API key here: ");
    io::stdout().flush()?;

    let mut api_key = String::new();
    io::stdin().read_line(&mut api_key)?;
    let api_key = api_key.trim().to_string();

    if api_key.is_empty() {
        anyhow::bail!("API key cannot be empty");
    }

    Ok(api_key)
}