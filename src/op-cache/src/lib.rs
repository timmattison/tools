//! 1Password credential caching with retry logic, atomic writes, and worktree support.
//!
//! Fetches secrets from 1Password once and caches them in a JSON file.
//! [`OpCache::new`] puts the file at the root of the current git repository,
//! as `.op-cache.json`. [`OpCache::with_path`] puts it at the path the caller
//! gives. Subsequent calls reuse cached values. If a credential
//! fails at point of use (e.g., R2 returns 403), invalidate the cache entry
//! and the next read re-fetches from 1Password.
//!
//! Environment variables always take priority over the cache and 1Password.
//!
//! **Important:** If you use [`OpCache::new`], add `.op-cache.json` to your
//! project's `.gitignore`.
//!
//! # Who asked
//!
//! The 1Password prompt names only the application above the caller. In a
//! zellij pane, that application is the zellij server. Thus, before each run
//! of `op`, op-cache names the caller itself:
//!
//! - On macOS, it shows a notification through `osascript`. The subtitle
//!   names the Claude Code session and the zellij session. A retry adds
//!   `(attempt 2 of 3)` to the title. `OP_CACHE_NOTIFY=0` turns the
//!   notification off.
//! - It appends an `op_start` record to `$XDG_STATE_HOME/op-cache/audit.jsonl`,
//!   and an `op_end` record after `op` returns. When `XDG_STATE_HOME` is
//!   unset, empty, or relative, the file is
//!   `~/.local/state/op-cache/audit.jsonl`. The two records of one attempt
//!   share a `request_id`. The file holds JSON Lines and has mode 0600. It
//!   never holds a secret or the standard output of `op`.
//!
//! A cache hit and an environment-variable override run no `op`, thus they
//! do neither step. A failure of either step prints one warning on standard
//! error and never stops the read.
//!
//! # Usage
//!
//! ```rust,ignore
//! use op_cache::{OpCache, OpPath};
//!
//! let cache = OpCache::new()?;
//! let path = OpPath::new("op://Private/R2 Credentials/R2_ACCOUNT_ID")?;
//! let value = cache.read(&path, None)?;
//! ```

use crate::op_run::{OpRun, Operation};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::NamedTempFile;

mod audit;
mod caller;
mod notify;
mod op_run;

/// Maximum number of retries for 1Password CLI operations.
const OP_MAX_RETRIES: u32 = 3;

/// Delay between retries in milliseconds.
const RETRY_DELAY_MS: u64 = 1000;

/// Cache file name that [`OpCache::new`] uses (a project that calls it must gitignore the file).
const CACHE_FILENAME: &str = ".op-cache.json";

/// Sentinel path segment that separates an item path from the field prefix
/// whose field list is cached under it.
const ITEM_FIELDS_SENTINEL: &str = "__item_fields__";

/// Errors that can occur during 1Password caching operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The provided path is not a valid 1Password reference.
    #[error("invalid 1Password path: \"{0}\" (must start with \"op://\" and contain no shell metacharacters)")]
    InvalidOpPath(String),

    /// The provided vault and item do not name a 1Password item.
    #[error("invalid 1Password item: \"{0}\" (the vault and the item must each hold more than blank space, must not start with \"-\", and must contain no shell metacharacters)")]
    InvalidOpItem(String),

    /// The `op` CLI binary was not found in PATH.
    #[error("1Password CLI (op) not found in PATH — install with: brew install 1password-cli")]
    OpCliNotFound,

    /// All retries exhausted when reading from 1Password.
    #[error("failed to read \"{0}\" from 1Password after {OP_MAX_RETRIES} attempts")]
    OpReadFailed(String),

    /// No fields matching the given prefix were found in the 1Password item.
    #[error("no fields matching prefix \"{prefix}\" found in 1Password item \"{item}\"")]
    NoMatchingFields { prefix: String, item: String },

    /// Failed to parse 1Password item JSON.
    #[error("failed to parse 1Password item JSON: {0}")]
    ItemJsonParse(String),

    /// Not inside a git repository.
    #[error("not inside a git repository — op-cache requires a git repo to locate the cache file")]
    GitRootNotFound,

    /// IO error reading or writing the cache file.
    #[error("cache IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("cache JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A validated 1Password secret reference path.
///
/// Guarantees the path starts with `op://` and contains no shell metacharacters,
/// preventing injection attacks.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpPath(String);

impl OpPath {
    /// Creates an `OpPath` from a string, validating the format.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidOpPath`] if the path doesn't start with `op://`
    /// or contains shell metacharacters.
    pub fn new(path: &str) -> Result<Self> {
        if !path.starts_with("op://") {
            return Err(Error::InvalidOpPath(path.to_string()));
        }
        // Block shell metacharacters as defense-in-depth
        if path
            .chars()
            .any(|c| c.is_control() || ";|&$`\\".contains(c))
        {
            return Err(Error::InvalidOpPath(path.to_string()));
        }
        Ok(Self(path.to_string()))
    }

    /// Extracts vault and item segments from an item-level path.
    ///
    /// For a path like `op://vault/item`, returns `("vault", "item")`.
    /// Returns `None` if the path has too few or too many segments.
    pub fn vault_and_item(&self) -> Option<(&str, &str)> {
        let without_prefix = self.0.strip_prefix("op://")?;
        let parts: Vec<&str> = without_prefix.splitn(3, '/').collect();
        if parts.len() == 2 {
            Some((parts[0], parts[1]))
        } else {
            None
        }
    }
}

impl AsRef<str> for OpPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OpPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A validated 1Password item reference: a vault, and an item inside it.
///
/// [`OpPath`] names one field of an item. This names the item itself, which is
/// what a caller holds when it wants to know *which* fields the item has.
///
/// The two names are handed to `op` as separate arguments: the vault as the
/// value of `--vault`, and the item after `--`. Thus `op` reads neither as an
/// option, nor as part of the other. The validation still refuses a name that
/// starts with `-`, and the shell metacharacters that [`OpPath::new`] refuses,
/// as defence in depth.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OpItem {
    vault: String,
    name: String,
}

