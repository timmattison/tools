//! A 1Password vault for tests, in front of a real op-cache file.
//!
//! No test may start the real `op`. The machine that runs the tests can have
//! 1Password installed, and a stray read then asks for a fingerprint or fails.
//! So the vault here answers a read that the cache file does not answer, with
//! the key that the test says the vault holds now.
//!
//! Every other operation goes to the store that every command uses,
//! [`CachedOnePassword`], over a real cache file in a directory of the test's
//! own: a read that the cache file answers, and every forget. A test therefore
//! runs the real `OpCache::invalidate` on a real file.
//!
//! A read that succeeds at the vault writes the key into the cache file, as
//! op-cache does after it asks 1Password. A failed read writes nothing.

use crate::config::{op_cache_in, CachedOnePassword, KeyStore};
use anyhow::{Context, Result};
use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The time that each seeded cache entry says it was read from 1Password.
const SEEDED_AT: &str = "2026-01-01T00:00:00Z";

/// The time that a cache entry says it was read from 1Password, when a read
/// of the vault wrote it.
const VAULT_READ_AT: &str = "2026-01-02T00:00:00Z";

/// The count of the directories that [`TempConfigDir::new`] named in this
/// process. Two reads of the clock in one tick give one value, thus the count
/// keeps two tests with one label apart.
static TEMP_CONFIG_DIRS: AtomicU64 = AtomicU64::new(0);

/// A throwaway config directory that never touches the developer's real
/// `~/.config/ufa`.
///
/// The directory name is keyed on the process id, a nanosecond timestamp, and
/// a count of the directories of this process, so two concurrent `cargo test`
/// runs — or two tests inside one run — can never collide on the same file.
pub(crate) struct TempConfigDir {
    /// The directory. Nothing makes it until a test writes into it.
    pub(crate) dir: PathBuf,
}

impl TempConfigDir {
    /// Name a directory that no other test and no other run names.
    ///
    /// # Arguments
    ///
    /// * `label` - A word in the directory name, so a person who reads the
    ///   temporary directory can tell which test made it.
    ///
    /// # Returns
    ///
    /// The directory. It does not exist yet.
    ///
    /// # Panics
    ///
    /// Panics if the system clock is before the Unix epoch.
    pub(crate) fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        let count = TEMP_CONFIG_DIRS.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ufa-config-{label}-{}-{nanos}-{count}",
            std::process::id()
        ));
        Self { dir }
    }

    /// The configuration file in this directory.
    ///
    /// # Returns
    ///
    /// The path of `config.toml` in this directory.
    pub(crate) fn config_file(&self) -> PathBuf {
        self.dir.join("config.toml")
    }
}

impl Drop for TempConfigDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A 1Password vault whose key can differ from the copy in the cache file.
///
/// A key rotation in 1Password is exactly that: the cache file holds the key
/// from before the rotation, and the vault holds the key from after it.
///
/// A read that goes to the vault writes the key into the cache file, as
/// op-cache does after it asks 1Password.
pub(crate) struct TestVault {
    /// The directory that holds the cache file. It is here for its drop,
    /// which removes the directory when the vault goes.
    _directory: TempConfigDir,
    /// The store that every command uses, over the cache file of this vault.
    keys: CachedOnePassword,
    /// The same cache file, read to learn which references it holds.
    cache: op_cache::OpCache,
    /// What the vault holds at every reference now, or `None` for a vault
    /// that cannot be read.
    current: Option<String>,
    /// How many reads the cache file did not answer.
    vault_reads: Cell<usize>,
    /// How many forgets arrived.
    forgets: Cell<usize>,
}

