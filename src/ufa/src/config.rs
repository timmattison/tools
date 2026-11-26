use crate::client::UnifiClient;
use crate::discovery::{discover_controllers, origin_url, validate_user_url};
use crate::http::answered_status;
use crate::prompt::{self, Console, Stdio};
use crate::secret::Secret;
use anyhow::{Context, Result};
use dirs::config_dir;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
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

/// The name of the file that holds what op-cache read from 1Password.
const OP_CACHE_FILE: &str = ".op-cache.json";

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
/// The load happens before the arguments are parsed, because clap reads the
/// `UNIFI_*` variables during the parse, and a value in the environment beats
/// the configuration file. `UNIFI_SITE_MANAGER_API_KEY` is one of them, so the
/// settings file can give the cloud key too.
///
/// Exactly one path is read: `directory/.env`, and no other. `dotenvy::dotenv`
/// and `dotenvy::from_filename` both walk up from the working directory to the
/// root, which puts a settings file in every directory above the user on the
/// path — enough for one of them to name the controller the user's own API key
/// goes to, with certificate verification off. The function reads that one path
/// with `fs::read` and hands the bytes to `dotenvy::from_read`, which searches
/// nothing.
///
/// The behaviour this changes: a settings file in a checkout, or in any other
/// directory `ufa` is run from, no longer applies. The file beside the
/// configuration file is the only one, because it is the only one the user
/// chose.
///
/// A file that is not there is the normal case and stays silent. Any other
/// fault is reported, because a settings file the user wrote and `ufa` cannot
/// read is a fault the user must see. A line that cannot be parsed is
/// reported by the line where its setting starts, and no text of it is shown:
/// dotenvy puts the text of the line into its error, and that text holds the
/// value, which is often a key. The position that dotenvy gives has a different
/// unit for each kind of error, so a [`LineCountingReader`] counts the lines
/// that dotenvy reads instead.
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

    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read settings file: {}", path.display()));
        }
    };
    let mut reader = LineCountingReader::new(&bytes);

    match dotenvy::from_read(&mut reader) {
        Ok(()) => Ok(Some(path)),
        Err(dotenvy::Error::LineParse(text, _)) => {
            let line = reader.lines_begun().saturating_sub(newlines_inside(&text));
            Err(anyhow::anyhow!(
                "Failed to parse settings file {}: the setting that starts on line {line} \
                 cannot be parsed. The line is not shown, because it can hold a key. \
                 Put quotes around a value that holds a space.",
                path.display()
            ))
        }
        Err(error) => Err(anyhow::Error::new(error))
            .with_context(|| format!("Failed to read settings file: {}", path.display())),
    }
}

/// Count the line breaks that sit inside the text of one setting.
///
/// A final line break ends the last line of the setting, so it is not inside.
///
/// # Arguments
///
/// * `text` - The text that dotenvy puts into a parse error.
///
/// # Returns
///
/// How many lines the setting spans after its first line.
fn newlines_inside(text: &str) -> usize {
    let breaks = text.matches('\n').count();
    if text.ends_with('\n') {
        breaks.saturating_sub(1)
    } else {
        breaks
    }
}

/// A reader that counts the physical lines it has handed out.
///
/// dotenvy wraps its reader in a `BufReader` and reads one line at a time. The
/// buffer refills only when it is empty, and this reader hands out at most one
/// line for each read. Thus dotenvy never holds a line it has not reached, and
/// when it reports its first error, the count is the last line of the setting
/// that failed.
struct LineCountingReader<'a> {
    /// The bytes that are not handed out yet.
    rest: &'a [u8],
    /// Whether the next byte is the first byte of a line.
    at_line_start: bool,
    /// How many lines have begun.
    lines_begun: usize,
}

impl<'a> LineCountingReader<'a> {
    /// Make a reader over `bytes` that has begun no line.
    ///
    /// # Arguments
    ///
    /// * `bytes` - The whole content of the settings file.
    ///
    /// # Returns
    ///
    /// The new reader.
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            rest: bytes,
            at_line_start: true,
            lines_begun: 0,
        }
    }

    /// How many lines have begun.
    ///
    /// # Returns
    ///
    /// The count of lines whose first byte was handed out.
    fn lines_begun(&self) -> usize {
        self.lines_begun
    }
}

impl io::Read for LineCountingReader<'_> {
    /// Hand out at most the rest of the current line, and never more than
    /// `buf` holds.
    ///
    /// # Arguments
    ///
    /// * `buf` - Where the bytes go.
    ///
    /// # Returns
    ///
    /// The number of bytes written, which is 0 for an empty `buf` or at the end.
    ///
    /// # Errors
    ///
    /// An error when the count of bytes does not fit the rest of the file or
    /// `buf`. The count is at most the length of each, thus no read gives it.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.rest.is_empty() {
            return Ok(0);
        }
        let line_end = self
            .rest
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(self.rest.len(), |index| index.saturating_add(1));
        let count = line_end.min(buf.len());
        let Some((head, tail)) = self.rest.split_at_checked(count) else {
            return Err(io::Error::other(
                "the line ends past the end of the settings file",
            ));
        };
        let Some(target) = buf.get_mut(..count) else {
            return Err(io::Error::other("the line is longer than the read buffer"));
        };
        target.copy_from_slice(head);
        if self.at_line_start {
            self.lines_begun = self.lines_begun.saturating_add(1);
        }
        self.at_line_start = head.last() == Some(&b'\n');
        self.rest = tail;
        Ok(head.len())
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
    /// The URL of this controller.
    ///
    /// # Returns
    ///
    /// The origin URL, as [`origin_url`] builds it.
    ///
    /// # Errors
    ///
    /// Returns an error if no URL can hold the host that the field label
    /// names.
    pub fn url(&self) -> Result<String> {
        origin_url(&self.host, self.port)
            .with_context(|| format!("The 1Password field {} names no usable host", self.op_path))
    }
}

/// Discover controllers stored in the 1Password `Private/ufa` item.
///
/// Fields with labels matching `key - <host> port <port>` are parsed.
///
/// `ufa` used to run `op item get ufa --vault Private --format json` itself,
/// and that command prints the value of every concealed field beside its
/// label — so every controller key in the item was read here to learn the
/// names beside them, outside the one crate this workspace keeps for 1Password
/// access. [`op_cache::field_labels`] runs that command instead. The values
/// still reach this process, in a buffer inside `op-cache`, and stop there: no
/// value reaches a type or a variable of `ufa`.
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
    /// `sm_op_path` is preferred. Setup writes this field when the user pastes
    /// a key that is not a 1Password reference.
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
    ///
    /// # Arguments
    ///
    /// * `keys` - The store that reads the 1Password reference in `op_path`.
    ///
    /// # Returns
    ///
    /// The key, and the reference it came from when it came from one.
    ///
    /// # Errors
    ///
    /// Returns an error if no key is configured, and if the configured
    /// reference cannot be read or holds only blank space.
    pub fn resolve_api_key(&self, keys: &impl KeyStore) -> Result<ResolvedKey> {
        resolve_secret_from(
            |reference| keys.read(reference),
            self.op_path.as_deref(),
            self.api_key.as_ref().map(Secret::expose),
            "No API key configured. Run 'ufa config setup'.",
        )
    }

    /// Read the Site Manager (cloud) API key from 1Password via op-cache,
    /// falling back to the plaintext `site_manager_api_key` field for
    /// backward compatibility.
    ///
    /// # Arguments
    ///
    /// * `keys` - The store that reads the 1Password reference in
    ///   `sm_op_path`.
    ///
    /// # Returns
    ///
    /// The key, and the reference it came from when it came from one.
    ///
    /// # Errors
    ///
    /// Returns an error if no key is configured, and if the configured
    /// reference cannot be read or holds only blank space.
    pub fn resolve_site_manager_api_key(&self, keys: &impl KeyStore) -> Result<ResolvedKey> {
        resolve_secret_from(
            |reference| keys.read(reference),
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
    #[expect(
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

        replace_config_file(config_path, |file| file.write_all(contents.as_bytes()))
            .with_context(|| format!("Failed to write config file: {}", config_path.display()))?;

        println!("Configuration saved to: {}", config_path.display());
        Ok(())
    }

    /// Interactive configuration setup
    #[expect(
        clippy::print_stdout,
        reason = "the setup wizard is a conversation at a terminal, and it has no --output format"
    )]
    pub async fn setup() -> Result<()> {
        println!("🚀 UniFi API Configuration Setup");
        println!("================================\n");

        // Step 1: Try 1Password first, then fall back to network discovery
        let op_controllers =
            controllers_to_offer(discover_op_controllers(), &mut std::io::stdout())?;

        #[derive(Debug)]
        enum Selection {
            Op(OpController),
            Network(String),
        }

        let selection = if !op_controllers.is_empty() {
            println!("Found {} controller(s) in 1Password:", op_controllers.len());
            for (i, c) in op_controllers.iter().enumerate() {
                match c.url() {
                    Ok(url) => println!("  {}. {url}", i + 1),
                    Err(e) => println!("  {}. {e:#}", i + 1),
                }
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
                let url = c.url()?;
                // Read the key to test it below. A user who rotated the key in
                // 1Password runs the wizard to use the new key, so the read
                // forgets any copy that op-cache kept, and 1Password answers.
                let key = read_again_from_1password(
                    &CachedOnePassword::beside_the_configuration()?,
                    &c.op_path,
                )?;
                (
                    url,
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
        let test = run_connection_test(&controller_url, &api_key, insecure).await;
        if decide_after_connection_test(test, &mut std::io::stdout(), &mut Stdio)?
            == SaveDecision::DoNotSave
        {
            anyhow::bail!("Configuration not saved");
        }

        // Site Manager API Key (optional)
        println!("\n\nOptional: UniFi Site Manager (Cloud) Configuration");
        let sm_answer = prompt_for_site_manager_key(&mut Stdio)?;

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
    #[expect(
        clippy::print_stdout,
        reason = "the setup wizard is a conversation at a terminal, and it has no --output format"
    )]
    pub fn setup_site_manager() -> Result<()> {
        println!("Setting up UniFi Site Manager (Cloud) API credentials...\n");

        let answer = prompt_for_site_manager_key(&mut Stdio)?;
        let config = Self::edit(|config| config.set_site_manager(&answer))?;

        if config.has_site_manager_key() {
            println!("\nCloud commands are available: try 'ufa cloud hosts'");
        } else {
            println!("\nNo Site Manager API key configured; cloud commands stay unavailable.");
        }

        Ok(())
    }
}

/// What the connection test of the setup wizard found.
///
/// The test is one request for the application info of the controller, with
/// the key the user gave. Building the client opens no connection, so a client
/// that builds proves nothing. Only the answer to the request passes the test.
enum ConnectionTest {
    /// The controller gave its application info.
    Passed {
        /// The version of the network application on the controller.
        version: String,
    },
    /// No client could be built from the URL and the key, or the controller
    /// did not give its application info.
    Failed(anyhow::Error),
}

/// Whether the setup wizard saves the configuration the user gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaveDecision {
    /// Save the configuration.
    Save,
    /// Stop, and save nothing.
    DoNotSave,
}