impl OpItem {
    /// Creates an `OpItem` from a vault name and an item name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidOpItem`] if either name holds nothing but blank
    /// space, starts with `-`, or carries a control character or a shell
    /// metacharacter.
    pub fn new(vault: &str, name: &str) -> Result<Self> {
        let item = Self {
            vault: vault.to_string(),
            name: name.to_string(),
        };

        if names_something(vault) && names_something(name) {
            Ok(item)
        } else {
            Err(Error::InvalidOpItem(item.to_string()))
        }
    }

    /// The item as a reference, `op://<vault>/<item>`, which is the name the
    /// audit trail and the notification give a run of `op item get`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidOpPath`] if the reference is not a valid
    /// [`OpPath`]. [`OpItem::new`] refuses each character that [`OpPath::new`]
    /// refuses, so no item it made gets this error.
    fn reference(&self) -> Result<OpPath> {
        OpPath::new(&format!("op://{}/{}", self.vault, self.name))
    }
}

/// Whether `name` can go to `op` as a vault name or as an item name.
///
/// A name of nothing but blank space names nothing. A leading `-` and the
/// metacharacters that [`OpPath::new`] refuses go as defence in depth, for the
/// reason that [`OpItem`] gives.
fn names_something(name: &str) -> bool {
    !name.trim().is_empty()
        && !name.starts_with('-')
        && !name
            .chars()
            .any(|c| c.is_control() || ";|&$`\\".contains(c))
}

impl fmt::Display for OpItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.vault, self.name)
    }
}

/// A single field from a 1Password item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemField {
    /// The field label (e.g., "credential", "credential-2")
    pub label: String,
    /// The field value
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    value: String,
    #[serde(rename = "fetchedAt")]
    fetched_at: String,
}

type CacheFile = HashMap<String, CacheEntry>;

/// The cache key prefix that every field list of one item shares.
///
/// It keeps the trailing separator, so `op://Private/Foo` cannot match the
/// keys of `op://Private/FooBar`.
fn item_fields_key_prefix(op_path: &OpPath) -> String {
    format!("{}/{ITEM_FIELDS_SENTINEL}/", op_path.as_ref())
}

/// The cache key holding the field list of one item and one field prefix.
fn item_fields_cache_key(op_path: &OpPath, field_prefix: &str) -> String {
    format!("{}{field_prefix}", item_fields_key_prefix(op_path))
}

/// 1Password credential cache manager.
///
/// Reads and writes a JSON cache file. [`OpCache::new`] puts the file at the
/// root of the current git repository. [`OpCache::with_path`] puts it at the
/// path the caller gives. The cache file is created with mode 600 on Unix
/// systems.
pub struct OpCache {
    cache_path: PathBuf,
}

impl OpCache {
    /// Creates a new `OpCache` by discovering the git repo root.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GitRootNotFound`] if not inside a git repository.
    pub fn new() -> Result<Self> {
        let root = find_repo_root()?;
        Ok(Self {
            cache_path: root.join(CACHE_FILENAME),
        })
    }

    /// Creates an `OpCache` with an explicit cache file path.
    ///
    /// Useful for testing or when the cache should live outside a git repo.
    pub fn with_path(cache_path: PathBuf) -> Self {
        Self { cache_path }
    }

    /// Reads a text secret from 1Password with file-based caching.
    ///
    /// Resolution order:
    /// 1. If `env_var` is provided and set in the environment, return it directly
    /// 2. If the op path is in the cache file, return the cached value
    /// 3. Fetch from 1Password, write to cache, return the value
    ///
    /// # Errors
    ///
    /// Returns an error if the `op` CLI is not found, 1Password read fails
    /// after retries, or there's a cache IO error.
    pub fn read(&self, op_path: &OpPath, env_var: Option<&str>) -> Result<String> {
        // 1. Environment variable override (for CI/CD)
        if let Some(var) = env_var {
            if let Ok(value) = std::env::var(var) {
                if !value.is_empty() {
                    return Ok(value);
                }
            }
        }

        // 2. Check file cache
        let mut cache = self.read_cache();
        if let Some(entry) = cache.get(op_path.as_ref()) {
            return Ok(entry.value.clone());
        }

        // 3. Fetch from 1Password and cache
        let value = fetch_from_1password(op_path, &self.cache_path)?;
        cache.insert(
            op_path.as_ref().to_string(),
            CacheEntry {
                value: value.clone(),
                fetched_at: Utc::now().to_rfc3339(),
            },
        );
        self.write_cache(&cache)?;

        Ok(value)
    }

    /// Reads a binary secret from 1Password and writes it to a file.
    ///
    /// Caches the output file path (not the binary content) so subsequent calls
    /// skip the 1Password fetch if the output file still exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the `op` CLI is not found, 1Password read fails
    /// after retries, or there's an IO error.
    pub fn read_binary(&self, op_path: &OpPath, output_path: &Path) -> Result<PathBuf> {
        // Ensure parent directory exists before canonicalizing
        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let resolved = fs::canonicalize(output_path.parent().unwrap_or(Path::new(".")))
            .unwrap_or_else(|_| output_path.parent().unwrap_or(Path::new(".")).to_path_buf())
            .join(output_path.file_name().unwrap_or_default());

        // If output file exists and is cached, skip fetching
        let mut cache = self.read_cache();
        if let Some(entry) = cache.get(op_path.as_ref()) {
            if entry.value == resolved.to_string_lossy() && resolved.exists() {
                return Ok(resolved);
            }
        }

        // Fetch from 1Password with retry
        fetch_binary_from_1password(op_path, &resolved, &self.cache_path)?;

        // Cache the output path
        cache.insert(
            op_path.as_ref().to_string(),
            CacheEntry {
                value: resolved.to_string_lossy().to_string(),
                fetched_at: Utc::now().to_rfc3339(),
            },
        );
        self.write_cache(&cache)?;

        Ok(resolved)
    }

