use anyhow::{Context, Result};
use dirs::config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub url: Option<String>,
    pub api_key: Option<String>,
    pub insecure: Option<bool>,
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
    pub fn setup() -> Result<()> {
        todo!("Interactive configuration setup not yet implemented")
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            url: None,
            api_key: None,
            insecure: None,
        }
    }
}