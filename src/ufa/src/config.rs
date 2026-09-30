use crate::client::UnifiClient;
use crate::discovery::{discover_controllers, validate_user_url};
use crate::prompt::{self, Stdio};
use crate::secret::Secret;
use anyhow::{Context, Result};
use dirs::config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Prefix identifying a 1Password secret reference.
const OP_REFERENCE_PREFIX: &str = "op://";

/// The 1Password vault that holds the item `ufa` keeps its keys in.
const OP_VAULT: &str = "Private";

/// The 1Password item that holds the controller keys and the Site Manager key.
const OP_ITEM: &str = "ufa";

/// The start of a field label that names a controller key.
const CONTROLLER_LABEL_PREFIX: &str = "key - ";

/// What stands between the host and the port in a controller field label.
const PORT_SEPARATOR: &str = " port ";

/// Said when setup has a choice to make and no terminal to make it at.
///
/// This is the refusal for the numbered menus. The free-text questions of the
/// wizard refuse for themselves, in [`crate::prompt::ask_line`], for the same
/// reason: a piped run cannot answer a question, and a read that waits for an
/// answer that never comes never ends.
const NEEDS_A_TERMINAL: &str = "Setup needs a terminal to ask which controller to use. \
     Run 'ufa config setup' interactively, or pass --url and --api-key.";

/// The file of settings `ufa` reads before it parses its arguments.
const ENVIRONMENT_FILE: &str = ".env";

/// Load the settings file that sits beside the configuration file.
///
/// The load happens before the arguments are parsed, because clap reads
/// `UNIFI_URL`, `UNIFI_API_KEY` and `UNIFI_INSECURE` during the parse, and a
/// value in the environment beats the configuration file.
///
/// Exactly one path is read: `directory/.env`, and no other. `dotenvy::dotenv`
/// and `dotenvy::from_filename` both walk up from the working directory to the
/// root, which puts a settings file in every directory above the user on the
/// path — enough for one of them to name the controller the user's own API key
/// goes to, with certificate verification off. `dotenvy::from_path` searches
/// nothing.
///
/// The behaviour this changes: a settings file in a checkout, or in any other
/// directory `ufa` is run from, no longer applies. The file beside the
/// configuration file is the only one, because it is the only one the user
/// chose.
///
/// A file that is not there is the normal case and stays silent. Any other
/// fault is reported, because a settings file the user wrote and `ufa` cannot
/// read is a fault the user must see.
///
/// # Arguments
///
/// * `directory` - The directory that holds the configuration file.
///
/// # Returns
///
/// The path of the file that was loaded, or `None` when there is no such file.
pub fn load_environment_file(directory: &Path) -> Result<Option<PathBuf>> {
    let path = directory.join(ENVIRONMENT_FILE);

    match dotenvy::from_path(&path) {
        Ok(()) => Ok(Some(path)),
        Err(error) if error.not_found() => Ok(None),
        Err(error) => Err(anyhow::Error::new(error))
            .with_context(|| format!("Failed to read settings file: {}", path.display())),
    }
}

/// Whether `value` holds nothing but blank space.
///
/// A credential that is blank names no credential. It arrives as `api_key = ""`
/// in the configuration file, as `export UNIFI_API_KEY=`, or as `--api-key ""`,
/// and each of those says nothing rather than "authenticate with an empty
/// key". The test trims and the value never does, because a key the user
/// supplied is theirs: altering what goes to the controller would make a
/// working key fail for a reason nothing states.
///
/// # Arguments
///
/// * `value` - The value to test.
///
/// # Returns
///
/// Whether the value holds no visible character.
pub fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

/// Whether a configuration field names a secret rather than blank space.
///
/// # Arguments
///
/// * `field` - The field to test.
///
/// # Returns
///
/// Whether the field is present and holds more than blank space.
fn names_a_secret(field: Option<&str>) -> bool {
    field.is_some_and(|value| !is_blank(value))
}

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
///
/// Only the labels of that item reach this process. `ufa` used to run
/// `op item get ufa --vault Private --format json` itself, and that command
/// prints the value of every concealed field beside its label — so every
/// controller key in the item was read here to learn the names beside them,
/// outside the one crate this workspace keeps for 1Password access.
/// [`op_cache::field_labels`] runs `op` instead and answers with the labels
/// alone.
///
/// # Returns
///
/// One controller per field whose label names one.
///
/// # Errors
///
/// Returns an error if the 1Password CLI is missing, or if it cannot print the
/// `Private/ufa` item.
pub fn discover_op_controllers() -> Result<Vec<OpController>> {
    let item = op_cache::OpItem::new(OP_VAULT, OP_ITEM).map_err(|e| anyhow::anyhow!("{e}"))?;
    let labels = op_cache::field_labels(&item)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("Failed to list the fields of {OP_VAULT}/{OP_ITEM}"))?;

    Ok(controllers_from_labels(&labels))
}