    /// Lists fields from a 1Password item that match a given prefix.
    ///
    /// Resolution order:
    /// 1. If the item fields are in the cache, return cached values. An entry
    ///    that does not parse is a miss, as a cache file that does not parse is
    /// 2. Fetch from 1Password with `op item get --format json`, parse, cache, return
    ///
    /// A field matches if its label equals `field_prefix` exactly or starts with
    /// `field_prefix-`. Results are sorted alphabetically by label.
    ///
    /// # Errors
    ///
    /// Returns an error if the `op` CLI is not found, 1Password read fails,
    /// no fields match the prefix, or there's a cache IO error.
    pub fn read_item_fields(&self, op_path: &OpPath, field_prefix: &str) -> Result<Vec<ItemField>> {
        // Cache key: item path + sentinel + prefix
        let cache_key = item_fields_cache_key(op_path, field_prefix);

        // Check cache first
        let mut cache = self.read_cache();
        if let Some(entry) = cache.get(&cache_key) {
            match serde_json::from_str::<Vec<(String, String)>>(&entry.value) {
                Ok(pairs) => {
                    return Ok(pairs
                        .into_iter()
                        .map(|(label, value)| ItemField { label, value })
                        .collect());
                }
                Err(e) => eprintln!(
                    "warning: cached field list {cache_key} is corrupted ({}), re-fetching it",
                    describe_json_error(&e)
                ),
            }
        }

        // Fetch from 1Password
        let item_json = fetch_item_from_1password(op_path, &self.cache_path)?;
        let fields = parse_item_fields(&item_json, field_prefix)?;

        // Cache the result
        let pairs: Vec<(&str, &str)> = fields
            .iter()
            .map(|f| (f.label.as_str(), f.value.as_str()))
            .collect();
        let serialized = serde_json::to_string(&pairs)
            .map_err(|e| Error::ItemJsonParse(format!("failed to serialize fields: {e}")))?;
        cache.insert(
            cache_key,
            CacheEntry {
                value: serialized,
                fetched_at: Utc::now().to_rfc3339(),
            },
        );
        self.write_cache(&cache)?;

        Ok(fields)
    }

    /// Removes everything the cache holds for a path: the credential itself
    /// and every field list [`read_item_fields`](Self::read_item_fields) cached
    /// for it, whatever field prefix each was fetched under.
    ///
    /// The next `read()` or `read_item_fields()` call for this path re-fetches
    /// from 1Password, so a field added to the item in 1Password after the last
    /// fetch becomes visible. Another item whose path merely starts with this
    /// one (`op://Private/FooBar` beside `op://Private/Foo`) is left alone.
    ///
    /// # Errors
    ///
    /// Returns an error if there's a cache IO error.
    pub fn invalidate(&self, op_path: &OpPath) -> Result<()> {
        let mut cache = self.read_cache();
        let fields_prefix = item_fields_key_prefix(op_path);
        let before = cache.len();
        cache.retain(|key, _| key != op_path.as_ref() && !key.starts_with(&fields_prefix));
        if cache.len() != before {
            self.write_cache(&cache)?;
        }
        Ok(())
    }

    /// Removes all entries from the cache file.
    ///
    /// # Errors
    ///
    /// Returns an error if there's a cache IO error.
    pub fn clear(&self) -> Result<()> {
        if self.cache_path.exists() {
            fs::remove_file(&self.cache_path)?;
        }
        Ok(())
    }

    /// Returns the cache contents for display purposes.
    /// Values are redacted.
    ///
    /// # Errors
    ///
    /// Returns an error if there's a cache IO error.
    pub fn entries(&self) -> Result<Vec<(String, String)>> {
        let cache = self.read_cache();
        Ok(cache
            .into_iter()
            .map(|(path, entry)| (path, entry.fetched_at))
            .collect())
    }

    /// Returns the cache file path.
    pub fn cache_path(&self) -> &Path {
        &self.cache_path
    }

    fn read_cache(&self) -> CacheFile {
        match fs::read_to_string(&self.cache_path) {
            Ok(raw) => match serde_json::from_str(&raw) {
                Ok(cache) => cache,
                Err(e) => {
                    eprintln!(
                        "warning: op-cache file is corrupted ({}), re-fetching credentials",
                        describe_json_error(&e)
                    );
                    CacheFile::new()
                }
            },
            Err(_) => CacheFile::new(),
        }
    }

    fn write_cache(&self, cache: &CacheFile) -> Result<()> {
        if let Some(parent) = self.cache_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp =
            NamedTempFile::new_in(self.cache_path.parent().unwrap_or_else(|| Path::new(".")))?;
        serde_json::to_writer_pretty(&tmp, cache)?;
        tmp.persist(&self.cache_path)
            .map_err(std::io::Error::other)?;

        // Restrict permissions to owner-only since file contains secrets
        #[cfg(unix)]
        fs::set_permissions(&self.cache_path, fs::Permissions::from_mode(0o600))?;

        Ok(())
    }
}

/// The label of every field of a 1Password item, with the values discarded.
///
/// This is how a caller that must know *which* fields an item holds asks,
/// and gets no value back. `op item get --format json` prints every field of
/// the item together with the value of each concealed one. This function runs
/// that command, so every value in the item still reaches the process. The
/// values stop in a buffer inside this crate, and only the labels come back:
/// no value reaches a type or a variable of the caller.
///
/// The answer is **not cached**, and it is the one read in this crate that is
/// not. Two reasons. A label is not a secret, so none of the reasons the cache
/// exists apply to it. And a cached list of names would be wrong from the
/// moment the user adds a field to the item, with nothing to say so: a stale
/// value is found out at the point of use, when the service it authenticates
/// refuses it and the caller invalidates it, whereas a stale list of names
/// simply hides the field the user just added.
///
/// # Errors
///
/// Returns [`Error::OpCliNotFound`] if `op` is not in `PATH`,
/// [`Error::OpReadFailed`] if `op` refuses the item on every attempt, and
/// [`Error::Json`] if what `op` printed is not a 1Password item — which covers
/// output that is not JSON at all, and JSON that carries no `fields` array.
pub fn field_labels(item: &OpItem) -> Result<Vec<String>> {
    let reference = item.reference()?;
    let mut run = OpRun::new(
        Operation::ItemGet {
            vault: &item.vault,
            item: &item.name,
        },
        &reference,
        Path::new(NO_CACHE_FILE),
        OP_MAX_RETRIES,
    );

    field_labels_from(
        |attempt| fetch_item_json(&mut run, attempt, item),
        item,
        Duration::from_millis(RETRY_DELAY_MS),
    )
}

/// The cache file that [`field_labels`] names to the audit trail: none.
///
/// [`OpRun`] writes the cache file of each run into its `op_start` record, and
/// the notification falls back to the directory of that file for its
/// subtitle. A listing of labels reads no cache, so it names no file, and the
/// notification shows no directory.
const NO_CACHE_FILE: &str = "";