impl TestVault {
    /// A vault that holds `current` now, behind a cache file that holds
    /// `cached`.
    ///
    /// # Arguments
    ///
    /// * `label` - A word in the name of the directory of the vault.
    /// * `cached` - Pairs of a 1Password reference and the key that the
    ///   cache file holds for it. An empty list writes no cache file.
    /// * `current` - What the vault holds at every reference now, or `None`
    ///   for a vault that cannot be read.
    ///
    /// # Returns
    ///
    /// The vault.
    ///
    /// # Panics
    ///
    /// Panics if the cache file cannot be written, and if op-cache does not
    /// find each seeded reference in it. The second check comes before any
    /// read: a read of a reference that op-cache does not find goes to the
    /// vault, and a test that seeded the reference expects the cache to
    /// answer.
    pub(crate) fn new(label: &str, cached: &[(&str, &str)], current: Option<&str>) -> Self {
        let directory = TempConfigDir::new(label);

        if !cached.is_empty() {
            let path = op_cache_in(&directory.dir).cache_path().to_path_buf();
            let mut entries = serde_json::Map::new();
            for (reference, key) in cached {
                put_entry(&mut entries, reference, key, SEEDED_AT);
            }
            write_entries(&path, entries);
        }

        let vault = Self {
            keys: CachedOnePassword::in_directory(&directory.dir),
            cache: op_cache_in(&directory.dir),
            _directory: directory,
            current: current.map(str::to_string),
            vault_reads: Cell::new(0),
            forgets: Cell::new(0),
        };

        for (reference, _) in cached {
            assert!(
                vault.caches(reference),
                "op-cache must find the seeded reference {reference}"
            );
        }

        vault
    }

    /// Whether the cache file holds a copy of the key at `reference`.
    ///
    /// # Arguments
    ///
    /// * `reference` - The 1Password reference to look for.
    ///
    /// # Returns
    ///
    /// Whether op-cache finds the reference in the cache file.
    ///
    /// # Panics
    ///
    /// Panics if op-cache cannot list the cache file.
    pub(crate) fn caches(&self, reference: &str) -> bool {
        self.cache
            .entries()
            .expect("op-cache must list the cache file of a test vault")
            .iter()
            .any(|(cached, _)| cached == reference)
    }

    /// The text of the cache file.
    ///
    /// # Returns
    ///
    /// What the cache file holds, or an empty string when there is no file.
    pub(crate) fn cache_file(&self) -> String {
        fs::read_to_string(self.cache.cache_path()).unwrap_or_default()
    }

    /// How many reads the cache file did not answer.
    ///
    /// # Returns
    ///
    /// The number of reads that went to the vault.
    pub(crate) fn vault_reads(&self) -> usize {
        self.vault_reads.get()
    }

    /// How many forgets arrived.
    ///
    /// # Returns
    ///
    /// The number of forgets.
    pub(crate) fn forgets(&self) -> usize {
        self.forgets.get()
    }
}

impl KeyStore for TestVault {
    fn read(&self, reference: &str) -> Result<String> {
        // The cache file holds the reference, so op-cache answers from the
        // file and starts no `op`.
        if self.caches(reference) {
            return self.keys.read(reference);
        }

        self.vault_reads.set(self.vault_reads.get() + 1);
        let key = self
            .current
            .clone()
            .with_context(|| format!("the vault of this test cannot be read at {reference}"))?;

        // op-cache keeps the key that 1Password gave. A failed read above
        // returned before this point, so it writes nothing.
        let path = self.cache.cache_path();
        let mut entries = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        put_entry(&mut entries, reference, &key, VAULT_READ_AT);
        write_entries(path, entries);

        Ok(key)
    }

    fn forget(&self, reference: &str) -> Result<()> {
        self.forgets.set(self.forgets.get() + 1);
        self.keys.forget(reference)
    }
}

/// Put one cache entry in the shape that op-cache writes.
fn put_entry(
    entries: &mut serde_json::Map<String, serde_json::Value>,
    reference: &str,
    key: &str,
    fetched_at: &str,
) {
    entries.insert(
        reference.to_string(),
        serde_json::json!({ "value": key, "fetchedAt": fetched_at }),
    );
}

/// Write the entries to the cache file, and make its directory first, as
/// op-cache does.
///
/// # Panics
///
/// Panics if the directory cannot be made, or if the file cannot be written.
fn write_entries(path: &std::path::Path, entries: serde_json::Map<String, serde_json::Value>) {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)
            .unwrap_or_else(|error| panic!("{} must be creatable: {error}", directory.display()));
    }
    fs::write(path, serde_json::Value::Object(entries).to_string())
        .unwrap_or_else(|error| panic!("{} must be writable: {error}", path.display()));
}