/// Ask the controller at `controller_url` for its application info.
///
/// # Arguments
///
/// * `controller_url` - The URL of the controller.
/// * `api_key` - The key to send.
/// * `insecure` - Whether to accept a certificate that does not verify.
///
/// # Returns
///
/// What the test found.
async fn run_connection_test(
    controller_url: &str,
    api_key: &Secret,
    insecure: bool,
) -> ConnectionTest {
    let client = match UnifiClient::new(controller_url, api_key.expose(), insecure) {
        Ok(client) => client,
        Err(error) => return ConnectionTest::Failed(error),
    };

    match client.get::<crate::models::ApplicationInfo>("info").await {
        Ok(info) => ConnectionTest::Passed {
            version: info.application_version,
        },
        Err(error) => ConnectionTest::Failed(error),
    }
}

/// The controllers that the setup wizard offers from 1Password, after it
/// tells the user what the read found.
///
/// # Arguments
///
/// * `found` - The answer of [`discover_op_controllers`].
/// * `out` - Where the wizard writes.
///
/// # Returns
///
/// The controllers to offer. The list is empty when 1Password holds none, or
/// when the read failed.
///
/// # Errors
///
/// Returns an error if the result cannot be written.
fn controllers_to_offer(
    found: Result<Vec<OpController>>,
    out: &mut impl Write,
) -> Result<Vec<OpController>> {
    match found {
        Ok(controllers) if !controllers.is_empty() => Ok(controllers),
        Ok(_) => {
            writeln!(out, "No controllers found in 1Password (Private/ufa).\n")?;
            Ok(Vec::new())
        }
        Err(e) => {
            writeln!(out, "Could not query 1Password: {e:#}\n")?;
            Ok(Vec::new())
        }
    }
}

/// The line the wizard shows when the connection test passed.
const CONNECTION_TEST_PASSED: &str = "✅ Successfully connected to UniFi controller!";

/// The start of the line the wizard shows when the connection test failed.
/// The error follows it.
const CONNECTION_TEST_FAILED: &str = "❌ Connection test failed";

/// Show what the connection test found, and decide whether setup saves.
///
/// A test that passed saves without a question. Every failure shows its error
/// and asks the user, because only the user knows if the controller is off
/// today or if the URL or the key is wrong. A 403 also gets a hint: the
/// controller took the key and refused the request, which a key with limited
/// permissions can cause.
///
/// # Arguments
///
/// * `test` - What the connection test found.
/// * `out` - Where the result goes. The wizard gives standard output.
/// * `console` - Where the question goes.
///
/// # Returns
///
/// Whether to save the configuration.
///
/// # Errors
///
/// Returns an error if the result cannot be written, or if the question cannot
/// be answered.
fn decide_after_connection_test(
    test: ConnectionTest,
    out: &mut impl Write,
    console: &mut impl Console,
) -> Result<SaveDecision> {
    let error = match test {
        ConnectionTest::Passed { version } => {
            writeln!(out, "{CONNECTION_TEST_PASSED}")?;
            writeln!(out, "   Version: {version}")?;
            return Ok(SaveDecision::Save);
        }
        ConnectionTest::Failed(error) => error,
    };

    writeln!(out, "{CONNECTION_TEST_FAILED}: {error:#}")?;
    if answered_status(&error) == Some(StatusCode::FORBIDDEN) {
        writeln!(
            out,
            "   This can be normal if the API key has limited permissions."
        )?;
    }

    if prompt::confirm(console, "\nSave configuration anyway?")? {
        Ok(SaveDecision::Save)
    } else {
        Ok(SaveDecision::DoNotSave)
    }
}

/// The question that reads the Site Manager (cloud) credential.
const SITE_MANAGER_KEY_QUESTION: &str = "Site Manager API key or 1Password reference [skip]: ";

/// The question that reads the controller key the user pastes.
const API_KEY_QUESTION: &str = "\nPaste your API key here: ";

/// Prompt for the Site Manager (cloud) credential.
///
/// The answer is either an `op://` reference, the key itself, or empty to skip.
/// The answer does not show on the screen, because it can be the key.
///
/// # Arguments
///
/// * `console` - Where the question is put.
///
/// # Errors
///
/// Returns an error when the answers do not come from a terminal, and if the
/// answer cannot be read.
#[expect(
    clippy::print_stdout,
    reason = "the setup wizard is a conversation at a terminal, and it has no --output format"
)]
fn prompt_for_site_manager_key(console: &mut impl Console) -> Result<String> {
    println!(
        "Paste the key from the unifi.ui.com API section, or a 1Password reference \
         ({OP_REFERENCE_PREFIX}Private/ufa/site manager key) to keep it out of the config file."
    );
    prompt::ask_hidden_line(console, SITE_MANAGER_KEY_QUESTION)
}

/// The mode of a file that only its owner can read and write.
#[cfg(unix)]
const OWNER_ONLY: u32 = 0o600;

/// Replace the configuration file at `path` with the bytes that `write` puts
/// into the file it is given.
///
/// The new contents go into a new file in the same directory, at a mode no
/// other user can read, and a rename then puts that file at `path`. Thus no
/// other user can read the new keys at any point, a descriptor that is open on
/// the old file keeps the old file, and a failure before the rename leaves the
/// old file whole. The mode goes on the creation of the new file, so the
/// process umask cannot widen it. A `path` that is a symbolic link stays a link,
/// and the rename puts the new file at the end of its chain of links, also when
/// no file is there yet. Windows has no mode bits to ask for. The new file
/// inherits the ACL of the directory, which is per-user under `%APPDATA%`.
///
/// # Arguments
///
/// * `path` - Where the configuration file goes.
/// * `write` - Puts the new contents into the new file.
///
/// # Errors
///
/// Returns the error of a read of the chain of links, of the creation of the
/// new file, of `write`, of the sync, or of the rename. The old file is whole
/// after each of them.
fn replace_config_file(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let target = end_of_link_chain(path)?;
    let directory = target.parent().unwrap_or_else(|| Path::new("."));

    let mut builder = tempfile::Builder::new();
    builder.prefix(".config.toml.");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(OWNER_ONLY));
    }
    let mut temp = builder.tempfile_in(directory)?;

    write(temp.as_file_mut())?;
    temp.as_file().sync_all()?;
    temp.persist(&target).map_err(|error| error.error)?;
    Ok(())
}

/// The most symbolic links that a save follows from the config path. Linux
/// stops a path lookup after 40 links, and macOS stops after 32.
const MAX_LINK_HOPS: usize = 40;