/// [`field_labels`] against an explicit reader of the item.
///
/// The reader is a parameter so a test can state what `op` printed without a
/// vault, a biometric prompt, or a network, and the delay is a parameter so
/// such a test does not sleep through the retries. The reader gets the number
/// of the attempt, from 1, because [`OpRun::attempt`] writes that number to
/// the audit trail and shows it in the notification.
///
/// A missing `op` ends the run at once. It is the one failure a second attempt
/// cannot change, and retrying it would make a user who never installed the
/// CLI wait three seconds to be told so.
fn field_labels_from(
    mut read_item: impl FnMut(u32) -> Result<Vec<u8>>,
    item: &OpItem,
    retry_delay: Duration,
) -> Result<Vec<String>> {
    for attempt in 1..=OP_MAX_RETRIES {
        match read_item(attempt) {
            Ok(printed) => return labels_of(&printed),
            Err(Error::OpCliNotFound) => return Err(Error::OpCliNotFound),
            Err(_) => {}
        }

        if attempt < OP_MAX_RETRIES {
            eprintln!(
                "Failed to list the fields of {item} in 1Password (attempt {attempt}/{OP_MAX_RETRIES}), retrying..."
            );
            std::thread::sleep(retry_delay * attempt);
        }
    }

    Err(Error::OpReadFailed(item.to_string()))
}

/// The label of every field of the item `printed` holds.
///
/// # Errors
///
/// Returns [`Error::Json`] if `printed` is not JSON, and if it is JSON that
/// carries no `fields` array.
fn labels_of(printed: &[u8]) -> Result<Vec<String>> {
    let item: PrintedItem = serde_json::from_slice(printed)?;

    Ok(item
        .fields
        .into_iter()
        .filter_map(|field| field.label)
        .collect())
}

/// The one part of a printed 1Password item this crate reads.
///
/// Nothing here names `value`, so `serde_json` skips every value of every
/// field along with every other key it was never asked for. A field value has
/// nowhere to land, which is what keeps it out of the answer rather than a
/// filter somebody must remember to write.
#[derive(Deserialize)]
struct PrintedItem {
    fields: Vec<PrintedField>,
}

/// One field of a printed 1Password item, reduced to its label.
///
/// A field with no label — a section that `op` prints with none — arrives as
/// `None` and is dropped.
#[derive(Deserialize)]
struct PrintedField {
    label: Option<String>,
}

/// Ask `op` to print the item, and hand back what it printed.
///
/// The run goes through [`OpRun`], the one entrance to `op`, so each attempt
/// writes the audit trail and shows the notification, as a read does.
///
/// What `op` says when it refuses goes to standard error, because "you are not
/// currently signed in" is the whole diagnosis and no error variant carries it.
///
/// # Errors
///
/// Returns [`Error::OpCliNotFound`] if `op` is not in `PATH`, and
/// [`Error::OpReadFailed`] if `op` could not be started, refused the item, or
/// printed nothing.
fn fetch_item_json(run: &mut OpRun<'_>, attempt: u32, item: &OpItem) -> Result<Vec<u8>> {
    ensure_op_available()?;

    let printed = run
        .attempt(attempt)
        .map_err(|_| Error::OpReadFailed(item.to_string()))?;

    if run.gave_value(&printed) {
        return Ok(printed.stdout);
    }

    let complaint = String::from_utf8_lossy(&printed.stderr);
    let complaint = complaint.trim();
    if !complaint.is_empty() {
        eprintln!("op could not print {item}: {complaint}");
    }

    Err(Error::OpReadFailed(item.to_string()))
}

/// Describes a JSON error by its class and its position, never by its text.
///
/// serde_json quotes a string of the wrong type in the text of its error, so
/// the text of an error from a parse of the cache can hold a cached secret.
fn describe_json_error(error: &serde_json::Error) -> String {
    let class = match error.classify() {
        serde_json::error::Category::Io => "an I/O error",
        serde_json::error::Category::Syntax => "a syntax error",
        serde_json::error::Category::Data => "a value of the wrong shape",
        serde_json::error::Category::Eof => "an early end of the input",
    };
    format!("{class} at line {} column {}", error.line(), error.column())
}

