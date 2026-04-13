use anyhow::{Context, Result};
use dirs::config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use crate::discovery::{discover_controllers, validate_user_url};
use crate::client::UnifiClient;

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Config {
    pub url: Option<String>,
    pub api_key: Option<String>,
    pub insecure: Option<bool>,
    pub site_manager_api_key: Option<String>,
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
        
        // Step 1: Discover or get controller URL
        let controllers = discover_controllers().await?;
        
        let controller_url = if controllers.is_empty() {
            println!("No UniFi controllers found automatically.\n");
            get_manual_controller_url().await?
        } else {
            println!("\nFound {} UniFi controller(s):", controllers.len());
            for (i, controller) in controllers.iter().enumerate() {
                println!("  {}. {} {}", 
                    i + 1, 
                    controller.url(),
                    if controller.is_verified { "✓" } else { "" }
                );
            }
            println!("  {}. Enter URL manually\n", controllers.len() + 1);
            
            loop {
                print!("Select a controller [1-{}]: ", controllers.len() + 1);
                io::stdout().flush()?;
                
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                
                if let Ok(choice) = input.trim().parse::<usize>() {
                    if choice > 0 && choice <= controllers.len() {
                        break controllers[choice - 1].url();
                    } else if choice == controllers.len() + 1 {
                        break get_manual_controller_url().await?;
                    } else {
                        println!("Invalid choice. Please try again.");
                        continue;
                    }
                } else {
                    println!("Invalid input. Please enter a number.");
                    continue;
                }
            }
        };
        
        // Step 2: Open browser for API key generation
        let settings_url = if controller_url.ends_with('/') {
            format!("{}settings/control-plane/integrations", controller_url)
        } else {
            format!("{}/settings/control-plane/integrations", controller_url)
        };
        
        println!("\n📋 To generate an API key:");
        println!("1. Go to: {}", settings_url);
        println!("2. Click 'Add Integration'");
        println!("3. Give it a name (e.g., 'ufa CLI')");
        println!("4. Copy the generated API key\n");
        
        // Try to open the browser
        if open::that(&settings_url).is_ok() {
            println!("✓ Opening browser...");
        } else {
            println!("⚠️  Could not open browser automatically. Please visit the URL above.");
        }
        
        // Step 3: Get API key
        print!("\nPaste your API key here: ");
        io::stdout().flush()?;
        
        let mut api_key = String::new();
        io::stdin().read_line(&mut api_key)?;
        let api_key = api_key.trim().to_string();
        
        if api_key.is_empty() {
            anyhow::bail!("API key cannot be empty");
        }
        
        // Step 4: Ask about certificate verification
        print!("\nSkip TLS certificate verification? (needed for self-signed certs) [y/N]: ");
        io::stdout().flush()?;
        
        let mut insecure_input = String::new();
        io::stdin().read_line(&mut insecure_input)?;
        let insecure = matches!(insecure_input.trim().to_lowercase().as_str(), "y" | "yes");
        
        // Step 5: Test the connection
        println!("\n🔍 Testing connection...");
        match UnifiClient::new(&controller_url, &api_key, insecure).await {
            Ok(client) => {
                // Try to get application info to verify the connection
                match client.get::<crate::models::ApplicationInfo>("info").await {
                    Ok(info) => {
                        println!("✅ Successfully connected to UniFi controller!");
                        println!("   Version: {}", info.application_version);
                    }
                    Err(e) => {
                        println!("⚠️  Connected but couldn't fetch info: {}", e);
                        println!("   This might be normal if the API key has limited permissions.");
                    }
                }
            }
            Err(e) => {
                println!("❌ Failed to connect: {}", e);
                print!("\nSave configuration anyway? [y/N]: ");
                io::stdout().flush()?;
                
                let mut save_anyway = String::new();
                io::stdin().read_line(&mut save_anyway)?;
                if !matches!(save_anyway.trim().to_lowercase().as_str(), "y" | "yes") {
                    anyhow::bail!("Configuration not saved");
                }
            }
        }
        
        // Step 6: Site Manager API Key (optional)
        println!("\n\nOptional: UniFi Site Manager (Cloud) Configuration");
        print!("Site Manager API Key (from unifi.ui.com API section) [skip]: ");
        io::stdout().flush()?;
        let mut sm_api_key = String::new();
        io::stdin().read_line(&mut sm_api_key)?;
        let sm_api_key = sm_api_key.trim();
        
        // Step 7: Save configuration
        let config = Config {
            url: Some(controller_url),
            api_key: Some(api_key),
            insecure: Some(insecure),
            site_manager_api_key: if sm_api_key.is_empty() { None } else { Some(sm_api_key.to_string()) },
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
        
        // Add https:// if not present
        let url = if !url.starts_with("http://") && !url.starts_with("https://") {
            format!("https://{}", url)
        } else {
            url.to_string()
        };
        
        // Validate the URL
        print!("Validating controller...");
        io::stdout().flush()?;
        
        match validate_user_url(&url).await {
            Ok(controller) => {
                println!(" ✓");
                return Ok(controller.url());
            }
            Err(e) => {
                println!(" ✗");
                println!("Failed to validate controller: {}", e);
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