/// The file that `path` names after each symbolic link in its last component
/// is followed. That file does not have to exist.
///
/// A rename does not follow a link in the last component of its target, thus
/// the save renames onto the end of the chain. `fs::canonicalize` cannot give
/// that end when the last link points at a file that does not exist yet. A
/// relative link is read from the directory of the link.
///
/// # Arguments
///
/// * `path` - The config path, which can be a symbolic link.
///
/// # Errors
///
/// Returns the error of a read of a link, or an error when the chain holds
/// more than [`MAX_LINK_HOPS`] links, for example a loop.
fn end_of_link_chain(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..=MAX_LINK_HOPS {
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let destination = fs::read_link(&current)?;
                current = match current.parent() {
                    Some(directory) => directory.join(destination),
                    None => destination,
                };
            }
            Ok(_) => return Ok(current),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(current),
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other(format!(
        "more than {MAX_LINK_HOPS} symbolic links from {}",
        path.display()
    )))
}

/// The store that reads the keys that 1Password holds.
///
/// [`CachedOnePassword`] is the store that every command uses. A test gives a
/// store of its own, so no test starts the real `op`.
pub trait KeyStore {
    /// Read the secret at a 1Password reference.
    ///
    /// # Arguments
    ///
    /// * `reference` - The 1Password reference to read.
    ///
    /// # Returns
    ///
    /// Whatever the store holds at that reference.
    ///
    /// # Errors
    ///
    /// Returns an error if the reference is not valid, and if the store
    /// cannot read it.
    fn read(&self, reference: &str) -> Result<String>;

    /// Forget the copy of the secret at a 1Password reference that the store
    /// keeps. The next read of that reference then asks 1Password.
    ///
    /// # Arguments
    ///
    /// * `reference` - The 1Password reference to forget.
    ///
    /// # Errors
    ///
    /// Returns an error if the reference is not valid, and if the store
    /// cannot forget its copy.
    fn forget(&self, reference: &str) -> Result<()>;
}

/// The keys that 1Password holds, read through op-cache.
///
/// Every read of a 1Password reference in `ufa` starts here. op-cache keeps a
/// copy of each key it reads, and answers the next read from that copy. The
/// copy stays until something forgets it: [`read_again_from_1password`]
/// forgets it when the API refuses it, and when the setup wizard tests it.
pub struct CachedOnePassword {
    /// The cache that holds a copy of each key it read.
    cache: op_cache::OpCache,
}

impl CachedOnePassword {
    /// The store that every command uses.
    ///
    /// The cache file sits in the configuration directory, beside the
    /// configuration file. [`op_cache::OpCache::new`] puts it at the root of
    /// the git repository that holds the working directory. `ufa` is an
    /// installed tool that runs from any directory, so that choice fails
    /// outside a git repository. Inside one, it writes the plaintext
    /// controller key into a repository that does not ignore the file, and a
    /// `git add -A` there stages the key.
    ///
    /// # Returns
    ///
    /// The store, with its cache at `.op-cache.json` in the configuration
    /// directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the operating system names no configuration
    /// directory.
    pub fn beside_the_configuration() -> Result<Self> {
        Ok(Self::in_directory(&Config::config_dir()?))
    }

    /// [`CachedOnePassword::beside_the_configuration`] against an explicit
    /// configuration directory.
    ///
    /// # Arguments
    ///
    /// * `directory` - The configuration directory.
    ///
    /// # Returns
    ///
    /// The store, with its cache at `.op-cache.json` in `directory`.
    pub fn in_directory(directory: &Path) -> Self {
        Self {
            cache: op_cache_in(directory),
        }
    }
}

impl KeyStore for CachedOnePassword {
    fn read(&self, reference: &str) -> Result<String> {
        let path = op_path(reference)?;

        self.cache
            .read(&path, None)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn forget(&self, reference: &str) -> Result<()> {
        let path = op_path(reference)?;

        self.cache
            .invalidate(&path)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// The op-cache path of a 1Password reference.
///
/// # Arguments
///
/// * `reference` - The 1Password reference.
///
/// # Returns
///
/// The validated path.
///
/// # Errors
///
/// Returns an error that names the reference if it does not start with
/// `op://`, or if it holds a character op-cache refuses.
fn op_path(reference: &str) -> Result<op_cache::OpPath> {
    op_cache::OpPath::new(reference).map_err(|e| anyhow::anyhow!("{e}"))
}

/// The op-cache at `.op-cache.json` in a configuration directory.
///
/// The directory is a parameter so a test can name its own. No git process
/// starts. The directory does not have to exist yet: op-cache makes it on the
/// first write, as [`Config::edit`] makes it for the configuration file, and
/// it writes the cache file at mode 0600.
///
/// # Arguments
///
/// * `directory` - The configuration directory.
///
/// # Returns
///
/// The cache at `.op-cache.json` in `directory`.
pub(crate) fn op_cache_in(directory: &Path) -> op_cache::OpCache {
    op_cache::OpCache::with_path(directory.join(OP_CACHE_FILE))
}

/// A key, and the 1Password reference it came from when it came from one.
///
/// The reference is what lets a command read the key again after the API
/// refuses it. A key from the command line, from the environment, or from a
/// plaintext field of the configuration file has no reference, and nothing
/// can read such a key again.
#[derive(Debug, Clone)]
pub struct ResolvedKey {
    /// The key.
    key: Secret,
    /// The 1Password reference that holds the key, when one does.
    reference: Option<String>,
}

impl ResolvedKey {
    /// A key that did not come from 1Password.
    ///
    /// # Arguments
    ///
    /// * `key` - The key.
    ///
    /// # Returns
    ///
    /// The key, with no reference.
    pub fn without_reference(key: Secret) -> Self {
        Self {
            key,
            reference: None,
        }
    }

    /// A key that 1Password holds at `reference`.
    ///
    /// # Arguments
    ///
    /// * `reference` - The 1Password reference that holds the key.
    /// * `key` - The key.
    ///
    /// # Returns
    ///
    /// The key, with its reference.
    fn from_1password(reference: &str, key: Secret) -> Self {
        Self {
            key,
            reference: Some(reference.to_string()),
        }
    }

    /// The key.
    ///
    /// # Returns
    ///
    /// The key, still wrapped.
    pub fn key(&self) -> &Secret {
        &self.key
    }

    /// The 1Password reference that holds the key.
    ///
    /// # Returns
    ///
    /// The reference, or `None` when the key did not come from 1Password.
    pub fn reference(&self) -> Option<&str> {
        self.reference.as_deref()
    }
}

/// Resolve a secret that may live in 1Password.
///
/// This is the one place that decides which source wins. A configured
/// `op_path` always wins: if it is present but cannot be read, the error is
/// reported rather than quietly downgrading to the plaintext copy, which would
/// hide a broken 1Password reference. `plaintext` is the fallback in the
/// configuration file, and `missing` is the message used when neither source
/// is configured.
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
/// * `plaintext` - The value in the configuration file, when there is one.
/// * `missing` - Said when neither source is configured.
///
/// # Returns
///
/// The secret, exactly as its source holds it, and the reference it came from
/// when it came from one.
///
/// # Errors
///
/// Returns an error if neither source is configured, and if the reference
/// cannot be read or holds only blank space.
fn resolve_secret_from(
    read_reference: impl FnOnce(&str) -> Result<String>,
    op_path: Option<&str>,
    plaintext: Option<&str>,
    missing: &str,
) -> Result<ResolvedKey> {
    if let Some(op_path) = op_path.filter(|reference| !is_blank(reference)) {
        let key = key_in_answer(op_path, read_reference(op_path)?)?;

        return Ok(ResolvedKey::from_1password(op_path, key));
    }
    if let Some(secret) = plaintext.filter(|value| !is_blank(value)) {
        return Ok(ResolvedKey::without_reference(Secret::from(secret)));
    }
    anyhow::bail!("{missing}")
}

/// The key in what 1Password answered for a reference.
///
/// # Arguments
///
/// * `reference` - The 1Password reference that was read.
/// * `answer` - What 1Password answered.
///
/// # Returns
///
/// The key, exactly as 1Password holds it.
///
/// # Errors
///
/// Returns an error that names the reference when the answer holds only blank
/// space.
fn key_in_answer(reference: &str, answer: String) -> Result<Secret> {
    if is_blank(&answer) {
        anyhow::bail!(
            "The 1Password reference {reference} holds no key. Put the key in that field, \
             or run 'ufa config setup' to name another reference."
        );
    }

    Ok(Secret::from(answer))
}

/// Read the key that 1Password holds now at a reference.
///
/// The store forgets its copy of the key first, so 1Password answers and not
/// the cache. A copy from before a rotation in 1Password is exactly what this
/// removes. The store then keeps a copy of the new key.
///
/// This is the one place that forgets and then reads. The setup wizard reads
/// through it, so it tests the key that 1Password holds now. A command reads
/// through it after the API refuses the key it sent.
///
/// # Arguments
///
/// * `keys` - The store that holds the key.
/// * `reference` - The 1Password reference of the key.
///
/// # Returns
///
/// The key that 1Password holds now.
///
/// # Errors
///
/// Returns an error if the store cannot forget its copy, if it cannot read
/// the reference, and if the reference holds only blank space.
pub fn read_again_from_1password(keys: &impl KeyStore, reference: &str) -> Result<Secret> {
    keys.forget(reference)
        .with_context(|| format!("Failed to forget the cached copy of {reference}"))?;

    key_in_answer(reference, keys.read(reference)?)
}

/// Run network discovery (mDNS + common IPs) and let the user pick.
#[expect(
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
            c.url()?,
            if c.is_verified { "✓" } else { "" }
        );
    }
    let manual_idx = controllers.len() + 1;
    println!("  {manual_idx}. Enter URL manually\n");