fn find_repo_root() -> Result<PathBuf> {
    let output = childenv::command("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|_| Error::GitRootNotFound)?;

    if !output.status.success() {
        return Err(Error::GitRootNotFound);
    }

    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

fn ensure_op_available() -> Result<()> {
    which::which("op").map_err(|_| Error::OpCliNotFound)?;
    Ok(())
}

fn fetch_from_1password(op_path: &OpPath, cache_path: &Path) -> Result<String> {
    ensure_op_available()?;
    let mut run = OpRun::new(Operation::Read, op_path, cache_path, OP_MAX_RETRIES);

    for attempt in 1..=OP_MAX_RETRIES {
        match run.attempt(attempt) {
            Ok(output) if run.gave_value(&output) => {
                return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
            }
            _ => {}
        }

        if attempt < OP_MAX_RETRIES {
            eprintln!(
                "Failed to read from 1Password (attempt {attempt}/{OP_MAX_RETRIES}), retrying..."
            );
            std::thread::sleep(std::time::Duration::from_millis(
                RETRY_DELAY_MS * u64::from(attempt),
            ));
        }
    }

    Err(Error::OpReadFailed(op_path.to_string()))
}

fn fetch_binary_from_1password(
    op_path: &OpPath,
    output_path: &Path,
    cache_path: &Path,
) -> Result<()> {
    ensure_op_available()?;
    let mut run = OpRun::new(
        Operation::ReadBinary { output_path },
        op_path,
        cache_path,
        OP_MAX_RETRIES,
    );

    for attempt in 1..=OP_MAX_RETRIES {
        match run.attempt(attempt) {
            Ok(output) if run.gave_value(&output) => return Ok(()),
            _ => {}
        }

        if attempt < OP_MAX_RETRIES {
            eprintln!(
                "Failed to read binary from 1Password (attempt {attempt}/{OP_MAX_RETRIES}), retrying..."
            );
            std::thread::sleep(std::time::Duration::from_millis(
                RETRY_DELAY_MS * u64::from(attempt),
            ));
        }
    }

    Err(Error::OpReadFailed(op_path.to_string()))
}

/// Runs `op item get --format json` for the item at `op_path` through
/// [`OpRun`], so each attempt writes the audit trail and shows the
/// notification, as a read does. Output that is not JSON is an error at once,
/// and no further attempt runs.
fn fetch_item_from_1password(op_path: &OpPath, cache_path: &Path) -> Result<serde_json::Value> {
    ensure_op_available()?;

    let (vault, item) = op_path.vault_and_item().ok_or_else(|| {
        Error::InvalidOpPath(format!(
            "{} (expected item-level path like op://vault/item)",
            op_path
        ))
    })?;
    let mut run = OpRun::new(
        Operation::ItemGet { vault, item },
        op_path,
        cache_path,
        OP_MAX_RETRIES,
    );

    for attempt in 1..=OP_MAX_RETRIES {
        match run.attempt(attempt) {
            Ok(output) if run.gave_value(&output) => {
                let json_str = String::from_utf8_lossy(&output.stdout);
                return serde_json::from_str(json_str.trim()).map_err(|e| {
                    Error::ItemJsonParse(format!("invalid JSON from `op item get`: {e}"))
                });
            }
            _ => {}
        }

        if attempt < OP_MAX_RETRIES {
            eprintln!(
                "Failed to get item from 1Password (attempt {attempt}/{OP_MAX_RETRIES}), retrying..."
            );
            std::thread::sleep(std::time::Duration::from_millis(
                RETRY_DELAY_MS * u64::from(attempt),
            ));
        }
    }

    Err(Error::OpReadFailed(op_path.to_string()))
}

/// Parses `op item get --format json` output and returns fields matching a prefix.
///
/// A field matches if its label equals `prefix` exactly or starts with `prefix-`.
/// Results are sorted alphabetically by label.
///
/// # Errors
///
/// Returns [`Error::NoMatchingFields`] if no fields match the prefix.
/// Returns [`Error::ItemJsonParse`] if the JSON structure is unexpected.
pub fn parse_item_fields(
    item_json: &serde_json::Value,
    field_prefix: &str,
) -> Result<Vec<ItemField>> {
    let fields = item_json
        .get("fields")
        .and_then(|f| f.as_array())
        .ok_or_else(|| Error::ItemJsonParse("missing or invalid 'fields' array".to_string()))?;

    let title = item_json
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("unknown");

    let dash_prefix = format!("{field_prefix}-");

    let mut matched: Vec<ItemField> = fields
        .iter()
        .filter_map(|f| {
            let label = f.get("label")?.as_str()?;
            let value = f.get("value")?.as_str()?;
            if label == field_prefix || label.starts_with(&dash_prefix) {
                Some(ItemField {
                    label: label.to_string(),
                    value: value.to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    if matched.is_empty() {
        return Err(Error::NoMatchingFields {
            prefix: field_prefix.to_string(),
            item: title.to_string(),
        });
    }

    matched.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(matched)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_op_path() {
        assert!(OpPath::new("op://Private/Item/field").is_ok());
        assert!(OpPath::new("op://Private/Item With Spaces/field name").is_ok());
    }

    #[test]
    fn invalid_op_path() {
        assert!(OpPath::new("not-an-op-path").is_err());
        assert!(OpPath::new("").is_err());
        assert!(OpPath::new("op:/missing-slash").is_err());
    }

    #[test]
    fn rejects_shell_metacharacters() {
        assert!(OpPath::new("op://vault/item; rm -rf /").is_err());
        assert!(OpPath::new("op://vault/item|cat /etc/passwd").is_err());
        assert!(OpPath::new("op://vault/item&background").is_err());
        assert!(OpPath::new("op://vault/item$var").is_err());
        assert!(OpPath::new("op://vault/item`cmd`").is_err());
    }

    #[test]
    fn vault_and_item_parsing() {
        let path = OpPath::new("op://Private/My Item").unwrap();
        assert_eq!(path.vault_and_item(), Some(("Private", "My Item")));

        let path = OpPath::new("op://Vault/Item With Spaces").unwrap();
        assert_eq!(path.vault_and_item(), Some(("Vault", "Item With Spaces")));

        // Three-segment path (field-level) should return None
        let path = OpPath::new("op://Private/Item/field").unwrap();
        assert_eq!(path.vault_and_item(), None);
    }

    #[test]
    fn cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join(CACHE_FILENAME);
        let cache = OpCache::with_path(cache_path);

        // Empty cache
        assert!(cache.entries().unwrap().is_empty());

        // Write and read back
        let mut file: CacheFile = HashMap::new();
        file.insert(
            "op://Private/Test/field".to_string(),
            CacheEntry {
                value: "secret123".to_string(),
                fetched_at: "2026-01-01T00:00:00Z".to_string(),
            },
        );
        cache.write_cache(&file).unwrap();

        let entries = cache.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "op://Private/Test/field");
    }

    #[cfg(unix)]
    #[test]
    fn cache_file_has_restricted_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join(CACHE_FILENAME);
        let cache = OpCache::with_path(cache_path.clone());

        let mut file: CacheFile = HashMap::new();
        file.insert(
            "op://Private/Test/field".to_string(),
            CacheEntry {
                value: "secret".to_string(),
                fetched_at: "2026-01-01T00:00:00Z".to_string(),
            },
        );
        cache.write_cache(&file).unwrap();

        let perms = fs::metadata(&cache_path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }

    /// A caller can name a cache file in a directory that does not exist yet.
    /// `ufa` does this on its first setup, when its configuration directory
    /// is not there. The first write makes the directory.
    #[test]
    fn the_first_write_makes_a_missing_cache_directory() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("not-made-yet").join(CACHE_FILENAME);
        let cache = OpCache::with_path(cache_path.clone());

        let mut file: CacheFile = HashMap::new();
        file.insert(
            "op://Private/Test/field".to_string(),
            CacheEntry {
                value: "secret".to_string(),
                fetched_at: "2026-01-01T00:00:00Z".to_string(),
            },
        );
        cache.write_cache(&file).unwrap();

        assert!(cache_path.exists(), "the first write must make the file");
        assert_eq!(cache.entries().unwrap().len(), 1);
    }

    #[test]
    fn env_var_override() {
        let dir = tempfile::tempdir().unwrap();
        let cache = OpCache::with_path(dir.path().join(CACHE_FILENAME));
        let path = OpPath::new("op://Private/Test/field").unwrap();

        // SAFETY: test runs single-threaded for this env var
        unsafe {
            std::env::set_var("OP_CACHE_TEST_VAR", "from-env");
        }
        let result = cache.read(&path, Some("OP_CACHE_TEST_VAR")).unwrap();
        assert_eq!(result, "from-env");
        // SAFETY: same justification as above — this test owns the env var and
        // runs in a single thread, so the mutation cannot race with another
        // reader of the environment.
        unsafe {
            std::env::remove_var("OP_CACHE_TEST_VAR");
        }
    }

    #[test]
    fn invalidate_removes_entry() {
        let dir = tempfile::tempdir().unwrap();
        let cache = OpCache::with_path(dir.path().join(CACHE_FILENAME));

        let mut file: CacheFile = HashMap::new();
        file.insert(
            "op://Private/Test/field".to_string(),
            CacheEntry {
                value: "secret".to_string(),
                fetched_at: "2026-01-01T00:00:00Z".to_string(),
            },
        );
        cache.write_cache(&file).unwrap();
        assert_eq!(cache.entries().unwrap().len(), 1);

        let path = OpPath::new("op://Private/Test/field").unwrap();
        cache.invalidate(&path).unwrap();
        assert!(cache.entries().unwrap().is_empty());
    }

    /// Builds a cache entry with a fixed timestamp, for tests that only care
    /// about which keys survive an operation.
    fn test_entry(value: &str) -> CacheEntry {
        CacheEntry {
            value: value.to_string(),
            fetched_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    /// Returns the keys the cache file holds, sorted.
    fn cached_keys(cache: &OpCache) -> Vec<String> {
        let mut keys: Vec<String> = cache
            .entries()
            .unwrap()
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        keys.sort();
        keys
    }

    #[test]
    fn invalidate_removes_item_field_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cache = OpCache::with_path(dir.path().join(CACHE_FILENAME));

        let item = OpPath::new("op://Private/ProtonVPN WireGuard key").unwrap();
        let mut file: CacheFile = HashMap::new();
        file.insert(item.as_ref().to_string(), test_entry("secret"));
        file.insert(
            item_fields_cache_key(&item, "credential"),
            test_entry("[[\"credential\",\"key-1\"]]"),
        );
        file.insert(
            item_fields_cache_key(&item, "username"),
            test_entry("[[\"username\",\"user\"]]"),
        );
        cache.write_cache(&file).unwrap();
        assert_eq!(cached_keys(&cache).len(), 3);

        cache.invalidate(&item).unwrap();
        assert!(
            cached_keys(&cache).is_empty(),
            "invalidate left entries behind: {:?}",
            cached_keys(&cache)
        );
    }

    #[test]
    fn invalidate_removes_item_field_entries_without_a_plain_entry() {
        let dir = tempfile::tempdir().unwrap();
        let cache = OpCache::with_path(dir.path().join(CACHE_FILENAME));

        // A `list-fields` call caches only the field list — no plain entry.
        let item = OpPath::new("op://Private/ProtonVPN WireGuard key").unwrap();
        let mut file: CacheFile = HashMap::new();
        file.insert(
            item_fields_cache_key(&item, "credential"),
            test_entry("[[\"credential\",\"key-1\"]]"),
        );
        cache.write_cache(&file).unwrap();

        cache.invalidate(&item).unwrap();
        assert!(
            cached_keys(&cache).is_empty(),
            "invalidate left entries behind: {:?}",
            cached_keys(&cache)
        );
    }

    #[test]
    fn invalidate_leaves_another_item_alone() {
        let dir = tempfile::tempdir().unwrap();
        let cache = OpCache::with_path(dir.path().join(CACHE_FILENAME));

        let target = OpPath::new("op://Private/Foo").unwrap();
        let neighbor = OpPath::new("op://Private/FooBar").unwrap();
        let neighbor_fields = item_fields_cache_key(&neighbor, "credential");
        let mut file: CacheFile = HashMap::new();
        file.insert(target.as_ref().to_string(), test_entry("foo-secret"));
        file.insert(
            item_fields_cache_key(&target, "credential"),
            test_entry("[[\"credential\",\"foo-key\"]]"),
        );
        file.insert(neighbor.as_ref().to_string(), test_entry("foobar-secret"));
        file.insert(
            neighbor_fields.clone(),
            test_entry("[[\"credential\",\"foobar-key\"]]"),
        );
        cache.write_cache(&file).unwrap();

        cache.invalidate(&target).unwrap();
        assert_eq!(
            cached_keys(&cache),
            vec![neighbor.as_ref().to_string(), neighbor_fields]
        );
    }

    #[test]
    fn clear_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join(CACHE_FILENAME);
        let cache = OpCache::with_path(cache_path.clone());

        let mut file: CacheFile = HashMap::new();
        file.insert(
            "op://Private/Test/field".to_string(),
            CacheEntry {
                value: "secret".to_string(),
                fetched_at: "2026-01-01T00:00:00Z".to_string(),
            },
        );
        cache.write_cache(&file).unwrap();
        assert!(cache_path.exists());

        cache.clear().unwrap();
        assert!(!cache_path.exists());
    }

    // -----------------------------------------------------------------------
    // Listing the fields of an item.
    //
    // Nothing below runs the real `op`. Every test states what `op` printed,
    // so no test reaches a vault, a biometric prompt, or a network.
    // -----------------------------------------------------------------------

    /// A controller key that exists nowhere but this file.
    ///
    /// The assertion that no value escapes looks for exactly this string, so
    /// that assertion can actually fail: an implementation that hands back
    /// what a field holds hands back this.
    const FAKE_CONTROLLER_KEY: &str = "not-a-real-key-4f3a2b1c";

    /// A second one, in another field of the same item.
    const FAKE_CLOUD_KEY: &str = "not-a-real-key-9d8e7f60";

    /// The label of the concealed field that holds the controller key.
    const CONTROLLER_LABEL: &str = "key - 192.168.1.1 port 443";

    /// The label of the concealed field that holds the cloud key.
    const CLOUD_LABEL: &str = "site manager key";

    /// The label of the field that carries no value, which is how `op` prints
    /// an empty note.
    const NOTES_LABEL: &str = "notesPlain";

    /// What `op item get ufa --vault Private --format json` prints: an item
    /// carrying a `fields` array, each entry with an `id`, a `type`, a
    /// `label`, and — for a concealed field — a `value`.
    fn printed_item() -> String {
        format!(
            r#"{{
  "id": "hcxxfyzabc123",
  "title": "ufa",
  "version": 4,
  "vault": {{ "id": "vaultid", "name": "Private" }},
  "category": "SECURE_NOTE",
  "fields": [
    {{ "id": "notesPlain", "type": "STRING", "purpose": "NOTES", "label": "{NOTES_LABEL}" }},
    {{ "id": "kzq6", "section": {{ "id": "sect1" }}, "type": "CONCEALED", "label": "{CONTROLLER_LABEL}", "value": "{FAKE_CONTROLLER_KEY}", "reference": "op://Private/ufa/{CONTROLLER_LABEL}" }},
    {{ "id": "pw27", "section": {{ "id": "sect1" }}, "type": "CONCEALED", "label": "{CLOUD_LABEL}", "value": "{FAKE_CLOUD_KEY}", "reference": "op://Private/ufa/{CLOUD_LABEL}" }}
  ],
  "createdAt": "2026-01-01T00:00:00Z",
  "updatedAt": "2026-01-02T00:00:00Z"
}}"#
        )
    }

    /// The item every test below reads.
    fn ufa_item() -> OpItem {
        OpItem::new("Private", "ufa").expect("Private/ufa names an item")
    }

    /// The labels of the item `printed` holds, with no delay between retries.
    fn labels_of_printed(printed: &str) -> Result<Vec<String>> {
        let item = ufa_item();
        field_labels_from(|_| Ok(printed.as_bytes().to_vec()), &item, Duration::ZERO)
    }

    /// The caller asked which fields the item holds, so it gets all of them.
    #[test]
    fn every_field_label_of_the_item_is_answered() {
        let labels = labels_of_printed(&printed_item()).expect("op printed an item");

        assert_eq!(
            labels,
            vec![
                NOTES_LABEL.to_string(),
                CONTROLLER_LABEL.to_string(),
                CLOUD_LABEL.to_string(),
            ],
            "every field of the item must be named, in the order op printed them"
        );
    }

    /// `op item get --format json` prints the value of every concealed field
    /// beside its label. The caller asked which fields the item holds, not
    /// what they hold, so the values stop here.
    #[test]
    fn no_field_value_reaches_the_caller() {
        let labels = labels_of_printed(&printed_item()).expect("op printed an item");

        assert!(
            labels.iter().any(|label| label == CONTROLLER_LABEL),
            "the concealed field must be named, or this test proves nothing, got {labels:?}"
        );
        for key in [FAKE_CONTROLLER_KEY, FAKE_CLOUD_KEY] {
            assert!(
                !labels.iter().any(|label| label.contains(key)),
                "a field value must not leave this crate, got {labels:?}"
            );
        }
    }

    /// `op` writes its complaints to standard output under some subcommands,
    /// so a successful exit is not a promise of JSON.
    #[test]
    fn output_that_is_not_json_is_refused() {
        let error = labels_of_printed("[ERROR] 2026/01/01 you are not currently signed in")
            .expect_err("output that is not JSON names no fields");

        assert!(
            matches!(error, Error::ItemJsonParse(_)),
            "output that is not a 1Password item is a fault of the item, got {error:?}"
        );
    }

    /// A listing of labels reads no cache file. Its parse error must not send
    /// the user to one, and it must not quote the printed item, because the
    /// item holds the value of each concealed field.
    #[test]
    fn a_parse_error_names_no_cache_and_quotes_no_value() {
        let printed = format!(r#"{{"id": "hcxxfyzabc123", "fields": "{FAKE_CONTROLLER_KEY}"}}"#);

        let error = labels_of_printed(&printed)
            .expect_err("a fields entry that is not an array names no fields");
        let message = error.to_string();

        assert!(
            !message.contains("cache"),
            "the listing reads no cache, so its error must not name one, got {message}"
        );
        assert!(
            !message.contains(FAKE_CONTROLLER_KEY),
            "a field value must not leave this crate in an error, got {message}"
        );
    }

    /// An item of a category that carries no fields at all parses as JSON and
    /// still names no field. Answering with an empty list would report "this
    /// item holds no controllers" for an item the guard never read.
    #[test]
    fn an_item_with_no_fields_array_is_refused() {
        let error = labels_of_printed(r#"{"id": "hcxxfyzabc123", "title": "ufa"}"#)
            .expect_err("an item with no fields array names no fields");

        assert!(
            matches!(error, Error::ItemJsonParse(_)),
            "an item with no fields array is a fault of the item, got {error:?}"
        );
    }

    /// A CLI that is not installed is not installed on the second attempt
    /// either, so retrying only makes the user wait to be told so.
    #[test]
    fn a_missing_op_cli_is_reported_at_once() {
        let item = ufa_item();
        let mut attempts: u32 = 0;

        let error = field_labels_from(
            |_| {
                attempts += 1;
                Err(Error::OpCliNotFound)
            },
            &item,
            Duration::ZERO,
        )
        .expect_err("a 1Password CLI that is not there answers nothing");

        assert!(
            matches!(error, Error::OpCliNotFound),
            "the failure must say the CLI is missing, got {error:?}"
        );
        assert_eq!(attempts, 1, "a missing CLI must not be asked twice");
    }

    /// Every other refusal gets the retries the rest of this crate gives a
    /// read, and the failure names the item it could not list. Each attempt
    /// gets its own number, because the audit trail and the notification show
    /// that number.
    #[test]
    fn a_refusal_is_retried_and_then_reported() {
        let item = ufa_item();
        let mut attempts: Vec<u32> = Vec::new();

        let error = field_labels_from(
            |attempt| {
                attempts.push(attempt);
                Err(Error::OpReadFailed(item.to_string()))
            },
            &item,
            Duration::ZERO,
        )
        .expect_err("an op that refuses every attempt answers nothing");

        assert!(
            matches!(&error, Error::OpReadFailed(named) if named == &item.to_string()),
            "the failure must name the item it could not list, got {error:?}"
        );
        assert_eq!(
            attempts,
            (1..=OP_MAX_RETRIES).collect::<Vec<u32>>(),
            "every attempt allowed must be spent, in order, before the run is given up"
        );
    }

    /// A name of nothing but blank space names nothing at all. A leading `-`
    /// and a shell metacharacter are refused as defence in depth, for the
    /// reason that the doc of `OpItem` gives.
    #[test]
    fn a_vault_or_an_item_that_names_nothing_is_refused() {
        for (vault, name) in [
            ("Private", "--vault"),
            ("Private", "-ufa"),
            ("-Private", "ufa"),
            ("Private", ""),
            ("Private", "   "),
            ("", "ufa"),
            ("Private", "ufa; rm -rf /"),
            ("Private", "ufa`whoami`"),
            ("Private", "ufa$HOME"),
            ("Private", "ufa|cat"),
            ("Private", "ufa&"),
            ("Private", "ufa\\"),
            ("Private", "ufa\n"),
        ] {
            assert!(
                OpItem::new(vault, name).is_err(),
                "{vault:?}/{name:?} does not name a 1Password item"
            );
        }
    }

    /// The ordinary case, and the shapes a real vault and a real item use.
    #[test]
    fn an_ordinary_vault_and_item_name_an_item() {
        let item = OpItem::new("Private", "ufa").expect("Private/ufa names an item");
        assert_eq!(item.to_string(), "Private/ufa");

        assert!(
            OpItem::new("Shared Vault", "ufa - staging").is_ok(),
            "a space and a hyphen inside a name are ordinary"
        );
    }

    #[test]
    fn parse_item_fields_single_credential() {
        let json = serde_json::json!({
            "id": "abc123",
            "title": "ProtonVPN WireGuard key",
            "fields": [
                {"id": "username", "label": "username", "value": "user@example.com", "type": "STRING"},
                {"id": "credential", "label": "credential", "value": "key-1-secret", "type": "CONCEALED"}
            ]
        });
        let fields = parse_item_fields(&json, "credential").unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].label, "credential");
        assert_eq!(fields[0].value, "key-1-secret");
    }

    #[test]
    fn parse_item_fields_multiple_credentials() {
        let json = serde_json::json!({
            "id": "abc123",
            "title": "ProtonVPN WireGuard key",
            "fields": [
                {"id": "credential", "label": "credential", "value": "key-1-secret", "type": "CONCEALED"},
                {"id": "cred2", "label": "credential-2", "value": "key-2-secret", "type": "CONCEALED"},
                {"id": "username", "label": "username", "value": "user@example.com", "type": "STRING"},
                {"id": "credback", "label": "credential-backup", "value": "key-3-secret", "type": "CONCEALED"}
            ]
        });
        let fields = parse_item_fields(&json, "credential").unwrap();
        assert_eq!(fields.len(), 3);
        // Should be sorted alphabetically by label
        assert_eq!(fields[0].label, "credential");
        assert_eq!(fields[0].value, "key-1-secret");
        assert_eq!(fields[1].label, "credential-2");
        assert_eq!(fields[1].value, "key-2-secret");
        assert_eq!(fields[2].label, "credential-backup");
        assert_eq!(fields[2].value, "key-3-secret");
    }

    #[test]
    fn parse_item_fields_missing_fields_array() {
        let json = serde_json::json!({"id": "abc123", "title": "Bad item"});
        let result = parse_item_fields(&json, "credential");
        assert!(matches!(result.unwrap_err(), Error::ItemJsonParse(_)));
    }

    #[test]
    fn parse_item_fields_fields_not_array() {
        let json =
            serde_json::json!({"id": "abc123", "title": "Bad item", "fields": "not-an-array"});
        let result = parse_item_fields(&json, "credential");
        assert!(matches!(result.unwrap_err(), Error::ItemJsonParse(_)));
    }

    #[test]
    fn parse_item_fields_no_matching_fields() {
        let json = serde_json::json!({
            "id": "abc123",
            "title": "ProtonVPN WireGuard key",
            "fields": [
                {"id": "username", "label": "username", "value": "user@example.com", "type": "STRING"},
                {"id": "password", "label": "password", "value": "pass123", "type": "CONCEALED"}
            ]
        });
        let result = parse_item_fields(&json, "credential");
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::NoMatchingFields { prefix, item } => {
                assert_eq!(prefix, "credential");
                assert_eq!(item, "ProtonVPN WireGuard key");
            }
            other => panic!("expected NoMatchingFields, got {other:?}"),
        }
    }

    #[test]
    fn read_item_fields_cache_hit() {
        let dir = tempfile::tempdir().unwrap();
        let cache = OpCache::with_path(dir.path().join(CACHE_FILENAME));

        // Pre-populate cache using the same key format as read_item_fields
        let op_path = OpPath::new("op://Private/TestItem").unwrap();
        let cache_key = item_fields_cache_key(&op_path, "credential");
        let pairs = vec![("credential", "key-1"), ("credential-2", "key-2")];
        let serialized = serde_json::to_string(&pairs).unwrap();
        let mut file: CacheFile = HashMap::new();
        file.insert(
            cache_key,
            CacheEntry {
                value: serialized,
                fetched_at: "2026-01-01T00:00:00Z".to_string(),
            },
        );
        cache.write_cache(&file).unwrap();

        // Exercise the actual method through cache-hit path
        let fields = cache.read_item_fields(&op_path, "credential").unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].label, "credential");
        assert_eq!(fields[0].value, "key-1");
        assert_eq!(fields[1].label, "credential-2");
        assert_eq!(fields[1].value, "key-2");
    }

    #[test]
    fn parse_item_fields_excludes_partial_prefix_matches() {
        let json = serde_json::json!({
            "id": "abc123",
            "title": "Test item",
            "fields": [
                {"id": "cred", "label": "credential", "value": "key-1", "type": "CONCEALED"},
                {"id": "creds", "label": "credentials", "value": "key-2", "type": "CONCEALED"}
            ]
        });
        // "credentials" should NOT match — only "credential" (exact) or "credential-*" (with dash)
        let fields = parse_item_fields(&json, "credential").unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].label, "credential");
    }
}