/// The controllers that a list of field labels names.
///
/// A label that does not read as `key - <host> port <port>` names no
/// controller and is passed over: the same item holds the Site Manager key and
/// the note every 1Password item carries.
///
/// # Arguments
///
/// * `labels` - The labels of the fields of the `Private/ufa` item.
///
/// # Returns
///
/// One controller per label that names one, in the order the labels arrived.
fn controllers_from_labels(labels: &[String]) -> Vec<OpController> {
    labels
        .iter()
        .filter_map(|label| controller_from_label(label))
        .collect()
}

/// The controller a field label names, when it names one.
///
/// The port is what stands after the **last** separator, so a host that
/// carries those same words itself still reads.
///
/// # Arguments
///
/// * `label` - The label of one field of the `Private/ufa` item.
///
/// # Returns
///
/// The controller, or `None` when the label names none.
fn controller_from_label(label: &str) -> Option<OpController> {
    let (host, port) = label
        .strip_prefix(CONTROLLER_LABEL_PREFIX)?
        .rsplit_once(PORT_SEPARATOR)?;

    Some(OpController {
        host: host.to_string(),
        port: port.parse().ok()?,
        op_path: format!("{OP_REFERENCE_PREFIX}{OP_VAULT}/{OP_ITEM}/{label}"),
    })
}

/// The controller credential gathered during interactive setup.
///
/// Either the key already lives in 1Password (and setup merely read it to
/// verify the reference works), or the user pasted it in because no 1Password
/// entry exists for this controller.
#[derive(Debug, Clone)]
pub enum ControllerCredential {
    /// The key is stored in 1Password at `op_path`; `key` is its current value.
    OnePassword { op_path: String, key: Secret },
    /// The key was pasted during setup and exists nowhere else.
    Pasted { key: Secret },
}

impl ControllerCredential {
    /// The API key value, used to test the connection during setup.
    ///
    /// # Returns
    ///
    /// The key, still wrapped: `op_path` names an item and stays readable in a
    /// debug dump, and the key beside it must not.
    pub fn key(&self) -> &Secret {
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
    pub api_key: Option<Secret>,
    pub insecure: Option<bool>,
    /// Site Manager (cloud) API key stored directly in the config file.
    ///
    /// Legacy fallback: `sm_op_path` is preferred.
    pub site_manager_api_key: Option<Secret>,
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
    pub fn resolve_api_key(&self) -> Result<Secret> {
        resolve_secret(
            self.op_path.as_deref(),
            self.api_key.as_ref().map(Secret::expose),
            "No API key configured. Run 'ufa config setup'.",
        )
    }

    /// Read the Site Manager (cloud) API key from 1Password via op-cache,
    /// falling back to the plaintext `site_manager_api_key` field for
    /// backward compatibility.
    pub fn resolve_site_manager_api_key(&self) -> Result<Secret> {
        resolve_secret(
            self.sm_op_path.as_deref(),
            self.site_manager_api_key.as_ref().map(Secret::expose),
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
            self.site_manager_api_key = Some(Secret::from(answer));
        }
    }

    /// Whether the config file names a source for the controller API key.
    ///
    /// A field holding nothing but blank space names no source. It has to
    /// answer the same way `resolve_api_key` does, or a credential counts as
    /// configured and then reports that nothing is configured.
    pub fn has_api_key(&self) -> bool {
        names_a_secret(self.op_path.as_deref())
            || names_a_secret(self.api_key.as_ref().map(Secret::expose))
    }

    /// Whether the config file names a source for the Site Manager API key.
    ///
    /// Blank fields are read the same way [`Config::has_api_key`] reads them.
    pub fn has_site_manager_key(&self) -> bool {
        names_a_secret(self.sm_op_path.as_deref())
            || names_a_secret(self.site_manager_api_key.as_ref().map(Secret::expose))
    }

    /// Save configuration to an explicit path, creating parent directories.
    ///
    /// Private on purpose: callers go through [`Config::edit`], which reads
    /// what is already on disk first, so no command can blank out a field it
    /// never asked about.
    #[allow(
        clippy::print_stdout,
        reason = "only the setup wizard saves, through Config::edit, and the wizard is a conversation at a terminal with no --output format"
    )]
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

        let mut file = create_config_file(config_path)
            .with_context(|| format!("Failed to create config file: {}", config_path.display()))?;
        file.write_all(contents.as_bytes())
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
    #[allow(
        clippy::print_stdout,
        reason = "the setup wizard is a conversation at a terminal, and it has no --output format"
    )]
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

            match prompt::select_one(&mut Stdio, "Select a controller", network_idx)?
                .map(|index| index + 1)
            {
                Some(choice) if choice <= op_controllers.len() => {
                    Selection::Op(op_controllers[choice - 1].clone())
                }
                Some(choice) if choice == manual_idx => {
                    Selection::Network(get_manual_controller_url().await?)
                }
                Some(_) => Selection::Network(network_discover_and_select().await?),
                None => anyhow::bail!(NEEDS_A_TERMINAL),
            }
        } else {
            Selection::Network(network_discover_and_select().await?)
        };

        let (controller_url, credential) = match selection {
            Selection::Op(c) => {
                // Read the key via op-cache to verify it works
                let cache = op_cache::OpCache::new().map_err(|e| anyhow::anyhow!("{e}"))?;
                let path = op_cache::OpPath::new(&c.op_path).map_err(|e| anyhow::anyhow!("{e}"))?;
                let key = Secret::from(
                    cache
                        .read(&path, None)
                        .map_err(|e| anyhow::anyhow!("{e}"))?,
                );
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
        let api_key = credential.key().clone();

        // Ask about certificate verification
        let insecure = prompt::confirm(
            &mut Stdio,
            "\nSkip TLS certificate verification? (needed for self-signed certs)",
        )?;

        // Test the connection
        println!("\n🔍 Testing connection...");
        match UnifiClient::new(&controller_url, api_key.expose(), insecure) {
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
                if !prompt::confirm(&mut Stdio, "\nSave configuration anyway?")? {
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
    #[allow(
        clippy::print_stdout,
        reason = "the setup wizard is a conversation at a terminal, and it has no --output format"
    )]
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
#[allow(
    clippy::print_stdout,
    reason = "the setup wizard is a conversation at a terminal, and it has no --output format"
)]
fn prompt_for_site_manager_key() -> Result<String> {
    println!(
        "Paste the key from the unifi.ui.com API section, or a 1Password reference \
         ({OP_REFERENCE_PREFIX}Private/ufa/site manager key) to keep it out of the config file."
    );
    prompt::ask_line(
        &mut Stdio,
        "Site Manager API key or 1Password reference [skip]: ",
    )
}

