use crate::client::UnifiClient;
use crate::discovery::{discover_controllers, validate_user_url};
use anyhow::{Context, Result};
use dirs::config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Prefix identifying a 1Password secret reference.
const OP_REFERENCE_PREFIX: &str = "op://";

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
        .args([
            "item", "get", "ufa", "--vault", "Private", "--format", "json",
        ])
        .output()
        .context("Failed to run 'op' CLI — is 1Password CLI installed?")?;

    if !output.status.success() {
        anyhow::bail!(
            "op item get failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse 1Password item JSON")?;

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

/// The controller credential gathered during interactive setup.
///
/// Either the key already lives in 1Password (and setup merely read it to
/// verify the reference works), or the user pasted it in because no 1Password
/// entry exists for this controller.
#[derive(Debug, Clone)]
pub enum ControllerCredential {
    /// The key is stored in 1Password at `op_path`; `key` is its current value.
    OnePassword { op_path: String, key: String },
    /// The key was pasted during setup and exists nowhere else.
    Pasted { key: String },
}

impl ControllerCredential {
    /// The API key value, used to test the connection during setup.
    pub fn key(&self) -> &str {
        match self {
            Self::OnePassword { key, .. } | Self::Pasted { key } => key,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Config {
    pub url: Option<String>,
    /// API key stored directly in the config file.
    ///
    /// `op_path` is preferred; this field is used only when the key is not in
    /// 1Password — for example when the user pasted it during setup because no
    /// `Private/ufa` item exists.
    pub api_key: Option<String>,
    pub insecure: Option<bool>,
    /// Site Manager (cloud) API key stored directly in the config file.
    ///
    /// Legacy fallback: `sm_op_path` is preferred.
    pub site_manager_api_key: Option<String>,
    /// 1Password path to the API key (e.g. "op://Private/ufa/key - 192.168.0.1 port 443")
    pub op_path: Option<String>,
    /// 1Password path to the Site Manager (cloud) API key
    /// (e.g. "op://Private/ufa/site manager key")
    pub sm_op_path: Option<String>,
}

impl Config {
    /// Get the OS-specific configuration directory
    pub fn config_dir() -> Result<PathBuf> {
        let base_dir =
            config_dir().context("Could not determine configuration directory for your OS")?;
        Ok(base_dir.join("ufa"))
    }

    /// Get the configuration file path
    pub fn config_file_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    /// Load configuration from the default location
    pub fn load() -> Result<Option<Self>> {
        Self::load_from(&Self::config_file_path()?)
    }

    /// Load configuration from an explicit path.
    ///
    /// Returns `Ok(None)` when the file does not exist.
    fn load_from(config_path: &Path) -> Result<Option<Self>> {
        if !config_path.exists() {
            return Ok(None);
        }

        let contents = fs::read_to_string(config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;

        Ok(Some(config))
    }

    /// Apply an edit to the saved configuration and write it back.
    ///
    /// This is the only way the interactive commands change what is on disk,
    /// so a command that asks about one credential can never clear another.
    fn edit(edit: impl FnOnce(&mut Self)) -> Result<Self> {
        Self::edit_at(&Self::config_file_path()?, edit)
    }

    /// [`Config::edit`] against an explicit path.
    fn edit_at(config_path: &Path, edit: impl FnOnce(&mut Self)) -> Result<Self> {
        let mut config = Self::load_from(config_path)?.unwrap_or_default();
        edit(&mut config);
        config.save_to(config_path)?;
        Ok(config)
    }

    /// Record the controller answers gathered during interactive setup.
    ///
    /// Fields the controller step does not ask about are left untouched.
    fn set_controller(&mut self, url: String, credential: &ControllerCredential, insecure: bool) {
        self.url = Some(url);
        self.insecure = Some(insecure);

        match credential {
            ControllerCredential::OnePassword { op_path, .. } => {
                self.op_path = Some(op_path.clone());
                self.api_key = None;
            }
            ControllerCredential::Pasted { key } => {
                // The pasted key exists nowhere else, so the config file is
                // the only place it can live. `resolve_api_key` falls back to
                // this field when no 1Password reference is configured.
                self.op_path = None;
                self.api_key = Some(key.clone());
            }
        }
    }

    /// Read the API key from 1Password via op-cache, falling back to the
    /// plaintext `api_key` field for backward compatibility.
    pub fn resolve_api_key(&self) -> Result<String> {
        resolve_secret(
            self.op_path.as_deref(),
            self.api_key.as_deref(),
            "No API key configured. Run 'ufa config setup'.",
        )
    }

    /// Read the Site Manager (cloud) API key from 1Password via op-cache,
    /// falling back to the plaintext `site_manager_api_key` field for
    /// backward compatibility.
    pub fn resolve_site_manager_api_key(&self) -> Result<String> {
        resolve_secret(
            self.sm_op_path.as_deref(),
            self.site_manager_api_key.as_deref(),
            "No Site Manager API key configured. Run 'ufa config cloud'.",
        )
    }

    /// Record the Site Manager answer gathered during interactive setup.
    ///
    /// An `op://` reference is stored as a 1Password path; anything else is
    /// treated as the key itself. An empty answer means "skip", so the
    /// currently configured credential — if any — is left untouched.
    fn set_site_manager(&mut self, answer: &str) {
        let answer = answer.trim();

        if answer.is_empty() {
            return;
        }

        if answer.starts_with(OP_REFERENCE_PREFIX) {
            self.sm_op_path = Some(answer.to_string());
            self.site_manager_api_key = None;
        } else {
            self.sm_op_path = None;
            self.site_manager_api_key = Some(answer.to_string());
        }
    }

    /// Whether the config file names a source for the controller API key.
    pub fn has_api_key(&self) -> bool {
        self.op_path.is_some() || self.api_key.is_some()
    }

    /// Whether the config file names a source for the Site Manager API key.
    pub fn has_site_manager_key(&self) -> bool {
        self.sm_op_path.is_some() || self.site_manager_api_key.is_some()
    }

    /// Save configuration to an explicit path, creating parent directories.
    ///
    /// Private on purpose: callers go through [`Config::edit`], which reads
    /// what is already on disk first, so no command can blank out a field it
    /// never asked about.
    fn save_to(&self, config_path: &Path) -> Result<()> {
        if let Some(config_dir) = config_path.parent() {
            fs::create_dir_all(config_dir).with_context(|| {
                format!(
                    "Failed to create config directory: {}",
                    config_dir.display()
                )
            })?;
        }

        let contents = toml::to_string_pretty(self).context("Failed to serialize configuration")?;

        fs::write(config_path, contents)
            .with_context(|| format!("Failed to write config file: {}", config_path.display()))?;

        restrict_to_owner(config_path).with_context(|| {
            format!(
                "Failed to restrict permissions on config file: {}",
                config_path.display()
            )
        })?;

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
                        break Selection::Network(network_discover_and_select().await?);
                    }
                }
                println!("Invalid choice. Please try again.");
            }
        } else {
            Selection::Network(network_discover_and_select().await?)
        };

        let (controller_url, credential) = match selection {
            Selection::Op(c) => {
                // Read the key via op-cache to verify it works
                let cache = op_cache::OpCache::new().map_err(|e| anyhow::anyhow!("{e}"))?;
                let path = op_cache::OpPath::new(&c.op_path).map_err(|e| anyhow::anyhow!("{e}"))?;
                let key = cache
                    .read(&path, None)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                (
                    c.url(),
                    ControllerCredential::OnePassword {
                        op_path: c.op_path,
                        key,
                    },
                )
            }
            Selection::Network(url) => {
                let key = prompt_for_api_key(&url)?;
                (url, ControllerCredential::Pasted { key })
            }
        };
        let api_key = credential.key().to_string();

        // Ask about certificate verification
        print!("\nSkip TLS certificate verification? (needed for self-signed certs) [y/N]: ");
        io::stdout().flush()?;

        let mut insecure_input = String::new();
        io::stdin().read_line(&mut insecure_input)?;
        let insecure = matches!(insecure_input.trim().to_lowercase().as_str(), "y" | "yes");

        // Test the connection
        println!("\n🔍 Testing connection...");
        match UnifiClient::new(&controller_url, &api_key, insecure).await {
            Ok(client) => match client.get::<crate::models::ApplicationInfo>("info").await {
                Ok(info) => {
                    println!("✅ Successfully connected to UniFi controller!");
                    println!("   Version: {}", info.application_version);
                }
                Err(e) => {
                    println!("⚠️  Connected but couldn't fetch info: {e}");
                    println!("   This might be normal if the API key has limited permissions.");
                }
            },
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
        let sm_answer = prompt_for_site_manager_key()?;

        // Save configuration
        let config = Self::edit(|config| {
            config.set_controller(controller_url, &credential, insecure);
            config.set_site_manager(&sm_answer);
        })?;

        println!("\n🎉 Configuration complete!");
        println!("You can now use ufa commands without specifying connection details.");
        if config.op_path.is_none() {
            println!(
                "The API key is stored in {}.",
                Self::config_file_path()?.display()
            );
            println!(
                "To keep it in 1Password instead, add it to the Private/ufa item and re-run setup."
            );
        }
        if config.has_site_manager_key() {
            println!("Cloud commands are available: try 'ufa cloud hosts'");
        }

        Ok(())
    }

    /// Interactive setup for the UniFi Site Manager (cloud) credential alone.
    ///
    /// Only the cloud credential is touched: the controller URL, its
    /// 1Password reference and the TLS choice are loaded from the saved
    /// configuration and written back unchanged.
    pub fn setup_site_manager() -> Result<()> {
        println!("Setting up UniFi Site Manager (Cloud) API credentials...\n");

        let answer = prompt_for_site_manager_key()?;
        let config = Self::edit(|config| config.set_site_manager(&answer))?;

        if config.has_site_manager_key() {
            println!("\nCloud commands are available: try 'ufa cloud hosts'");
        } else {
            println!("\nNo Site Manager API key configured; cloud commands stay unavailable.");
        }

        Ok(())
    }
}

/// Prompt for the Site Manager (cloud) credential.
///
/// The answer is either an `op://` reference, the key itself, or empty to skip.
fn prompt_for_site_manager_key() -> Result<String> {
    println!(
        "Paste the key from the unifi.ui.com API section, or a 1Password reference \
         ({OP_REFERENCE_PREFIX}Private/ufa/site manager key) to keep it out of the config file."
    );
    print!("Site Manager API key or 1Password reference [skip]: ");
    io::stdout().flush()?;

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().to_string())
}

/// Restrict a file that may contain secrets to its owner (mode 0600).
///
/// The config file can hold an API key the user pasted during setup and a
/// legacy plaintext Site Manager key, and it is created with the process umask
/// — typically 0644 — so it has to be tightened after writing.
#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

/// Windows has no umask and no mode bits to tighten; the file inherits the
/// containing directory's ACL, which is already per-user under `%APPDATA%`.
#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Resolve a secret that may live in 1Password.
///
/// A configured `op_path` always wins: if it is present but cannot be read the
/// error is reported rather than quietly downgrading to the plaintext copy,
/// which would hide a broken 1Password reference. `plaintext` is the legacy
/// in-config fallback, and `missing` is the message used when neither source is
/// configured.
fn resolve_secret(op_path: Option<&str>, plaintext: Option<&str>, missing: &str) -> Result<String> {
    if let Some(op_path) = op_path {
        let path = op_cache::OpPath::new(op_path).map_err(|e| anyhow::anyhow!("{e}"))?;
        let cache = op_cache::OpCache::new().map_err(|e| anyhow::anyhow!("{e}"))?;
        let secret = cache
            .read(&path, None)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        return Ok(secret);
    }
    if let Some(secret) = plaintext {
        return Ok(secret.to_string());
    }
    anyhow::bail!("{missing}")
}

/// Run network discovery (mDNS + common IPs) and let the user pick.
async fn network_discover_and_select() -> Result<String> {
    let controllers = discover_controllers().await?;

    if controllers.is_empty() {
        println!("No UniFi controllers found on the network.\n");
        return get_manual_controller_url().await;
    }

    println!(
        "\nFound {} controller(s) on the network:",
        controllers.len()
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A throwaway config directory that never touches the developer's real
    /// `~/.config/ufa`.
    ///
    /// The directory name is keyed on the process id *and* a nanosecond
    /// timestamp so two concurrent `cargo test` runs — or two tests inside one
    /// run — can never collide on the same file.
    struct TempConfigDir {
        dir: PathBuf,
    }

    impl TempConfigDir {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is after the Unix epoch")
                .as_nanos();
            let dir = std::env::temp_dir()
                .join(format!("ufa-config-{label}-{}-{nanos}", std::process::id()));
            Self { dir }
        }

        fn config_file(&self) -> PathBuf {
            self.dir.join("config.toml")
        }
    }

    impl Drop for TempConfigDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// A user with no 1Password `Private/ufa` item goes through the network
    /// discovery branch of `setup()` and pastes an API key. That key is the
    /// only copy in existence, so it has to survive the round trip to disk —
    /// otherwise every subsequent `ufa` command fails with "No API key
    /// configured. Run 'ufa config setup'", which just re-runs this wizard.
    #[test]
    fn pasted_api_key_survives_the_round_trip_to_disk() {
        let temp = TempConfigDir::new("pasted-key");
        let credential = ControllerCredential::Pasted {
            key: "pasted-api-key".to_string(),
        };

        let mut config = Config::default();
        config.set_controller("https://192.168.1.1".to_string(), &credential, true);
        config
            .save_to(&temp.config_file())
            .expect("saving the config must succeed");

        let loaded = Config::load_from(&temp.config_file())
            .expect("loading the config must succeed")
            .expect("the config file must exist after saving");

        let resolved = loaded
            .resolve_api_key()
            .expect("the key pasted during setup must still resolve after saving");
        assert_eq!(resolved, "pasted-api-key");
    }

    /// The config file can hold an API key in plaintext (the pasted-key path,
    /// and the legacy `site_manager_api_key` field), so it must never be
    /// readable by other users on the machine.
    #[cfg(unix)]
    #[test]
    fn saved_config_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempConfigDir::new("permissions");
        let config = Config {
            api_key: Some("plaintext-controller-key".to_string()),
            ..Config::default()
        };
        config
            .save_to(&temp.config_file())
            .expect("saving the config must succeed");

        let mode = fs::metadata(temp.config_file())
            .expect("the saved config file must exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "config file holding plaintext secrets must be owner-only, got {mode:o}"
        );
    }

    /// A configured 1Password reference wins over the legacy plaintext field.
    /// Falling back on failure would silently keep using a stale plaintext key
    /// after the user moved the credential into 1Password.
    #[test]
    fn site_manager_key_prefers_1password_over_the_plaintext_field() {
        // Deliberately not an `op://` reference: op-cache rejects it locally,
        // so the test never shells out to the `op` CLI.
        let config = Config {
            sm_op_path: Some("not-an-op-path".to_string()),
            site_manager_api_key: Some("stale-plaintext-key".to_string()),
            ..Config::default()
        };

        let error = config.resolve_site_manager_api_key().expect_err(
            "an unreadable 1Password reference must not fall back to the plaintext key",
        );
        assert!(
            error.to_string().contains("not-an-op-path"),
            "the failure must name the 1Password reference it could not read, got {error}"
        );
    }

    /// `ufa config cloud` announces that it configures cloud credentials, so
    /// it must leave the controller configuration exactly as it found it.
    #[test]
    fn configuring_the_cloud_key_preserves_the_controller_configuration() {
        let temp = TempConfigDir::new("cloud-preserves");
        let existing = Config {
            url: Some("https://192.168.1.1".to_string()),
            op_path: Some("op://Private/ufa/key - 192.168.1.1 port 443".to_string()),
            insecure: Some(true),
            ..Config::default()
        };
        existing
            .save_to(&temp.config_file())
            .expect("saving the existing config must succeed");

        Config::edit_at(&temp.config_file(), |config| {
            config.set_site_manager("op://Private/ufa/site manager key");
        })
        .expect("configuring the cloud key must succeed");

        let loaded = Config::load_from(&temp.config_file())
            .expect("loading the config must succeed")
            .expect("the config file must still exist");

        assert_eq!(loaded.url, existing.url, "controller URL must survive");
        assert_eq!(
            loaded.op_path, existing.op_path,
            "controller 1Password reference must survive"
        );
        assert_eq!(
            loaded.insecure, existing.insecure,
            "TLS verification choice must survive"
        );
        assert_eq!(
            loaded.sm_op_path.as_deref(),
            Some("op://Private/ufa/site manager key"),
            "the cloud credential must actually be recorded"
        );
    }

    /// Pressing Enter at the Site Manager prompt means "leave it alone", not
    /// "delete the key I set last time".
    #[test]
    fn skipping_the_site_manager_prompt_preserves_the_existing_key() {
        let temp = TempConfigDir::new("cloud-skip");
        let existing = Config {
            sm_op_path: Some("op://Private/ufa/site manager key".to_string()),
            ..Config::default()
        };
        existing
            .save_to(&temp.config_file())
            .expect("saving the existing config must succeed");

        Config::edit_at(&temp.config_file(), |config| config.set_site_manager(""))
            .expect("skipping the prompt must succeed");

        let loaded = Config::load_from(&temp.config_file())
            .expect("loading the config must succeed")
            .expect("the config file must still exist");

        assert_eq!(
            loaded.sm_op_path, existing.sm_op_path,
            "an empty answer must not clear the configured cloud credential"
        );
    }

    /// Configs written before `sm_op_path` existed keep working.
    #[test]
    fn site_manager_key_falls_back_to_the_plaintext_field() {
        let config = Config {
            site_manager_api_key: Some("legacy-plaintext-key".to_string()),
            ..Config::default()
        };

        assert_eq!(
            config
                .resolve_site_manager_api_key()
                .expect("a legacy plaintext key must still resolve"),
            "legacy-plaintext-key"
        );
    }
}
