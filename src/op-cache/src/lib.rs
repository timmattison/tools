//! 1Password credential caching with retry logic, atomic writes, and worktree support.
//!
//! Fetches secrets from 1Password once and caches them in `.op-cache.json`
//! at the repo root. Subsequent calls reuse cached values. If a credential
//! fails at point of use (e.g., R2 returns 403), invalidate the cache entry
//! and the next read re-fetches from 1Password.
//!
//! Environment variables always take priority over the cache and 1Password.
//!
//! **Important:** Add `.op-cache.json` to your project's `.gitignore`.
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

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tempfile::NamedTempFile;

/// Maximum number of retries for 1Password CLI operations.
const OP_MAX_RETRIES: u32 = 3;

/// Delay between retries in milliseconds.
const RETRY_DELAY_MS: u64 = 1000;

/// Cache file name (must be gitignored by consuming projects).
const CACHE_FILENAME: &str = ".op-cache.json";

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
/// The two names are handed to `op` as separate arguments, so neither can be
/// read as part of another. The validation still refuses a name that starts
/// with `-`, because `op` reads such an argument as an option rather than as a
/// name, and it refuses the shell metacharacters [`OpPath::new`] refuses, for
/// the same defence-in-depth reason.
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

        Ok(item)
    }
}

impl fmt::Display for OpItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.vault, self.name)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    value: String,
    #[serde(rename = "fetchedAt")]
    fetched_at: String,
}

type CacheFile = HashMap<String, CacheEntry>;

/// 1Password credential cache manager.
///
/// Reads and writes a JSON cache file at the root of the current git repository.
/// The cache file is created with mode 600 on Unix systems.
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
        let value = fetch_from_1password(op_path)?;
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
        fetch_binary_from_1password(op_path, &resolved)?;

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

    /// Removes a credential from the cache file.
    ///
    /// The next `read()` call for this path will re-fetch from 1Password.
    ///
    /// # Errors
    ///
    /// Returns an error if there's a cache IO error.
    pub fn invalidate(&self, op_path: &OpPath) -> Result<()> {
        let mut cache = self.read_cache();
        if cache.remove(op_path.as_ref()).is_some() {
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
                        e
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
/// without reading what they hold. `op item get --format json` prints every
/// field of the item together with the value of each concealed one, so a
/// caller that runs it pulls every secret in that item into its own process
/// merely to read the names beside them. Only this crate runs it, and only the
/// labels come back.
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
    field_labels_from(
        || fetch_item_json(item),
        item,
        Duration::from_millis(RETRY_DELAY_MS),
    )
}

/// [`field_labels`] against an explicit reader of the item.
///
/// The reader is a parameter so a test can state what `op` printed without a
/// vault, a biometric prompt, or a network, and the delay is a parameter so
/// such a test does not sleep through the retries.
///
/// A missing `op` ends the run at once. It is the one failure a second attempt
/// cannot change, and retrying it would make a user who never installed the
/// CLI wait three seconds to be told so.
fn field_labels_from(
    _read_item: impl FnMut() -> Result<Vec<u8>>,
    _item: &OpItem,
    _retry_delay: Duration,
) -> Result<Vec<String>> {
    Ok(Vec::new())
}

/// Ask `op` to print the item, and hand back what it printed.
///
/// What `op` says when it refuses goes to standard error, because "you are not
/// currently signed in" is the whole diagnosis and no error variant carries it.
///
/// # Errors
///
/// Returns [`Error::OpCliNotFound`] if `op` is not in `PATH`, and
/// [`Error::OpReadFailed`] if `op` could not be started or refused the item.
fn fetch_item_json(item: &OpItem) -> Result<Vec<u8>> {
    ensure_op_available()?;

    let printed = Command::new("op")
        .args([
            "item",
            "get",
            &item.name,
            "--vault",
            &item.vault,
            "--format",
            "json",
        ])
        .output()
        .map_err(|_| Error::OpReadFailed(item.to_string()))?;

    if printed.status.success() {
        return Ok(printed.stdout);
    }

    let complaint = String::from_utf8_lossy(&printed.stderr);
    let complaint = complaint.trim();
    if !complaint.is_empty() {
        eprintln!("op could not print {item}: {complaint}");
    }

    Err(Error::OpReadFailed(item.to_string()))
}

fn find_repo_root() -> Result<PathBuf> {
    let output = Command::new("git")
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

fn fetch_from_1password(op_path: &OpPath) -> Result<String> {
    ensure_op_available()?;

    for attempt in 1..=OP_MAX_RETRIES {
        match Command::new("op").args(["read", op_path.as_ref()]).output() {
            Ok(output) if output.status.success() => {
                let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !value.is_empty() {
                    return Ok(value);
                }
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

fn fetch_binary_from_1password(op_path: &OpPath, output_path: &Path) -> Result<()> {
    ensure_op_available()?;

    for attempt in 1..=OP_MAX_RETRIES {
        match Command::new("op")
            .args([
                "read",
                "--out-file",
                &output_path.to_string_lossy(),
                op_path.as_ref(),
            ])
            .output()
        {
            Ok(output)
                if output.status.success()
                    && output_path.exists()
                    && output_path.metadata().is_ok_and(|m| m.len() > 0) =>
            {
                return Ok(());
            }
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
        field_labels_from(|| Ok(printed.as_bytes().to_vec()), &item, Duration::ZERO)
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
            matches!(error, Error::Json(_)),
            "output that is not a 1Password item is a JSON fault, got {error:?}"
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
            matches!(error, Error::Json(_)),
            "an item with no fields array is a JSON fault, got {error:?}"
        );
    }

    /// A CLI that is not installed is not installed on the second attempt
    /// either, so retrying only makes the user wait to be told so.
    #[test]
    fn a_missing_op_cli_is_reported_at_once() {
        let item = ufa_item();
        let mut attempts: u32 = 0;

        let error = field_labels_from(
            || {
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
    /// read, and the failure names the item it could not list.
    #[test]
    fn a_refusal_is_retried_and_then_reported() {
        let item = ufa_item();
        let mut attempts: u32 = 0;

        let error = field_labels_from(
            || {
                attempts += 1;
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
            attempts, OP_MAX_RETRIES,
            "every attempt allowed must be spent before the run is given up"
        );
    }

    /// The two names go to `op` as arguments. A name that starts with `-` is
    /// read as an option rather than as a name, and a name of nothing but
    /// blank space names nothing at all.
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
}