/// The mode of a file that only its owner can read and write.
#[cfg(unix)]
const OWNER_ONLY: u32 = 0o600;

/// Create the configuration file, emptied and ready to be written, at a mode
/// no other user can read.
///
/// The file holds a controller key the user pasted during setup and a legacy
/// plaintext Site Manager key, and the mode goes on the open rather than on
/// the file afterwards. A create asks the kernel for 0666 and the kernel
/// subtracts the process umask, which lands on 0644 on a normal machine, so a
/// file narrowed only after the write is readable by every other user of that
/// machine for as long as the write takes — and stays that way for good when
/// the run is interrupted inside that window. Nothing in this crate handles a
/// signal, so nothing tidies up after such a run.
///
/// The mode reaches a file this call **creates** and no other, which is why
/// [`restrict_to_owner`] stays: a file that was already there keeps the mode
/// it already had, and a configuration file written by an earlier version of
/// `ufa` is exactly such a file.
///
/// # Arguments
///
/// * `path` - Where the configuration file goes.
///
/// # Returns
///
/// The open file, truncated to nothing.
#[cfg(unix)]
fn create_config_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(OWNER_ONLY)
        .open(path)
}

/// Create the configuration file, emptied and ready to be written.
///
/// Windows has no umask and no mode bits to ask for; the file inherits the
/// containing directory's ACL, which is already per-user under `%APPDATA%`.
///
/// # Arguments
///
/// * `path` - Where the configuration file goes.
///
/// # Returns
///
/// The open file, truncated to nothing.
#[cfg(not(unix))]
fn create_config_file(path: &Path) -> std::io::Result<fs::File> {
    fs::File::create(path)
}

/// Restrict a file that may contain secrets to its owner (mode 0600).
///
/// [`create_config_file`] already asks for that mode, and the kernel grants it
/// only to a file that open **created**. So this covers the other case: a
/// configuration file that was already on disk at a wider mode, written by a
/// version of `ufa` that created it at the process umask.
#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(OWNER_ONLY))
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
fn resolve_secret(op_path: Option<&str>, plaintext: Option<&str>, missing: &str) -> Result<Secret> {
    resolve_secret_from(read_from_1password, op_path, plaintext, missing)
}