    match prompt::select_one(&mut Stdio, "Select a controller", manual_idx)? {
        Some(index) if index < controllers.len() => controllers[index].url(),
        Some(_) => get_manual_controller_url().await,
        None => anyhow::bail!(NEEDS_A_TERMINAL),
    }
}

#[expect(
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

        let validated = validate_user_url(&url).await;
        if let Some(url_to_save) =
            url_after_validation(validated, url, &mut std::io::stdout(), &mut Stdio)?
        {
            return Ok(url_to_save);
        }
    }
}

/// The question that the wizard asks when a typed URL failed validation.
const USE_URL_ANYWAY: &str = "Use this URL anyway?";

/// The URL that the setup wizard keeps after it validated a typed URL, after
/// it tells the user what the validation found.
///
/// # Arguments
///
/// * `validated` - The answer of [`validate_user_url`] for `typed`.
/// * `typed` - The URL the user typed, with its scheme.
/// * `out` - Where the wizard writes. The wizard gives standard output.
/// * `console` - Where the question goes.
///
/// # Returns
///
/// The URL to save, or `None` when the user asks to type a new URL.
///
/// # Errors
///
/// Returns an error if the result cannot be written, or if the question
/// cannot be answered.
fn url_after_validation(
    validated: Result<String>,
    typed: String,
    out: &mut impl Write,
    console: &mut impl Console,
) -> Result<Option<String>> {
    match validated {
        Ok(url_to_save) => {
            writeln!(out, " ✓")?;
            Ok(Some(url_to_save))
        }
        Err(e) => {
            writeln!(out, " ✗")?;
            writeln!(out, "Failed to validate controller: {e:#}")?;
            if prompt::confirm(console, USE_URL_ANYWAY)? {
                Ok(Some(typed))
            } else {
                Ok(None)
            }
        }
    }
}

/// Prompt for an API key when no 1Password entry exists.
#[expect(
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

    read_pasted_api_key(&mut Stdio)
}

