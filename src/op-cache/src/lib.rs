//! 1Password credential caching with retry logic, atomic writes, and worktree support.
//!
//! Fetches secrets from 1Password once and caches them in `.op-cache.json`
//! at the repo root. Subsequent calls reuse cached values. If a credential
//! fails at point of use (e.g., R2 returns 403), invalidate the cache entry
//! and the next read re-fetches from 1Password.
//!
//! Environment variables always take priority over the cache and 1Password.
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
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::NamedTempFile;

/// Maximum number of retries for 1Password CLI operations.
const OP_MAX_RETRIES: u32 = 3;

/// Cache file name (should be gitignored).
const CACHE_FILENAME: &str = ".op-cache.json";

/// Errors that can occur during 1Password caching operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The provided path does not start with `op://`.
    #[error("invalid 1Password path: \"{0}\" (must start with \"op://\")")]
    InvalidOpPath(String),

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
/// Guarantees the path starts with `op://`, preventing injection attacks.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpPath(String);

impl OpPath {
    /// Creates an `OpPath` from a string, validating the `op://` prefix.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidOpPath`] if the path doesn't start with `op://`.
    pub fn new(path: &str) -> Result<Self> {
        if !path.starts_with("op://") {
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

        // Ensure output directory exists
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)?;
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
        fs::read_to_string(&self.cache_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn write_cache(&self, cache: &CacheFile) -> Result<()> {
        if let Some(parent) = self.cache_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = NamedTempFile::new_in(
            self.cache_path
                .parent()
                .unwrap_or_else(|| Path::new(".")),
        )?;
        serde_json::to_writer_pretty(&tmp, cache)?;
        tmp.persist(&self.cache_path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        Ok(())
    }
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
        match Command::new("op")
            .args(["read", op_path.as_ref()])
            .output()
        {
            Ok(output) if output.status.success() => {
                let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !value.is_empty() && !value.contains("[ERROR]") {
                    return Ok(value);
                }
            }
            _ => {}
        }

        if attempt < OP_MAX_RETRIES {
            eprintln!(
                "Failed to read from 1Password (attempt {attempt}/{OP_MAX_RETRIES}), retrying..."
            );
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
            Ok(output) if output.status.success() => {
                if output_path.exists() && output_path.metadata().map_or(false, |m| m.len() > 0) {
                    return Ok(());
                }
            }
            _ => {}
        }

        if attempt < OP_MAX_RETRIES {
            eprintln!(
                "Failed to read binary from 1Password (attempt {attempt}/{OP_MAX_RETRIES}), retrying..."
            );
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
    }

    #[test]
    fn invalid_op_path() {
        assert!(OpPath::new("not-an-op-path").is_err());
        assert!(OpPath::new("").is_err());
        assert!(OpPath::new("op:/missing-slash").is_err());
    }

    #[test]
    fn cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join(CACHE_FILENAME);
        let cache = OpCache::with_path(cache_path.clone());

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

    #[test]
    fn env_var_override() {
        let dir = tempfile::tempdir().unwrap();
        let cache = OpCache::with_path(dir.path().join(CACHE_FILENAME));
        let path = OpPath::new("op://Private/Test/field").unwrap();

        std::env::set_var("OP_CACHE_TEST_VAR", "from-env");
        let result = cache.read(&path, Some("OP_CACHE_TEST_VAR")).unwrap();
        assert_eq!(result, "from-env");
        std::env::remove_var("OP_CACHE_TEST_VAR");
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
}