/// Read the secret the 1Password reference `op_path` names, through op-cache.
///
/// # Arguments
///
/// * `op_path` - The 1Password reference to read.
///
/// # Returns
///
/// Whatever 1Password holds at that reference.
fn read_from_1password(op_path: &str) -> Result<String> {
    let path = op_cache::OpPath::new(op_path).map_err(|e| anyhow::anyhow!("{e}"))?;
    let cache = op_cache::OpCache::new().map_err(|e| anyhow::anyhow!("{e}"))?;

    cache.read(&path, None).map_err(|e| anyhow::anyhow!("{e}"))
}

/// [`resolve_secret`] against an explicit reader.
///
/// The reader is a parameter so a test can say what 1Password answered without
/// a vault, a biometric prompt, or a network.
///
/// A blank field and a blank answer from 1Password are different faults and
/// get different answers. A field that holds only blank space names nothing at
/// all, so it is skipped and the run ends on `missing`, which is the advice
/// that sends the user to the wizard. An answer that holds only blank space
/// comes from an item the user named, so the reference is broken rather than
/// absent, and it is reported the way any other unreadable reference is —
/// without downgrading to the plaintext copy.
///
/// # Arguments
///
/// * `read_reference` - Reads the secret a 1Password reference names.
/// * `op_path` - The configured 1Password reference, when there is one.
/// * `plaintext` - The legacy in-config value, when there is one.
/// * `missing` - Said when neither source is configured.
///
/// # Returns
///
/// The secret, exactly as its source holds it.
fn resolve_secret_from(
    read_reference: impl FnOnce(&str) -> Result<String>,
    op_path: Option<&str>,
    plaintext: Option<&str>,
    missing: &str,
) -> Result<Secret> {
    if let Some(op_path) = op_path.filter(|reference| !is_blank(reference)) {
        let secret = read_reference(op_path)?;

        if is_blank(&secret) {
            anyhow::bail!(
                "The 1Password reference {op_path} holds no key. Put the key in that field, \
                 or run 'ufa config setup' to name another reference."
            );
        }

        return Ok(Secret::from(secret));
    }
    if let Some(secret) = plaintext.filter(|value| !is_blank(value)) {
        return Ok(Secret::from(secret));
    }
    anyhow::bail!("{missing}")
}

/// Run network discovery (mDNS + common IPs) and let the user pick.
#[allow(
    clippy::print_stdout,
    reason = "the setup wizard is a conversation at a terminal, and it has no --output format"
)]
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

    match prompt::select_one(&mut Stdio, "Select a controller", manual_idx)? {
        Some(index) if index < controllers.len() => Ok(controllers[index].url()),
        Some(_) => get_manual_controller_url().await,
        None => anyhow::bail!(NEEDS_A_TERMINAL),
    }
}

#[allow(
    clippy::print_stdout,
    reason = "the setup wizard is a conversation at a terminal, and it has no --output format"
)]
async fn get_manual_controller_url() -> Result<String> {
    loop {
        let url = prompt::ask_line(
            &mut Stdio,
            "Enter your UniFi controller URL (e.g., https://192.168.1.1): ",
        )?;
        let url = url.as_str();

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
        std::io::stdout().flush()?;

        match validate_user_url(&url).await {
            Ok(controller) => {
                println!(" ✓");
                return Ok(controller.url());
            }
            Err(e) => {
                println!(" ✗");
                println!("Failed to validate controller: {e}");
                if prompt::confirm(&mut Stdio, "Use this URL anyway?")? {
                    return Ok(url);
                }
            }
        }
    }
}