/// Read the controller key the user pastes, and do not show it on the screen.
///
/// [`prompt_for_api_key`] opens a browser before it reads, so the read is a
/// function of its own, and a test can drive it.
///
/// # Arguments
///
/// * `console` - Where the question is put.
///
/// # Errors
///
/// Returns an error when the answers do not come from a terminal, if the
/// answer cannot be read, and when the answer is empty.
fn read_pasted_api_key(console: &mut impl Console) -> Result<Secret> {
    let api_key = prompt::ask_hidden_line(console, API_KEY_QUESTION)?;

    if api_key.is_empty() {
        anyhow::bail!("API key cannot be empty");
    }

    Ok(Secret::from(api_key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_vault::{TempConfigDir, TestVault};

    /// The store for a test whose key does not come from 1Password.
    ///
    /// The vault cannot be read and no cache file exists, so a test that
    /// reaches the store by mistake gets an error, and no call to `op`.
    fn no_1password() -> TestVault {
        TestVault::new("no-1password", &[], None)
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
            .resolve_api_key(&no_1password())
            .expect("the key pasted during setup must still resolve after saving");
        assert_eq!(resolved.key().expose(), "pasted-api-key");
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
                .resolve_api_key(&no_1password())
                .expect("the controller key must still resolve")
                .key()
                .expose(),
            "key-from-an-older-ufa"
        );
        assert_eq!(
            loaded
                .resolve_site_manager_api_key(&no_1password())
                .expect("the cloud key must still resolve")
                .key()
                .expose(),
            "cloud-key-from-an-older-ufa"
        );
    }

    /// The config file can hold an API key in plaintext (the pasted controller
    /// key, and the pasted `site_manager_api_key` field), so it must never be
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
    /// The file holds a pasted controller key and a pasted plaintext Site
    /// Manager key, and it is created before either is written. A create at the
    /// process umask lands on 0644 on a normal machine, so every save hands
    /// every other user of that machine a window in which the file is
    /// readable — and a run interrupted inside the window leaves it that way
    /// for good.
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

        let mut mode_during_write = 0;
        crate::test_support::with_umask(0o000, || {
            replace_config_file(&path, |file| {
                mode_during_write = file.metadata()?.permissions().mode() & 0o777;
                Ok(())
            })
            .expect("replacing the config file must succeed");
        });
        let final_mode = fs::metadata(&path)
            .expect("the created config file must exist")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(
            mode_during_write, 0o600,
            "the config file holds cleartext keys, so no other user may read it \
             at any point in its life, got {mode_during_write:o}"
        );
        assert_eq!(
            final_mode, 0o600,
            "the config file must end at an owner-only mode, got {final_mode:o}"
        );
    }

    /// A configuration file written by a version of `ufa` that created it at the
    /// process umask is already on disk at 0644. The save puts a new owner-only
    /// file at its path, and
    /// `the_keys_go_into_a_new_owner_only_file_when_a_wider_one_exists` shows
    /// that the keys never go into the old file.
    #[cfg(unix)]
    #[test]
    fn saving_over_a_file_that_was_already_wider_leaves_an_owner_only_file() {
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
            "a file that was already readable by others must be replaced by an \
             owner-only file, got {mode:o}"
        );
    }

    /// The mode of the old file does not matter, because the keys go into a new
    /// file. The test reads the inode and the mode of the file that the write
    /// step is given, which is the moment before the keys go in.
    #[cfg(unix)]
    #[test]
    fn the_keys_go_into_a_new_owner_only_file_when_a_wider_one_exists() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let temp = TempConfigDir::new("new-file-for-keys");
        create_directory(&temp.dir);
        let path = temp.config_file();
        write_file(&path, "url = \"https://192.168.1.1\"\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("the existing file must be settable to a wider mode");
        let old_inode = fs::metadata(&path)
            .expect("the existing file must have metadata")
            .ino();

        let mut inode_during_write = old_inode;
        let mut mode_during_write = 0;
        replace_config_file(&path, |file| {
            let metadata = file.metadata()?;
            inode_during_write = metadata.ino();
            mode_during_write = metadata.permissions().mode() & 0o777;
            Ok(())
        })
        .expect("replacing the config file must succeed");

        assert_ne!(
            inode_during_write, old_inode,
            "the keys must go into a new file, not into the old one that another \
             user can hold open"
        );
        assert_eq!(
            mode_during_write, 0o600,
            "the keys are written next, and a file that is readable by others \
             at this moment leaks them, got {mode_during_write:o}"
        );
    }

    /// A save that fails after the file is open must not cost the user the old
    /// file. The URL, the 1Password references and the keys of the old file
    /// stay on disk, and the failed save leaves no other file in the directory.
    #[test]
    fn a_save_that_fails_after_the_open_leaves_the_old_config_file_whole() {
        let temp = TempConfigDir::new("failed-save");
        create_directory(&temp.dir);
        let path = temp.config_file();
        let old_contents = "url = \"https://192.168.1.1\"\n";
        write_file(&path, old_contents);

        let result = replace_config_file(&path, |_| {
            Err(std::io::Error::other("the write step failed"))
        });

        assert!(result.is_err(), "a failed write step must be an error");
        assert_eq!(
            fs::read_to_string(&path).expect("the old config file must still exist"),
            old_contents,
            "the URL, the 1Password references and the keys of the old file \
             must survive a save that failed"
        );
        let mut names: Vec<_> = fs::read_dir(&temp.dir)
            .expect("the config directory must be readable")
            .map(|entry| {
                entry
                    .expect("a directory entry must be readable")
                    .file_name()
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from("config.toml")],
            "a failed save must leave no other file behind"
        );
    }

    /// The kernel checks the mode of a file only at the open. Narrowing the old
    /// inode does not take back a descriptor that is already open, so a reader
    /// that opened the old 0644 file before the save must not see the keys of
    /// the save.
    #[cfg(unix)]
    #[test]
    fn a_reader_of_the_old_config_file_does_not_see_the_new_keys() {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;

        let temp = TempConfigDir::new("old-reader");
        create_directory(&temp.dir);
        let path = temp.config_file();
        write_file(&path, "url = \"https://192.168.1.1\"\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("the existing file must be settable to a wider mode");
        // Stands for another local user who opened the old file before the save.
        let mut old_reader = fs::File::open(&path).expect("the old file must open");

        Config {
            api_key: Some("new-plaintext-controller-key".into()),
            ..Config::default()
        }
        .save_to(&path)
        .expect("saving over the existing file must succeed");

        let mut seen = String::new();
        old_reader
            .read_to_string(&mut seen)
            .expect("the old descriptor must stay readable");
        assert!(
            !seen.contains("new-plaintext-controller-key"),
            "a descriptor opened on the old file before the save must not reach \
             the new key"
        );
        assert!(
            fs::read_to_string(&path)
                .expect("the saved config file must exist")
                .contains("new-plaintext-controller-key"),
            "the config file at the path must hold the new key"
        );
    }

    /// A config file that is a symbolic link stays a link. The rename replaces
    /// the file that the link points at, and not the link.
    #[cfg(unix)]
    #[test]
    fn saving_through_a_symlinked_config_file_keeps_the_link() {
        let temp = TempConfigDir::new("symlinked-config");
        create_directory(&temp.dir);
        let real = temp.dir.join("real-config.toml");
        write_file(&real, "url = \"https://192.168.1.1\"\n");
        let link = temp.config_file();
        std::os::unix::fs::symlink(&real, &link).expect("the link must be creatable");

        Config {
            api_key: Some("plaintext-controller-key".into()),
            ..Config::default()
        }
        .save_to(&link)
        .expect("saving through the link must succeed");

        assert!(
            fs::symlink_metadata(&link)
                .expect("the link must exist")
                .file_type()
                .is_symlink(),
            "the save must keep the config file a symbolic link"
        );
        assert!(
            fs::read_to_string(&real)
                .expect("the real file must exist")
                .contains("plaintext-controller-key"),
            "the real file that the link points at must hold the key"
        );
    }

    /// A config file can be a symbolic link to a file that does not exist
    /// yet, for example a link that a dotfiles manager made. The save makes
    /// that file and keeps the link. The link is relative, so the save must
    /// read it from the directory of the link.
    #[cfg(unix)]
    #[test]
    fn saving_through_a_dangling_symlinked_config_file_keeps_the_link() {
        let temp = TempConfigDir::new("dangling-config");
        create_directory(&temp.dir);
        let real = temp.dir.join("real-config.toml");
        let link = temp.config_file();
        std::os::unix::fs::symlink("real-config.toml", &link).expect("the link must be creatable");

        Config {
            api_key: Some("plaintext-controller-key".into()),
            ..Config::default()
        }
        .save_to(&link)
        .expect("saving through the dangling link must succeed");

        assert!(
            fs::symlink_metadata(&link)
                .expect("the link must exist")
                .file_type()
                .is_symlink(),
            "the save must keep the config file a symbolic link"
        );
        assert!(
            fs::read_to_string(&real)
                .expect("the save must make the file that the link points at")
                .contains("plaintext-controller-key"),
            "the file that the link points at must hold the key"
        );
    }

    /// The save reads each relative link of a chain from the directory of that
    /// link, as the kernel does. In the chain `config.toml -> sub/next.toml`
    /// and `sub/next.toml -> real.toml`, the second link names
    /// `sub/real.toml`, not the `real.toml` beside the config path.
    #[cfg(unix)]
    #[test]
    fn saving_through_a_chain_of_links_across_directories_reads_each_link_from_its_directory() {
        let temp = TempConfigDir::new("chained-config");
        let sub = temp.dir.join("sub");
        create_directory(&sub);
        let link = temp.config_file();
        let next = sub.join("next.toml");
        std::os::unix::fs::symlink("sub/next.toml", &link).expect("the link must be creatable");
        std::os::unix::fs::symlink("real.toml", &next).expect("the link must be creatable");

        Config {
            api_key: Some("plaintext-controller-key".into()),
            ..Config::default()
        }
        .save_to(&link)
        .expect("saving through the chain of links must succeed");

        for path in [&link, &next] {
            assert!(
                fs::symlink_metadata(path)
                    .expect("the link must exist")
                    .file_type()
                    .is_symlink(),
                "the save must keep {} a symbolic link",
                path.display()
            );
        }
        assert!(
            fs::read_to_string(sub.join("real.toml"))
                .expect("the save must make the file at the end of the chain")
                .contains("plaintext-controller-key"),
            "the file at the end of the chain must hold the key"
        );
        assert!(
            fs::symlink_metadata(temp.dir.join("real.toml")).is_err(),
            "the save must not make a real.toml beside the config path"
        );
    }

    /// A config path in a loop of symbolic links names no file. The save is
    /// an error, and it leaves both links as they were.
    #[cfg(unix)]
    #[test]
    fn saving_through_a_loop_of_symlinks_is_an_error() {
        let temp = TempConfigDir::new("looped-config");
        create_directory(&temp.dir);
        let link = temp.config_file();
        let other = temp.dir.join("other.toml");
        std::os::unix::fs::symlink("other.toml", &link).expect("the link must be creatable");
        std::os::unix::fs::symlink("config.toml", &other).expect("the link must be creatable");

        let result = replace_config_file(&link, |file| file.write_all(b"insecure = true\n"));

        assert!(result.is_err(), "a loop of links must be an error");
        for path in [&link, &other] {
            assert!(
                fs::symlink_metadata(path)
                    .expect("the link must exist")
                    .file_type()
                    .is_symlink(),
                "the failed save must leave {} a symbolic link",
                path.display()
            );
        }
    }

    /// A 1Password reference names the item that holds the key. It is not the
    /// key, and a dump that hides it says nothing a reader can act on.
    const OP_REFERENCE: &str = "op://Private/ufa/key - 192.168.1.1 port 443";

    /// Nothing formats a `Config` with `{:?}` today, so nothing leaks today.
    /// The type is one `eprintln!` added while debugging away from printing a
    /// pasted controller key and a pasted plaintext Site Manager key in full,
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

    /// A configured 1Password reference wins over the plaintext field.
    /// Falling back on failure would silently keep using a stale plaintext key
    /// after the user moved the credential into 1Password.
    #[test]
    fn site_manager_key_prefers_1password_over_the_plaintext_field() {
        // Deliberately not an `op://` reference: op-cache rejects it locally,
        // so the test never shells out to the `op` CLI. The store is the one
        // every command uses, over a directory of this test's own.
        let config = Config {
            sm_op_path: Some("not-an-op-path".to_string()),
            site_manager_api_key: Some("stale-plaintext-key".into()),
            ..Config::default()
        };
        let temp = TempConfigDir::new("unreadable-cloud-reference");

        let error = config
            .resolve_site_manager_api_key(&CachedOnePassword::in_directory(&temp.dir))
            .expect_err(
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

    /// A config file that a person wrote, with comments as in the sample of
    /// the README, and with a key that this version of `ufa` does not know.
    const HAND_WRITTEN_CONFIG: &str = "\
# Controller
url = \"https://192.168.1.1\" # the gateway in the rack
insecure = true

# Controller API key, in 1Password (recommended)
op_path = \"op://Private/ufa/key - 192.168.1.1 port 443\"

# A key that a later version of ufa reads
later_setting = \"kept\"
";

    /// A save writes the fields that the command asked about. The comments of
    /// the person who wrote the file, and a key that `ufa` does not know, are
    /// not fields that the command asked about, so they stay.
    #[test]
    fn a_save_keeps_each_comment_and_each_key_that_ufa_does_not_know() {
        let temp = TempConfigDir::new("keeps-comments");
        create_directory(&temp.dir);
        write_file(&temp.config_file(), HAND_WRITTEN_CONFIG);

        Config::edit_at(&temp.config_file(), |config| {
            config.set_site_manager("op://Private/ufa/site manager key");
        })
        .expect("configuring the cloud key must succeed");

        let saved = fs::read_to_string(temp.config_file()).expect("the saved file must exist");
        for line in [
            "# Controller",
            "url = \"https://192.168.1.1\" # the gateway in the rack",
            "# Controller API key, in 1Password (recommended)",
            "# A key that a later version of ufa reads",
            "later_setting = \"kept\"",
            "sm_op_path = \"op://Private/ufa/site manager key\"",
        ] {
            assert!(
                saved.lines().any(|saved_line| saved_line == line),
                "the save must keep the line {line:?}, got:\n{saved}"
            );
        }
    }

    /// A field that the edit clears leaves the file, and the field that takes
    /// its place goes in.
    #[test]
    fn a_save_removes_a_field_that_the_edit_cleared() {
        let temp = TempConfigDir::new("clears-a-field");
        create_directory(&temp.dir);
        write_file(&temp.config_file(), HAND_WRITTEN_CONFIG);

        Config::edit_at(&temp.config_file(), |config| {
            config.set_controller(
                "https://192.168.1.1".to_string(),
                &ControllerCredential::Pasted {
                    key: Secret::from("pasted-controller-key"),
                },
                true,
            );
        })
        .expect("configuring a pasted key must succeed");

        let loaded = Config::load_from(&temp.config_file())
            .expect("loading the config must succeed")
            .expect("the config file must still exist");
        assert_eq!(
            loaded.op_path, None,
            "the cleared reference must leave the file"
        );
        assert_eq!(
            loaded.api_key.as_ref().map(Secret::expose),
            Some("pasted-controller-key"),
            "the pasted key must go in"
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

    /// dotenvy puts the whole text of a line it cannot parse into its error,
    /// and that text holds the value. `main` prints the error, so a key with a
    /// space in it, or a key with no closing quote, must not reach the report.
    #[test]
    fn a_settings_line_that_cannot_be_parsed_does_not_show_its_value() {
        const LEAKED_KEY: &str = "sekrit-api-key";
        let lines = [
            format!("UNIFI_API_KEY={LEAKED_KEY} with space\n"),
            format!("UNIFI_API_KEY=\"{LEAKED_KEY}\n"),
        ];

        for line in lines {
            let temp = TempConfigDir::new("env-unparsable");
            create_directory(&temp.dir);
            write_file(&temp.dir.join(ENVIRONMENT_FILE), &line);

            let (loaded, _) = load_and_take_back("UNIFI_API_KEY", &temp.dir);

            let error = loaded.expect_err("a settings line that cannot be parsed is a fault");
            for report in [format!("{error:#}"), format!("{error:?}")] {
                assert!(
                    report.contains(ENVIRONMENT_FILE),
                    "the failure must name the file it could not parse, got {report}"
                );
                assert!(
                    !report.contains(LEAKED_KEY),
                    "the failure must not show the value of the line {line:?}, got {report}"
                );
            }
        }
    }

    /// A user must find the bad line in the file, and the report holds no text
    /// of it, because the text can hold a key. So the report names the line
    /// where the setting starts. The position that dotenvy gives is no help:
    /// its unit changes with the kind of error. Comments, blank lines, and a
    /// value over two lines come before the bad line in some cases, and a
    /// setting that spans lines is named by its first line.
    #[test]
    fn a_settings_line_that_cannot_be_parsed_is_named_by_its_line_number() {
        const LEAKED_KEY: &str = "sekrit-api-key";
        const EARLIER_SETTING: &str = "UFA_TEST_SETTING_BEFORE_THE_BAD_LINE";
        let cases = [
            (
                format!(
                    "# ufa settings\n\n{EARLIER_SETTING}=\"first\nsecond\"\n\
                     UNIFI_API_KEY={LEAKED_KEY} with space\nUNIFI_INSECURE=true\n"
                ),
                5,
            ),
            (
                format!(
                    "{EARLIER_SETTING}=one\nUNIFI_API_KEY=\"{LEAKED_KEY}\nUNIFI_INSECURE=true\n"
                ),
                2,
            ),
            (
                format!("{EARLIER_SETTING}=one\n\n1UNIFI_API_KEY={LEAKED_KEY}\n"),
                3,
            ),
            (
                format!("{EARLIER_SETTING}=one\nUNIFI_API_KEY=\"{LEAKED_KEY}\nrest\" junk\n"),
                2,
            ),
        ];

        for (text, expected) in cases {
            let temp = TempConfigDir::new("env-unparsable-line-number");
            create_directory(&temp.dir);
            write_file(&temp.dir.join(ENVIRONMENT_FILE), &text);

            let (loaded, _) = load_and_take_back(EARLIER_SETTING, &temp.dir);

            let error = loaded.expect_err("a settings line that cannot be parsed is a fault");
            for report in [format!("{error:#}"), format!("{error:?}")] {
                assert!(
                    report.contains(&format!(" line {expected} ")),
                    "the failure must name line {expected} of {text:?}, got {report}"
                );
                assert!(
                    !report.contains(LEAKED_KEY),
                    "the failure must not show the value of {text:?}, got {report}"
                );
            }
        }
    }

    /// Many settings files have no line break after the last line. The reader
    /// finds no line break there, and it must still hand out that last line,
    /// or dotenvy sees the end of the file and drops the last setting with no
    /// error.
    #[test]
    fn a_last_setting_with_no_final_line_break_is_loaded() {
        let temp = TempConfigDir::new("env-no-final-line-break");
        create_directory(&temp.dir);
        write_file(
            &temp.dir.join(ENVIRONMENT_FILE),
            "# ufa settings\nUNIFI_URL=https://last.example",
        );

        let (loaded, url) = load_and_take_back("UNIFI_URL", &temp.dir);

        assert_eq!(
            loaded.expect("a settings file with no final line break must load"),
            Some(temp.dir.join(ENVIRONMENT_FILE)),
            "the load must report the file it read"
        );
        assert_eq!(
            url.as_deref(),
            Some("https://last.example"),
            "the last setting must reach the process environment when no line break ends it"
        );
    }

    /// dotenvy reads through a `BufReader` of 8 KiB, so a line longer than
    /// that buffer arrives in more than one read. The line must count once,
    /// or each line after it gets a line number that is too high.
    #[test]
    fn a_bad_line_after_a_value_longer_than_the_read_buffer_is_named_by_its_line_number() {
        /// The capacity of the `BufReader` that dotenvy reads through.
        const READ_BUFFER: usize = 8 * 1024;
        const LEAKED_KEY: &str = "sekrit-api-key";
        const LONG_SETTING: &str = "UFA_TEST_SETTING_LONGER_THAN_THE_READ_BUFFER";
        let long_value = "v".repeat(READ_BUFFER.saturating_mul(3));
        let text = format!("{LONG_SETTING}={long_value}\nUNIFI_API_KEY={LEAKED_KEY} with space\n");
        let temp = TempConfigDir::new("env-long-value-line-number");
        create_directory(&temp.dir);
        write_file(&temp.dir.join(ENVIRONMENT_FILE), &text);

        let (loaded, _) = load_and_take_back(LONG_SETTING, &temp.dir);

        let error = loaded.expect_err("a settings line that cannot be parsed is a fault");
        let report = format!("{error:#}");
        assert!(
            report.contains(" line 2 "),
            "the failure must name line 2, after a value of {} bytes on line 1, got {report}",
            long_value.len()
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
                .resolve_api_key(&no_1password())
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
            .resolve_site_manager_api_key(&no_1password())
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
            .resolve_api_key(&no_1password())
            .expect_err("a blank reference must not resolve as a key");
        assert!(
            format!("{error:#}").contains("ufa config setup"),
            "a blank reference must earn the advice a missing one earns, got {error:#}"
        );
    }

    /// A blank answer from 1Password is a different fault from a blank field.
    /// The user named a specific item, so the reference is broken rather than
    /// absent — and `resolve_secret_from` already refuses to downgrade a broken
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
                .resolve_api_key(&no_1password())
                .expect("a key that is not blank must resolve")
                .key()
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
            .key()
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
        assert_eq!(
            controllers[0].url().expect("an IPv4 address fits in a URL"),
            "https://192.168.1.1:443"
        );
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

    /// A field label that names an IPv6 address gives a URL with the
    /// brackets the address needs.
    #[test]
    fn a_field_label_with_an_ipv6_address_gives_a_url_that_parses() {
        let controllers = controllers_from_labels(&["key - 2001:db8::1 port 443".to_string()]);

        let url = controllers[0].url().expect("an IPv6 address fits in a URL");

        assert_eq!(url, "https://[2001:db8::1]:443");
        assert!(
            url::Url::parse(&url).is_ok(),
            "the URL of the controller must parse, got {url}"
        );
    }

    /// A field label whose host fits in no URL still names a controller,
    /// but its URL is an error that names the field.
    #[test]
    fn a_field_label_whose_host_fits_in_no_url_has_no_url() {
        let controllers = controllers_from_labels(&["key - port forward port 8443".to_string()]);

        let error = controllers[0]
            .url()
            .expect_err("a host with a space in it fits in no URL");

        assert!(
            error.to_string().contains("key - port forward port 8443"),
            "the error must name the field to correct, got: {error}"
        );
    }

    /// The wizard shows the cause of a failed read of 1Password, not only the
    /// outer context that `discover_op_controllers` adds.
    #[test]
    fn a_failed_read_of_1password_shows_its_cause() {
        const CAUSE: &str = "op: command not found";
        const CONTEXT: &str = "Failed to list the fields of Private/ufa";
        let found: Result<Vec<OpController>> = Err(anyhow::anyhow!(CAUSE)).context(CONTEXT);
        let mut out = Vec::new();

        let offered = controllers_to_offer(found, &mut out).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(
            offered.is_empty(),
            "no controller to offer, got {offered:?}"
        );
        assert!(
            text.contains(CONTEXT),
            "the context is missing from {text:?}"
        );
        assert!(text.contains(CAUSE), "the cause is missing from {text:?}");
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
                .resolve_site_manager_api_key(&no_1password())
                .expect("a legacy plaintext key must still resolve")
                .key()
                .expose(),
            "legacy-plaintext-key"
        );
    }

    /// `ufa` is an installed tool, and it runs from any directory. A cache
    /// file at the root of the git repository around the working directory
    /// fails outside a repository, and inside one it puts a plaintext
    /// controller key into a repository that does not ignore the file. The
    /// cache file sits beside the configuration file instead.
    ///
    /// The directory does not exist yet, as on the first `ufa config setup`.
    #[test]
    fn the_op_cache_file_sits_beside_the_configuration_file() {
        let temp = TempConfigDir::new("op-cache-path");

        let cache = op_cache_in(&temp.dir);

        assert_eq!(
            cache.cache_path(),
            temp.dir.join(OP_CACHE_FILE),
            "the op-cache file must sit in the configuration directory"
        );
    }

    /// A key that op-cache read before is read again from the cache file
    /// beside the configuration file, with no git repository around the
    /// working directory and no call to 1Password.
    ///
    /// Two assertions come before the read, and each one stops the test
    /// before the read can reach the real `op`: the first proves the cache
    /// reads the seeded file, the second proves op-cache parses that file
    /// and finds the reference in it.
    #[test]
    fn a_key_cached_beside_the_configuration_file_is_read_without_1password() {
        use crate::test_support::FAKE_CONTROLLER_KEY;

        let temp = TempConfigDir::new("op-cache-read");
        create_directory(&temp.dir);
        let seeded = serde_json::json!({
            OP_REFERENCE: {
                "value": FAKE_CONTROLLER_KEY,
                "fetchedAt": "2026-01-01T00:00:00Z",
            }
        });
        write_file(&temp.dir.join(OP_CACHE_FILE), &seeded.to_string());

        let cache = op_cache_in(&temp.dir);

        assert_eq!(
            cache.cache_path(),
            temp.dir.join(OP_CACHE_FILE),
            "the op-cache must read the file beside the configuration file"
        );
        let entries = cache.entries().expect("the seeded cache file must list");
        assert!(
            entries
                .iter()
                .any(|(reference, _)| reference == OP_REFERENCE),
            "op-cache must find the seeded reference, or the read below calls `op`, \
             got {entries:?}"
        );

        let reference = op_cache::OpPath::new(OP_REFERENCE).expect("the reference is valid");
        let key = cache
            .read(&reference, None)
            .expect("a cached key must read without 1Password");

        assert_eq!(key, FAKE_CONTROLLER_KEY);
    }

    /// The key that 1Password held before a rotation. The cache file keeps it.
    const KEY_BEFORE_THE_ROTATION: &str = "key-before-the-rotation-7Hc2Qx";

    /// The key that 1Password holds after the rotation.
    const KEY_AFTER_THE_ROTATION: &str = "key-after-the-rotation-3Lp9Vz";

    /// The setup wizard tests the key that it reads. A user who rotated the key
    /// in 1Password runs the wizard to use the new key, and a copy that
    /// op-cache kept from before the rotation fails the test of the wizard. So
    /// the read forgets the copy first, and 1Password answers.
    #[test]
    fn reading_a_key_again_forgets_the_cached_copy_first() {
        let vault = TestVault::new(
            "read-again",
            &[(OP_REFERENCE, KEY_BEFORE_THE_ROTATION)],
            Some(KEY_AFTER_THE_ROTATION),
        );

        let key = read_again_from_1password(&vault, OP_REFERENCE)
            .expect("the vault holds a key at the reference");

        assert_eq!(
            key.expose(),
            KEY_AFTER_THE_ROTATION,
            "the key must be the one that 1Password holds now"
        );
        assert_eq!(
            vault.forgets(),
            1,
            "the cached copy must be forgotten before the read"
        );
        assert!(
            vault.caches(OP_REFERENCE),
            "the cache file must hold the copy that 1Password gave"
        );
        assert!(
            vault.cache_file().contains(KEY_AFTER_THE_ROTATION),
            "the cache file must hold the key after the rotation"
        );
        assert!(
            !vault.cache_file().contains(KEY_BEFORE_THE_ROTATION),
            "the cache file must not hold the key from before the rotation"
        );
    }

    /// A blank answer is a broken reference on a second read too, and the
    /// error names the reference. A command that reads the key again after a
    /// refusal must not send a blank key.
    #[test]
    fn a_key_read_again_that_holds_only_blank_space_is_a_broken_reference() {
        let vault = TestVault::new(
            "read-again-blank",
            &[(OP_REFERENCE, KEY_BEFORE_THE_ROTATION)],
            Some("   "),
        );

        let error = read_again_from_1password(&vault, OP_REFERENCE)
            .expect_err("a blank answer must not read as a key");

        assert!(
            format!("{error:#}").contains(OP_REFERENCE),
            "the failure must name the reference that holds nothing, got {error:#}"
        );
    }

    /// A key from a reference comes back with that reference, and a key from
    /// a plaintext field comes back with none. The reference is what lets a
    /// command read the key again after the API refuses it, so the one
    /// function that decides which source wins also says which one it was.
    #[test]
    fn a_resolved_key_names_the_reference_that_holds_it() {
        let vault = TestVault::new(
            "resolved-reference",
            &[(OP_REFERENCE, KEY_BEFORE_THE_ROTATION)],
            None,
        );

        let from_the_reference = Config {
            op_path: Some(OP_REFERENCE.to_string()),
            api_key: Some("plaintext-key".into()),
            ..Config::default()
        }
        .resolve_api_key(&vault)
        .expect("the cached key must resolve");

        assert_eq!(from_the_reference.key().expose(), KEY_BEFORE_THE_ROTATION);
        assert_eq!(
            from_the_reference.reference(),
            Some(OP_REFERENCE),
            "a key from a reference must name the reference"
        );

        let from_the_plaintext_field = Config {
            api_key: Some("plaintext-key".into()),
            ..Config::default()
        }
        .resolve_api_key(&vault)
        .expect("the plaintext key must resolve");

        assert_eq!(from_the_plaintext_field.key().expose(), "plaintext-key");
        assert_eq!(
            from_the_plaintext_field.reference(),
            None,
            "a key from a plaintext field names no reference"
        );
    }
}

#[cfg(test)]
mod connection_test_tests {
    use super::*;
    use crate::prompt::Scripted;
    use crate::test_server::{json_response, json_response_with_status, TestServer};
    use crate::test_support::FAKE_CONTROLLER_KEY;

    /// The question the wizard puts before it saves a configuration that
    /// failed its connection test.
    const SAVE_ANYWAY: &str = "Save configuration anyway?";

    /// Words of the hint that tells the user a key can have limited
    /// permissions.
    const PERMISSION_HINT: &str = "limited permissions";

    /// A URL that no client can be built from.
    const MALFORMED_URL: &str = "not a url";

    /// The version the answering controller gives.
    const CONTROLLER_VERSION: &str = "9.3.45";

    /// The wizard shows the cause of a failed validation of a typed URL, not
    /// only the outer context that `validate_user_url` adds.
    #[test]
    fn a_failed_validation_of_a_typed_url_shows_its_cause() {
        const CAUSE: &str = "Connection refused (os error 61)";
        const CONTEXT: &str = "error sending request for url (https://192.168.1.1/)";
        const TYPED: &str = "https://192.168.1.1";
        let validated: Result<String> = Err(anyhow::anyhow!(CAUSE)).context(CONTEXT);
        let mut out = Vec::new();
        let mut console = Scripted::terminal(&["n\n"]);

        let kept =
            url_after_validation(validated, TYPED.to_string(), &mut out, &mut console).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert_eq!(kept, None, "a refused URL must not be kept");
        assert!(
            console.asked().contains(USE_URL_ANYWAY),
            "the wizard must ask whether to keep the URL, asked {:?}",
            console.asked()
        );
        assert!(
            text.contains(CONTEXT),
            "the context is missing from {text:?}"
        );
        assert!(text.contains(CAUSE), "the cause is missing from {text:?}");
    }

    /// A typed URL that validates is kept, with a mark of success, and the
    /// wizard asks nothing.
    #[test]
    fn a_typed_url_that_validates_is_kept_without_a_question() {
        const TYPED: &str = "https://192.168.1.1";
        let mut out = Vec::new();
        let mut console = Scripted::terminal(&[]);

        let kept = url_after_validation(
            Ok(TYPED.to_string()),
            TYPED.to_string(),
            &mut out,
            &mut console,
        )
        .unwrap();

        assert_eq!(kept.as_deref(), Some(TYPED), "a validated URL must be kept");
        assert_eq!(String::from_utf8(out).unwrap(), " ✓\n");
        assert!(
            !console.was_asked(),
            "a validated URL needs no question, asked {:?}",
            console.asked()
        );
    }

    /// A yes to the question after a failed validation keeps the typed URL,
    /// so the wizard does not ask for the URL again.
    #[test]
    fn a_yes_after_a_failed_validation_keeps_the_typed_url() {
        const TYPED: &str = "https://192.168.1.1";
        let validated: Result<String> = Err(anyhow::anyhow!("Connection refused (os error 61)"));
        let mut out = Vec::new();
        let mut console = Scripted::terminal(&["y\n"]);

        let kept =
            url_after_validation(validated, TYPED.to_string(), &mut out, &mut console).unwrap();

        assert_eq!(
            kept.as_deref(),
            Some(TYPED),
            "a URL the user chose to use anyway must be kept"
        );
        assert!(
            console.asked().contains(USE_URL_ANYWAY),
            "the wizard must ask whether to keep the URL, asked {:?}",
            console.asked()
        );
    }

    /// A controller that does not accept the key.
    fn unauthorized() -> String {
        json_response_with_status("401 Unauthorized", r#"{"message":"Unauthorized"}"#)
    }

    /// A controller that accepts the key and refuses the request.
    fn forbidden() -> String {
        json_response_with_status("403 Forbidden", r#"{"message":"Forbidden"}"#)
    }

    /// What the setup wizard shows and decides after it tests
    /// `controller_url`.
    ///
    /// The test is the real one: the real client sends the request, so each
    /// error arrives in the shape a user sees.
    ///
    /// # Arguments
    ///
    /// * `controller_url` - The URL the wizard tests.
    /// * `answers` - What the user types, in order.
    ///
    /// # Returns
    ///
    /// The decision, the text the wizard showed, and the console, which keeps
    /// every question it was asked.
    async fn setup_after_testing(
        controller_url: &str,
        answers: &[&str],
    ) -> (SaveDecision, String, Scripted) {
        let key = Secret::from(FAKE_CONTROLLER_KEY);
        let test = run_connection_test(controller_url, &key, false).await;

        let mut out = Vec::new();
        let mut console = Scripted::terminal(answers);
        let decision = decide_after_connection_test(test, &mut out, &mut console)
            .expect("a scripted console answers every question the test scripted");

        (
            decision,
            String::from_utf8(out).expect("the wizard writes UTF-8"),
            console,
        )
    }

    /// A failed connection test must not save the configuration without a
    /// question. A controller that gives no answer, a key the controller does
    /// not accept and a key without permission each fail the test, the same
    /// as a URL that no client can be built from.
    #[tokio::test]
    async fn every_failed_connection_test_asks_before_it_saves() {
        let hanging_up = TestServer::hanging_up().await;
        let unauthorized = TestServer::replying(&unauthorized()).await;
        let forbidden = TestServer::replying(&forbidden()).await;

        for (case, url) in [
            ("a URL that no client can be built from", MALFORMED_URL),
            ("a controller that hangs up", hanging_up.origin()),
            ("a 401", unauthorized.origin()),
            ("a 403", forbidden.origin()),
        ] {
            let (decision, shown, console) = setup_after_testing(url, &["n\n"]).await;

            assert!(
                console.asked().contains(SAVE_ANYWAY),
                "{case} must ask {SAVE_ANYWAY:?}, it asked {:?} and showed {shown:?}",
                console.asked()
            );
            assert_eq!(
                decision,
                SaveDecision::DoNotSave,
                "a no to {SAVE_ANYWAY:?} after {case} must save nothing"
            );
        }
    }

    /// A yes to the question saves the configuration all the same. The user
    /// can know more than the test, for example about a controller that is
    /// off today.
    #[tokio::test]
    async fn a_yes_saves_after_a_failed_connection_test() {
        let hanging_up = TestServer::hanging_up().await;

        let (decision, shown, console) = setup_after_testing(hanging_up.origin(), &["y\n"]).await;

        assert!(
            console.asked().contains(SAVE_ANYWAY),
            "a controller that hangs up must ask {SAVE_ANYWAY:?}, it showed {shown:?}"
        );
        assert_eq!(
            decision,
            SaveDecision::Save,
            "a yes to {SAVE_ANYWAY:?} must save the configuration"
        );
    }

    /// The hint about a key with limited permissions fits one answer only: a
    /// controller that took the key and refused the request (HTTP 403). For
    /// any other failure it sends the user to the wrong fix.
    #[tokio::test]
    async fn the_permission_hint_shows_for_a_403_alone() {
        let forbidden = TestServer::replying(&forbidden()).await;

        let (_, shown, _) = setup_after_testing(forbidden.origin(), &["n\n"]).await;

        assert!(
            shown.contains(PERMISSION_HINT),
            "a 403 must show the permission hint, it showed {shown:?}"
        );

        let hanging_up = TestServer::hanging_up().await;
        let unauthorized = TestServer::replying(&unauthorized()).await;

        for (case, url) in [
            ("a controller that hangs up", hanging_up.origin()),
            ("a 401", unauthorized.origin()),
        ] {
            let (_, shown, _) = setup_after_testing(url, &["n\n"]).await;

            assert!(
                !shown.contains(PERMISSION_HINT),
                "{case} must not show the permission hint, it showed {shown:?}"
            );
        }
    }

    /// A controller that gives no answer did not talk to the wizard, and the
    /// report must not say that it connected.
    #[tokio::test]
    async fn a_controller_that_gives_no_answer_is_not_reported_as_connected() {
        let hanging_up = TestServer::hanging_up().await;

        let (_, shown, _) = setup_after_testing(hanging_up.origin(), &["n\n"]).await;

        assert!(
            !shown.contains(CONNECTION_TEST_PASSED),
            "a controller that hangs up must not be reported as connected, it showed {shown:?}"
        );
        assert!(
            shown.contains(CONNECTION_TEST_FAILED),
            "a controller that hangs up must be reported as a failed test, it showed {shown:?}"
        );
    }

    /// A controller that answers passes the test, and the wizard goes on
    /// without a question.
    #[tokio::test]
    async fn a_controller_that_answers_is_saved_without_a_question() {
        let controller = TestServer::replying(&json_response(&format!(
            r#"{{"applicationVersion":"{CONTROLLER_VERSION}"}}"#
        )))
        .await;

        let (decision, shown, console) = setup_after_testing(controller.origin(), &["n\n"]).await;

        assert!(
            !console.was_asked(),
            "a passed test has nothing to ask, it asked {:?}",
            console.asked()
        );
        assert_eq!(
            decision,
            SaveDecision::Save,
            "a passed test must save the configuration"
        );
        assert!(
            shown.contains(CONTROLLER_VERSION),
            "a passed test must show the version of the controller, it showed {shown:?}"
        );
    }
}

#[cfg(test)]
mod key_prompt_tests {
    use super::*;
    use crate::prompt::Scripted;
    use crate::test_support::FAKE_CONTROLLER_KEY;

    /// The controller key the user pastes does not show on the screen. A key
    /// that shows stays in the scrollback, in a screen share and in a
    /// recording.
    #[test]
    fn the_controller_key_is_read_without_showing_it() {
        let pasted = format!("  {FAKE_CONTROLLER_KEY}  \n");
        let mut console = Scripted::terminal(&[pasted.as_str()]);

        let key = read_pasted_api_key(&mut console).expect("a scripted answer must be readable");

        assert_eq!(
            key.expose(),
            FAKE_CONTROLLER_KEY,
            "the key must arrive without the space around it"
        );
        assert_eq!(
            console.asked(),
            "",
            "the controller key must not be read with a question that shows the answer"
        );
        assert_eq!(
            console.asked_hidden(),
            API_KEY_QUESTION,
            "the controller key must be read with a hidden question"
        );
    }

    /// The hidden read gives an empty answer back, so the refusal of an empty
    /// controller key stays with the caller.
    #[test]
    fn an_empty_controller_key_is_refused() {
        let mut console = Scripted::terminal(&["\n"]);

        let error = read_pasted_api_key(&mut console).expect_err("an empty key is not a key");

        assert!(
            format!("{error:#}").contains("cannot be empty"),
            "the refusal must say the key is empty, got {error:#}"
        );
    }

    /// The Site Manager answer does not show on the screen, whether it is the
    /// key itself, a 1Password reference, or an empty line that skips.
    #[test]
    fn the_site_manager_answer_is_read_without_showing_it() {
        for (typed, answer) in [
            (
                "  sm-key-from-the-clipboard  \n",
                "sm-key-from-the-clipboard",
            ),
            (
                "op://Private/ufa/site manager key\n",
                "op://Private/ufa/site manager key",
            ),
            ("\n", ""),
        ] {
            let mut console = Scripted::terminal(&[typed]);

            let read = prompt_for_site_manager_key(&mut console)
                .expect("a scripted answer must be readable");

            assert_eq!(read, answer, "{typed:?} must arrive as {answer:?}");
            assert_eq!(
                console.asked(),
                "",
                "the Site Manager answer must not be read with a question that shows it"
            );
            assert_eq!(
                console.asked_hidden(),
                SITE_MANAGER_KEY_QUESTION,
                "the Site Manager answer must be read with a hidden question"
            );
        }
    }
}