/// Prompt for an API key when no 1Password entry exists.
#[allow(
    clippy::print_stdout,
    reason = "the setup wizard is a conversation at a terminal, and it has no --output format"
)]
fn prompt_for_api_key(controller_url: &str) -> Result<Secret> {
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

    let api_key = prompt::ask_line(&mut Stdio, "\nPaste your API key here: ")?;

    if api_key.is_empty() {
        anyhow::bail!("API key cannot be empty");
    }

    Ok(Secret::from(api_key))
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
            key: "pasted-api-key".into(),
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
        assert_eq!(resolved.expose(), "pasted-api-key");
    }

    /// Every `ufa` user already has a `config.toml` on disk, written when the
    /// credentials were plain `String` fields. The type that holds them now
    /// reads and writes as a plain string for that reason, and this is the
    /// file it has to keep reading: written by hand, in the shape the old
    /// serializer produced, and never through this crate's own save.
    #[test]
    fn a_configuration_file_written_before_the_credentials_were_wrapped_still_loads() {
        let temp = TempConfigDir::new("legacy-file");
        create_directory(&temp.dir);
        write_file(
            &temp.config_file(),
            "url = \"https://192.168.1.1\"\n\
             api_key = \"key-from-an-older-ufa\"\n\
             insecure = true\n\
             site_manager_api_key = \"cloud-key-from-an-older-ufa\"\n",
        );

        let loaded = Config::load_from(&temp.config_file())
            .expect("loading the config must succeed")
            .expect("the config file must exist");

        assert_eq!(
            loaded
                .resolve_api_key()
                .expect("the controller key must still resolve")
                .expose(),
            "key-from-an-older-ufa"
        );
        assert_eq!(
            loaded
                .resolve_site_manager_api_key()
                .expect("the cloud key must still resolve")
                .expose(),
            "cloud-key-from-an-older-ufa"
        );
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
            api_key: Some("plaintext-controller-key".into()),
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

    /// The mode a file ends at says nothing about the mode it was created at.
    ///
    /// The file holds a pasted controller key and a legacy plaintext Site
    /// Manager key, and it is created before either is written and before the
    /// mode is narrowed. A create at the process umask lands on 0644 on a
    /// normal machine, so every save hands every other user of that machine a
    /// window in which the file is readable — and a run interrupted inside the
    /// window leaves it that way for good.
    ///
    /// The umask is stated rather than inherited. A developer whose shell sets
    /// `umask 077` would otherwise watch a 0666 create land on 0600 and read
    /// that as the code doing the right thing.
    #[cfg(unix)]
    #[test]
    fn the_config_file_is_created_at_a_mode_no_other_user_can_read() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempConfigDir::new("creation-mode");
        create_directory(&temp.dir);
        let path = temp.config_file();

        let mode = crate::test_support::with_umask(0o000, || {
            let file = create_config_file(&path).expect("creating the config file must succeed");
            drop(file);

            fs::metadata(&path)
                .expect("the created config file must exist")
                .permissions()
                .mode()
                & 0o777
        });

        assert_eq!(
            mode, 0o600,
            "the config file holds cleartext keys, so no other user may read it \
             at any point in its life, got {mode:o}"
        );
    }

    /// The mode an open asks for reaches a file that open created, and no
    /// other. A configuration file written by a version of `ufa` that created
    /// it at the process umask is already on disk at 0644, and the save that
    /// tightens it is the pass that runs after the write — so that pass stays,
    /// and this is what it covers.
    #[cfg(unix)]
    #[test]
    fn saving_over_a_file_that_was_already_wider_narrows_it() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempConfigDir::new("already-wider");
        create_directory(&temp.dir);
        let path = temp.config_file();
        write_file(&path, "url = \"https://192.168.1.1\"\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("the existing file must be settable to a wider mode");

        let config = Config {
            api_key: Some("plaintext-controller-key".into()),
            ..Config::default()
        };
        config
            .save_to(&path)
            .expect("saving over the existing file must succeed");

        let mode = fs::metadata(&path)
            .expect("the saved config file must exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "a file that was already readable by others must be narrowed by the \
             save, got {mode:o}"
        );
    }

    /// A 1Password reference names the item that holds the key. It is not the
    /// key, and a dump that hides it says nothing a reader can act on.
    const OP_REFERENCE: &str = "op://Private/ufa/key - 192.168.1.1 port 443";

    /// Nothing formats a `Config` with `{:?}` today, so nothing leaks today.
    /// The type is one `eprintln!` added while debugging away from printing a
    /// pasted controller key and a legacy plaintext Site Manager key in full,
    /// and a panic message that carries the config does the same on a machine
    /// nobody chose.
    #[test]
    fn a_debug_dump_of_the_configuration_redacts_the_keys() {
        use crate::test_support::{FAKE_CLOUD_KEY, FAKE_CONTROLLER_KEY, REDACTED};

        let config = Config {
            url: Some("https://192.168.1.1".to_string()),
            api_key: Some(FAKE_CONTROLLER_KEY.into()),
            site_manager_api_key: Some(FAKE_CLOUD_KEY.into()),
            op_path: Some(OP_REFERENCE.to_string()),
            insecure: Some(true),
            sm_op_path: None,
        };

        let dump = format!("{config:?}");

        assert!(
            !dump.contains(FAKE_CONTROLLER_KEY),
            "the controller key must not reach a debug dump, got {dump}"
        );
        assert!(
            !dump.contains(FAKE_CLOUD_KEY),
            "the Site Manager key must not reach a debug dump, got {dump}"
        );
        assert_eq!(
            dump.matches(REDACTED).count(),
            2,
            "both credentials must say {REDACTED} in place of what they hold, got {dump}"
        );
        assert!(
            dump.contains(OP_REFERENCE),
            "the 1Password reference names an item rather than holding a key, and \
             a dump that hides it helps nobody, got {dump}"
        );
        assert!(
            dump.contains("192.168.1.1"),
            "the controller URL is not a secret either, got {dump}"
        );
    }

    /// The credential setup gathers, in both of its shapes.
    #[test]
    fn a_debug_dump_of_the_controller_credential_redacts_the_key() {
        use crate::test_support::{FAKE_CONTROLLER_KEY, REDACTED};

        for credential in [
            ControllerCredential::OnePassword {
                op_path: OP_REFERENCE.to_string(),
                key: FAKE_CONTROLLER_KEY.into(),
            },
            ControllerCredential::Pasted {
                key: FAKE_CONTROLLER_KEY.into(),
            },
        ] {
            let dump = format!("{credential:?}");

            assert!(
                !dump.contains(FAKE_CONTROLLER_KEY),
                "the key must not reach a debug dump, got {dump}"
            );
            assert!(
                dump.contains(REDACTED),
                "the key must say {REDACTED} in place of what it holds, got {dump}"
            );
        }

        let from_1password = ControllerCredential::OnePassword {
            op_path: OP_REFERENCE.to_string(),
            key: FAKE_CONTROLLER_KEY.into(),
        };
        assert!(
            format!("{from_1password:?}").contains(OP_REFERENCE),
            "the reference names an item rather than holding a key, so it stays \
             readable"
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
            site_manager_api_key: Some("stale-plaintext-key".into()),
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

    /// Load the settings file beside `directory` under the environment lock,
    /// read `name` back, and take it out again.
    ///
    /// The load writes into the process environment, which every test in this
    /// binary shares, so the lock is held for the whole of it and whatever
    /// arrived is removed before the lock is released.
    ///
    /// # Arguments
    ///
    /// * `name` - The setting the loaded file is expected to carry.
    /// * `directory` - The directory to load the settings file from.
    ///
    /// # Returns
    ///
    /// What the load returned, and the value `name` held after it.
    fn load_and_take_back(
        name: &str,
        directory: &Path,
    ) -> (Result<Option<PathBuf>>, Option<String>) {
        crate::test_support::with_environment_lock(|| {
            let loaded = load_environment_file(directory);
            let value = std::env::var(name).ok();
            std::env::remove_var(name);

            (loaded, value)
        })
    }

    /// Create the directory `path` names, and every directory above it.
    fn create_directory(path: &Path) {
        fs::create_dir_all(path)
            .unwrap_or_else(|error| panic!("{} must be creatable: {error}", path.display()));
    }

    /// Write `contents` to `path`.
    fn write_file(path: &Path, contents: &str) {
        fs::write(path, contents)
            .unwrap_or_else(|error| panic!("{} must be writable: {error}", path.display()));
    }

    /// The settings file names `UNIFI_URL`, `UNIFI_API_KEY` and
    /// `UNIFI_INSECURE`, and clap gives a value in the environment precedence
    /// over the configuration file. A search that walks up from the directory
    /// therefore lets any directory above it name the controller the user's
    /// own API key goes to, with certificate verification off. Only the file
    /// the user put beside the configuration file counts.
    #[test]
    fn a_settings_file_above_the_configuration_directory_is_not_loaded() {
        let temp = TempConfigDir::new("env-above");
        let config_directory = temp.dir.join("ufa");
        create_directory(&config_directory);
        write_file(
            &temp.dir.join(ENVIRONMENT_FILE),
            "UNIFI_URL=https://foreign.example\n",
        );

        let (loaded, url) = load_and_take_back("UNIFI_URL", &config_directory);

        assert_eq!(
            loaded.expect("a directory with no settings file beside it is not an error"),
            None,
            "no file may be loaded when the configuration directory holds none"
        );
        assert_eq!(
            url, None,
            "a settings file above the configuration directory must not reach the process \
             environment"
        );
    }

    /// A file that cannot be read is a different answer from a file that is
    /// not there. The first is a fault the user must see; the second is the
    /// normal case for everybody who keeps no settings file.
    #[test]
    fn a_settings_file_that_cannot_be_read_is_reported() {
        let temp = TempConfigDir::new("env-unreadable");
        create_directory(&temp.dir);
        // A directory of this name cannot be opened as a file, which gives an
        // I/O failure that is not "no such file".
        create_directory(&temp.dir.join(ENVIRONMENT_FILE));

        let (loaded, _) = load_and_take_back("UNIFI_URL", &temp.dir);

        let error = loaded.expect_err("an unreadable settings file must not be discarded");
        assert!(
            format!("{error:#}").contains(ENVIRONMENT_FILE),
            "the failure must name the file it could not read, got {error:#}"
        );
    }

    /// The file the user put beside the configuration file is the one that is
    /// read, and its settings reach the process environment.
    #[test]
    fn the_settings_file_beside_the_configuration_file_is_loaded() {
        let temp = TempConfigDir::new("env-beside");
        create_directory(&temp.dir);
        write_file(
            &temp.dir.join(ENVIRONMENT_FILE),
            "UNIFI_URL=https://beside.example\n",
        );

        let (loaded, url) = load_and_take_back("UNIFI_URL", &temp.dir);

        assert_eq!(
            loaded.expect("the settings file must load"),
            Some(temp.dir.join(ENVIRONMENT_FILE)),
            "the load must report the file it read"
        );
        assert_eq!(
            url.as_deref(),
            Some("https://beside.example"),
            "the settings file beside the configuration file must reach the process environment"
        );
    }

    /// Most users keep no settings file at all, so its absence is silent.
    #[test]
    fn a_configuration_directory_without_a_settings_file_loads_nothing() {
        let temp = TempConfigDir::new("env-absent");
        create_directory(&temp.dir);

        let (loaded, url) = load_and_take_back("UNIFI_URL", &temp.dir);

        assert_eq!(
            loaded.expect("a missing settings file must not be an error"),
            None,
            "there is no file to report"
        );
        assert_eq!(url, None, "nothing may reach the process environment");
    }

    /// A field holding nothing but blank space names no key. It still made
    /// `has_api_key` answer true, so the credential counted as configured and
    /// the blank value went into the header and out to the controller. The
    /// user then got a generic 401 instead of the advice to run the wizard.
    #[test]
    fn a_blank_api_key_field_is_not_a_configured_key() {
        for blank in ["", "   ", "\n", "\t "] {
            let config = Config {
                api_key: Some(blank.into()),
                ..Config::default()
            };

            assert!(
                !config.has_api_key(),
                "{blank:?} names no key, so the controller key is not configured"
            );

            let error = config
                .resolve_api_key()
                .expect_err("a blank field must not resolve as a key");
            assert!(
                format!("{error:#}").contains("ufa config setup"),
                "a blank field must earn the advice a missing one earns, got {error:#}"
            );
        }
    }

    /// The cloud credential has the same two fields and the same predicate.
    #[test]
    fn a_blank_site_manager_key_field_is_not_a_configured_key() {
        let config = Config {
            site_manager_api_key: Some("   ".into()),
            ..Config::default()
        };

        assert!(
            !config.has_site_manager_key(),
            "a blank field names no Site Manager key"
        );

        let error = config
            .resolve_site_manager_api_key()
            .expect_err("a blank field must not resolve as a key");
        assert!(
            format!("{error:#}").contains("ufa config cloud"),
            "a blank field must earn the advice a missing one earns, got {error:#}"
        );
    }

    /// A blank 1Password reference names no item, so it is a blank placeholder
    /// like a blank key rather than a reference that failed to read. The
    /// predicate and the resolution have to agree about that, or a config
    /// counts as configured and then reports that nothing is configured.
    #[test]
    fn a_blank_op_path_is_not_a_configured_reference() {
        let config = Config {
            op_path: Some("  ".to_string()),
            ..Config::default()
        };

        assert!(
            !config.has_api_key(),
            "a blank reference names no key, so the controller key is not configured"
        );

        let error = config
            .resolve_api_key()
            .expect_err("a blank reference must not resolve as a key");
        assert!(
            format!("{error:#}").contains("ufa config setup"),
            "a blank reference must earn the advice a missing one earns, got {error:#}"
        );
    }

    /// A blank answer from 1Password is a different fault from a blank field.
    /// The user named a specific item, so the reference is broken rather than
    /// absent — and `resolve_secret` already refuses to downgrade a broken
    /// reference to the plaintext copy. The answer is an error that names the
    /// reference, not the advice to run the wizard.
    #[test]
    fn a_blank_answer_from_1password_is_a_broken_reference() {
        let reference = "op://Private/ufa/key - 192.168.1.1 port 443";

        let error = resolve_secret_from(
            |_| Ok("   ".to_string()),
            Some(reference),
            Some("stale-plaintext-key"),
            "No API key configured. Run 'ufa config setup'.",
        )
        .expect_err("a blank answer from 1Password must not resolve as a key");
        let report = format!("{error:#}");

        assert!(
            report.contains(reference),
            "the failure must name the reference that holds nothing, got {report}"
        );
        assert!(
            !report.contains("No API key configured"),
            "a broken reference is not an absent one, so the missing-credential message is \
             wrong, got {report}"
        );
        assert!(
            !report.contains("stale-plaintext-key"),
            "a broken reference must not downgrade to the plaintext copy, got {report}"
        );
    }

    /// The emptiness test trims. The value does not: a key the user supplied
    /// is theirs, and silently altering what goes to the controller would make
    /// a working key fail for a reason nothing states.
    #[test]
    fn a_key_that_holds_more_than_blank_space_is_resolved_untouched() {
        let config = Config {
            api_key: Some(" padded-key\n".into()),
            ..Config::default()
        };

        assert!(config.has_api_key(), "the field names a key");
        assert_eq!(
            config
                .resolve_api_key()
                .expect("a key that is not blank must resolve")
                .expose(),
            " padded-key\n",
            "the resolved key must be what its source holds, byte for byte"
        );

        assert_eq!(
            resolve_secret_from(
                |_| Ok(" padded-op-key\n".to_string()),
                Some("op://Private/ufa/key"),
                None,
                "missing",
            )
            .expect("a 1Password answer that is not blank must resolve")
            .expose(),
            " padded-op-key\n",
            "what 1Password answered must reach the caller untouched"
        );
    }

    /// The labels of the fields of a `Private/ufa` item that holds two
    /// controller keys, the Site Manager key, and the note every 1Password
    /// item carries.
    fn labels_of_the_ufa_item() -> Vec<String> {
        [
            "notesPlain",
            "key - 192.168.1.1 port 443",
            "site manager key",
            "key - unifi.example.com port 8443",
        ]
        .iter()
        .map(|label| (*label).to_string())
        .collect()
    }

    /// A field label of the shape `key - <host> port <port>` names one
    /// controller, and the reference of the key that reaches it.
    #[test]
    fn a_field_label_that_names_a_controller_becomes_one() {
        let controllers = controllers_from_labels(&labels_of_the_ufa_item());

        assert_eq!(
            controllers.len(),
            2,
            "the item names two controllers, got {controllers:?}"
        );

        assert_eq!(controllers[0].host, "192.168.1.1");
        assert_eq!(controllers[0].port, 443);
        assert_eq!(controllers[0].url(), "https://192.168.1.1:443");
        assert_eq!(
            controllers[0].op_path, "op://Private/ufa/key - 192.168.1.1 port 443",
            "the reference must name the field the label came from"
        );

        assert_eq!(controllers[1].host, "unifi.example.com");
        assert_eq!(controllers[1].port, 8443);
        assert_eq!(
            controllers[1].op_path,
            "op://Private/ufa/key - unifi.example.com port 8443"
        );
    }

    /// The item holds fields that name no controller, and a label that almost
    /// reads as one names none either.
    #[test]
    fn a_field_label_that_names_no_controller_is_passed_over() {
        for label in [
            "site manager key",
            "notesPlain",
            // No port at all.
            "key - 192.168.1.1",
            // Nothing after the separator.
            "key - 192.168.1.1 port ",
            // No port is that high.
            "key - 192.168.1.1 port 99999",
            // A service name is not a port number.
            "key - 192.168.1.1 port https",
            // The prefix is exactly "key - ".
            "KEY - 192.168.1.1 port 443",
            "  key - 192.168.1.1 port 443",
        ] {
            assert!(
                controllers_from_labels(&[label.to_string()]).is_empty(),
                "{label:?} names no controller"
            );
        }
    }

    /// The port is what stands after the *last* separator, so a host that
    /// carries those same words still reads.
    #[test]
    fn the_last_separator_in_a_label_is_the_one_that_names_the_port() {
        let controllers = controllers_from_labels(&["key - port forward port 8443".to_string()]);

        assert_eq!(
            controllers.len(),
            1,
            "the label names one controller, got {controllers:?}"
        );
        assert_eq!(controllers[0].host, "port forward");
        assert_eq!(controllers[0].port, 8443);
    }

    /// Configs written before `sm_op_path` existed keep working.
    #[test]
    fn site_manager_key_falls_back_to_the_plaintext_field() {
        let config = Config {
            site_manager_api_key: Some("legacy-plaintext-key".into()),
            ..Config::default()
        };

        assert_eq!(
            config
                .resolve_site_manager_api_key()
                .expect("a legacy plaintext key must still resolve")
                .expose(),
            "legacy-plaintext-key"
        );
    }
}
