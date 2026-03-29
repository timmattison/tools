# vpn-tunnel: Auto-Select Unused WireGuard Credential — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically pick an unused WireGuard credential from 1Password when generating a VPN tunnel, preventing concurrent tunnels from knocking each other offline.

**Architecture:** Add item-level field enumeration to `op-cache` library, then build credential selection logic in `vpn-tunnel` that compares available credentials against keys in running gluetun containers. Pure selection logic is extracted into a testable module.

**Tech Stack:** Rust, op-cache (1Password caching), Docker CLI, serde_json, clap

---

## File Structure

### op-cache crate (`src/op-cache/`)

- **Modify:** `src/lib.rs` — Add `ItemField` struct, `read_item_fields()` method, `parse_item_fields()` pure function, `fetch_item_from_1password()` helper, new error variants
- **Modify:** `src/main.rs` — Add `ListFields` subcommand for debugging

### vpn-tunnel crate (`src/vpn-tunnel/`)

- **Create:** `src/credential.rs` — Pure credential selection logic (available vs in-use → pick or error)
- **Modify:** `src/main.rs` — Change `DEFAULT_OP_PATH`, wire up auto-selection in `generate`, update `status` to show credential field
- **Modify:** `src/generator.rs` — Update `generate()` and `write_env()` to accept and store `credential_field`

---

### Task 1: op-cache — Add item field parsing and tests

**Files:**
- Modify: `src/op-cache/src/lib.rs`

This task adds the pure parsing function and types. No CLI calls yet — just parse `op item get` JSON output.

- [ ] **Step 1: Write failing tests for `parse_item_fields()`**

Add at the bottom of the `mod tests` block in `src/op-cache/src/lib.rs`:

```rust
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
    let err = result.unwrap_err();
    assert!(matches!(err, Error::NoMatchingFields { .. }));
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p op-cache`
Expected: Compilation errors — `parse_item_fields`, `ItemField`, and `Error::NoMatchingFields` don't exist yet.

- [ ] **Step 3: Add `ItemField` struct, `NoMatchingFields` error variant, and `parse_item_fields()` function**

Add the `ItemField` struct after the `OpPath` impl block (after line 108 in `src/op-cache/src/lib.rs`):

```rust
/// A single field from a 1Password item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemField {
    /// The field label (e.g., "credential", "credential-2")
    pub label: String,
    /// The field value
    pub value: String,
}
```

Add a new error variant to the `Error` enum (after the `OpReadFailed` variant):

```rust
    /// No fields matching the given prefix were found in the 1Password item.
    #[error("no fields matching prefix \"{prefix}\" found in 1Password item \"{item}\"")]
    NoMatchingFields { prefix: String, item: String },

    /// Failed to parse 1Password item JSON.
    #[error("failed to parse 1Password item JSON: {0}")]
    ItemJsonParse(String),
```

Add the `parse_item_fields` function after the `fetch_binary_from_1password` function (before `#[cfg(test)]`):

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p op-cache`
Expected: All 4 new tests pass, all existing tests still pass.

- [ ] **Step 5: Commit**

```bash
git add src/op-cache/src/lib.rs
git commit -m "feat(op-cache): add ItemField type and parse_item_fields() for field enumeration"
```

---

### Task 2: op-cache — Add `read_item_fields()` method and CLI subcommand

**Files:**
- Modify: `src/op-cache/src/lib.rs`
- Modify: `src/op-cache/src/main.rs`

This task adds the `OpCache::read_item_fields()` method that calls `op item get --format json`, uses the parsing function from Task 1, and caches the result. Also adds a CLI subcommand for debugging.

- [ ] **Step 1: Write failing test for cache roundtrip of item fields**

Add to the `mod tests` block in `src/op-cache/src/lib.rs`:

```rust
#[test]
fn item_fields_cache_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let cache = OpCache::with_path(dir.path().join(CACHE_FILENAME));

    let fields = vec![
        ItemField { label: "credential".to_string(), value: "key-1".to_string() },
        ItemField { label: "credential-2".to_string(), value: "key-2".to_string() },
    ];

    // Write to cache
    let cache_key = "op://Private/TestItem/__item_fields__/credential";
    let serialized = serde_json::to_string(&fields.iter().map(|f| (&f.label, &f.value)).collect::<Vec<_>>()).unwrap();
    let mut file: CacheFile = HashMap::new();
    file.insert(
        cache_key.to_string(),
        CacheEntry {
            value: serialized.clone(),
            fetched_at: "2026-01-01T00:00:00Z".to_string(),
        },
    );
    cache.write_cache(&file).unwrap();

    // Read back and deserialize
    let raw_cache = cache.read_cache();
    let entry = raw_cache.get(cache_key).unwrap();
    let deserialized: Vec<(String, String)> = serde_json::from_str(&entry.value).unwrap();
    assert_eq!(deserialized.len(), 2);
    assert_eq!(deserialized[0].0, "credential");
    assert_eq!(deserialized[0].1, "key-1");
}
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p op-cache item_fields_cache_roundtrip`
Expected: PASS (this test uses existing cache infrastructure — it validates the serialization approach).

- [ ] **Step 3: Add `fetch_item_from_1password()` and `OpCache::read_item_fields()` method**

Add `fetch_item_from_1password` after `fetch_binary_from_1password` in `src/op-cache/src/lib.rs`:

```rust
fn fetch_item_from_1password(op_path: &OpPath) -> Result<serde_json::Value> {
    ensure_op_available()?;

    // Extract the item reference (everything in the op:// path)
    // op_path is like "op://vault/item" — pass it directly to `op item get`
    let item_ref = op_path.as_ref();

    for attempt in 1..=OP_MAX_RETRIES {
        match Command::new("op")
            .args(["item", "get", item_ref, "--format", "json"])
            .output()
        {
            Ok(output) if output.status.success() => {
                let json_str = String::from_utf8_lossy(&output.stdout);
                match serde_json::from_str(json_str.trim()) {
                    Ok(value) => return Ok(value),
                    Err(e) => {
                        return Err(Error::ItemJsonParse(format!(
                            "invalid JSON from `op item get`: {e}"
                        )));
                    }
                }
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
```

Add `read_item_fields` method to the `impl OpCache` block, after `read_binary`:

```rust
    /// Lists fields from a 1Password item that match a given prefix.
    ///
    /// Resolution order:
    /// 1. If the item fields are in the cache, return cached values
    /// 2. Fetch from 1Password with `op item get --format json`, parse, cache, return
    ///
    /// A field matches if its label equals `field_prefix` exactly or starts with
    /// `field_prefix-`. Results are sorted alphabetically by label.
    ///
    /// # Errors
    ///
    /// Returns an error if the `op` CLI is not found, 1Password read fails,
    /// no fields match the prefix, or there's a cache IO error.
    pub fn read_item_fields(
        &self,
        op_path: &OpPath,
        field_prefix: &str,
    ) -> Result<Vec<ItemField>> {
        // Cache key: item path + sentinel + prefix
        let cache_key = format!("{}/__item_fields__/{}", op_path.as_ref(), field_prefix);

        // Check cache first
        let mut cache = self.read_cache();
        if let Some(entry) = cache.get(&cache_key) {
            let pairs: Vec<(String, String)> = serde_json::from_str(&entry.value)
                .map_err(|e| Error::ItemJsonParse(format!("cached item fields corrupted: {e}")))?;
            return Ok(pairs
                .into_iter()
                .map(|(label, value)| ItemField { label, value })
                .collect());
        }

        // Fetch from 1Password
        let item_json = fetch_item_from_1password(op_path)?;
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
```

- [ ] **Step 4: Add `ListFields` subcommand to op-cache CLI**

Add to the `Commands` enum in `src/op-cache/src/main.rs`:

```rust
    /// List fields matching a prefix from a 1Password item
    ListFields {
        /// 1Password item path (e.g., "op://Private/ProtonVPN WireGuard key")
        op_path: String,
        /// Field name prefix to filter by (e.g., "credential")
        #[arg(long, default_value = "credential")]
        prefix: String,
    },
```

Add the handler in the `run()` match block, before the closing `}`:

```rust
        Commands::ListFields { op_path, prefix } => {
            let cache = OpCache::new()?;
            let path = OpPath::new(&op_path)?;
            let fields = cache.read_item_fields(&path, &prefix)?;
            eprintln!("Fields matching \"{}\" ({}):", prefix, fields.len());
            for field in &fields {
                println!("  {} = {}...", field.label, &field.value[..8.min(field.value.len())]);
            }
        }
```

- [ ] **Step 5: Run all op-cache tests**

Run: `cargo test -p op-cache`
Expected: All tests pass. The new `read_item_fields` method compiles and the CLI builds.

- [ ] **Step 6: Verify op-cache CLI builds**

Run: `cargo build -p op-cache`
Expected: Builds without errors or warnings.

- [ ] **Step 7: Commit**

```bash
git add src/op-cache/src/lib.rs src/op-cache/src/main.rs
git commit -m "feat(op-cache): add read_item_fields() method and list-fields CLI subcommand"
```

---

### Task 3: vpn-tunnel — Add credential selection module with tests

**Files:**
- Create: `src/vpn-tunnel/src/credential.rs`
- Modify: `src/vpn-tunnel/src/main.rs` (add `mod credential;`)

This task creates the pure credential selection logic, fully testable without docker or 1Password.

- [ ] **Step 1: Add `mod credential;` to main.rs**

In `src/vpn-tunnel/src/main.rs`, add after line 2 (`mod generator;`):

```rust
mod credential;
```

- [ ] **Step 2: Write failing tests in `credential.rs`**

Create `src/vpn-tunnel/src/credential.rs`:

```rust
use op_cache::ItemField;

/// A running gluetun container and its WireGuard key.
#[derive(Debug, Clone)]
pub struct RunningTunnel {
    pub container_name: String,
    pub wireguard_key: String,
}

/// The result of credential selection.
#[derive(Debug)]
pub struct SelectedCredential {
    /// The field label (e.g., "credential-2")
    pub field_label: String,
    /// The WireGuard private key value
    pub key: String,
    /// Total number of available credentials
    pub total: usize,
    /// Number of credentials currently in use
    pub in_use: usize,
}

/// Error when all credentials are in use.
#[derive(Debug)]
pub struct AllCredentialsInUse {
    /// Each entry: (field_label, container_name)
    pub usage: Vec<(String, String)>,
}

/// Selects the first unused credential from available fields.
///
/// Compares available credential values against keys used by running tunnels.
/// Returns the first credential whose value does not appear in any running tunnel.
///
/// # Errors
///
/// Returns `AllCredentialsInUse` if every credential is used by a running container.
pub fn select_credential(
    available: &[ItemField],
    running: &[RunningTunnel],
) -> Result<SelectedCredential, AllCredentialsInUse> {
    let in_use_count = available
        .iter()
        .filter(|f| running.iter().any(|r| r.wireguard_key == f.value))
        .count();

    for field in available {
        if !running.iter().any(|r| r.wireguard_key == field.value) {
            return Ok(SelectedCredential {
                field_label: field.label.clone(),
                key: field.value.clone(),
                total: available.len(),
                in_use: in_use_count,
            });
        }
    }

    // All in use — build the usage list
    let usage: Vec<(String, String)> = available
        .iter()
        .map(|field| {
            let container = running
                .iter()
                .find(|r| r.wireguard_key == field.value)
                .map(|r| r.container_name.clone())
                .unwrap_or_else(|| "unknown".to_string());
            (field.label.clone(), container)
        })
        .collect();

    Err(AllCredentialsInUse { usage })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(label: &str, value: &str) -> ItemField {
        ItemField {
            label: label.to_string(),
            value: value.to_string(),
        }
    }

    fn tunnel(name: &str, key: &str) -> RunningTunnel {
        RunningTunnel {
            container_name: name.to_string(),
            wireguard_key: key.to_string(),
        }
    }

    #[test]
    fn single_credential_none_in_use() {
        let available = vec![field("credential", "key-1")];
        let running = vec![];
        let result = select_credential(&available, &running).unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.key, "key-1");
        assert_eq!(result.total, 1);
        assert_eq!(result.in_use, 0);
    }

    #[test]
    fn multiple_credentials_none_in_use_selects_first() {
        let available = vec![
            field("credential", "key-1"),
            field("credential-2", "key-2"),
        ];
        let running = vec![];
        let result = select_credential(&available, &running).unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.key, "key-1");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 0);
    }

    #[test]
    fn multiple_credentials_first_in_use_selects_second() {
        let available = vec![
            field("credential", "key-1"),
            field("credential-2", "key-2"),
        ];
        let running = vec![tunnel("scraper-gluetun", "key-1")];
        let result = select_credential(&available, &running).unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(result.key, "key-2");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 1);
    }

    #[test]
    fn all_credentials_in_use_returns_error() {
        let available = vec![
            field("credential", "key-1"),
            field("credential-2", "key-2"),
        ];
        let running = vec![
            tunnel("scraper-gluetun", "key-1"),
            tunnel("vpn-gluetun", "key-2"),
        ];
        let err = select_credential(&available, &running).unwrap_err();
        assert_eq!(err.usage.len(), 2);
        assert_eq!(err.usage[0], ("credential".to_string(), "scraper-gluetun".to_string()));
        assert_eq!(err.usage[1], ("credential-2".to_string(), "vpn-gluetun".to_string()));
    }

    #[test]
    fn running_tunnel_with_unknown_key_does_not_block() {
        let available = vec![field("credential", "key-1")];
        let running = vec![tunnel("other-gluetun", "different-key")];
        let result = select_credential(&available, &running).unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.in_use, 0);
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p vpn-tunnel`
Expected: All 5 credential selection tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/vpn-tunnel/src/credential.rs src/vpn-tunnel/src/main.rs
git commit -m "feat(vpn-tunnel): add credential selection module with tests"
```

---

### Task 4: vpn-tunnel — Update generator to include CREDENTIAL_FIELD in .env

**Files:**
- Modify: `src/vpn-tunnel/src/generator.rs`

- [ ] **Step 1: Update `generate()` signature to accept `credential_field`**

Change the `generate` function signature in `src/vpn-tunnel/src/generator.rs`:

```rust
pub fn generate(
    output_dir: &Path,
    city: Option<&str>,
    container_prefix: &str,
    gluetun_version: &str,
    wireguard_key: &str,
    credential_field: &str,
    extra_ports: &[String],
) -> Result<()> {
```

Update the `write_env` call inside `generate`:

```rust
    write_env(output_dir, wireguard_key, credential_field)?;
```

- [ ] **Step 2: Update `write_env()` to write `CREDENTIAL_FIELD`**

Change `write_env` in `src/vpn-tunnel/src/generator.rs`:

```rust
fn write_env(output_dir: &Path, wireguard_key: &str, credential_field: &str) -> Result<()> {
    let env_path = output_dir.join(".env");
    fs::write(
        &env_path,
        format!("WIREGUARD_PRIVATE_KEY={wireguard_key}\nCREDENTIAL_FIELD={credential_field}\n"),
    )?;
    fs::set_permissions(&env_path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}
```

- [ ] **Step 3: Verify it compiles (will fail — caller not updated yet)**

Run: `cargo build -p vpn-tunnel`
Expected: Compilation error in `main.rs` — `generate()` now requires `credential_field` parameter. This is expected and will be fixed in Task 5.

- [ ] **Step 4: Commit**

```bash
git add src/vpn-tunnel/src/generator.rs
git commit -m "feat(vpn-tunnel): add CREDENTIAL_FIELD to .env output"
```

---

### Task 5: vpn-tunnel — Wire up auto-selection in generate command

**Files:**
- Modify: `src/vpn-tunnel/src/main.rs`

This task replaces the single-field credential read with multi-field enumeration and auto-selection.

- [ ] **Step 1: Update `DEFAULT_OP_PATH` and add docker inspection helper**

In `src/vpn-tunnel/src/main.rs`, change the constant:

```rust
const DEFAULT_OP_PATH: &str = "op://Private/ProtonVPN WireGuard key";
```

Add a function after `show_vpn_ip` for detecting running tunnels:

```rust
fn find_running_tunnels() -> Vec<credential::RunningTunnel> {
    // List running gluetun containers
    let output = match Command::new("docker")
        .args(["ps", "--filter", "ancestor=qmcgaw/gluetun", "--format", "{{.Names}}"])
        .output()
    {
        Ok(out) if out.status.success() => out,
        _ => return vec![],
    };

    let container_names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    let mut tunnels = Vec::new();
    for name in container_names {
        // Extract WIREGUARD_PRIVATE_KEY from container environment
        let inspect = match Command::new("docker")
            .args(["inspect", "--format", "{{range .Config.Env}}{{println .}}{{end}}", &name])
            .output()
        {
            Ok(out) if out.status.success() => out,
            _ => continue,
        };

        let env_vars = String::from_utf8_lossy(&inspect.stdout);
        for line in env_vars.lines() {
            if let Some(key) = line.strip_prefix("WIREGUARD_PRIVATE_KEY=") {
                tunnels.push(credential::RunningTunnel {
                    container_name: name.clone(),
                    wireguard_key: key.to_string(),
                });
                break;
            }
        }
    }

    tunnels
}
```

- [ ] **Step 2: Replace the credential fetching in the `Generate` handler**

Replace the credential fetching section in the `Generate` arm (lines 142-148 in the original `main.rs`) and the `generator::generate` call with:

```rust
            // Fetch available WireGuard credentials via op-cache
            let op_path_validated =
                op_cache::OpPath::new(&op_path).map_err(|e| anyhow::anyhow!("{e}"))?;
            let cache = op_cache::OpCache::new().map_err(|e| anyhow::anyhow!("{e}"))?;

            let available_fields = cache
                .read_item_fields(&op_path_validated, "credential")
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            // Check which credentials are already in use by running gluetun containers
            let running_tunnels = find_running_tunnels();

            let selected = credential::select_credential(&available_fields, &running_tunnels)
                .map_err(|err| {
                    let mut msg = "all WireGuard credentials are in use\n".to_string();
                    for (label, container) in &err.usage {
                        msg.push_str(&format!("\n  {label:<20} -> used by {container}"));
                    }
                    msg.push_str("\n\nAdd another credential in 1Password,");
                    msg.push_str("\nor stop an existing tunnel with: vpn-tunnel down --dir <path>");
                    anyhow::anyhow!("{msg}")
                })?;

            let wg_key = &selected.key;
            let credential_field = &selected.field_label;
```

Update the `generator::generate` call to pass `credential_field`:

```rust
            generator::generate(
                &output_dir,
                city.as_deref(),
                &container_prefix,
                &gluetun_version,
                wg_key,
                credential_field,
                &extra_port_list,
            )?;
```

Update the success output to mention which credential was selected:

```rust
            println!(
                "\n{} Generated VPN tunnel in {}",
                "done:".green().bold(),
                output_dir.display()
            );
            println!(
                "Using credential: {} ({} of {} available, {} in use)",
                credential_field.cyan(),
                selected.in_use + 1,
                selected.total,
                selected.in_use
            );
```

- [ ] **Step 3: Remove the `WIREGUARD_PRIVATE_KEY` env var override from op-cache read**

The old code passed `Some("WIREGUARD_PRIVATE_KEY")` to allow env var override. Since we're now using `read_item_fields` instead of `read`, this override is no longer applicable. No code change needed — this is just confirming the old pattern is gone.

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p vpn-tunnel`
Expected: Builds without errors.

- [ ] **Step 5: Commit**

```bash
git add src/vpn-tunnel/src/main.rs
git commit -m "feat(vpn-tunnel): wire up credential auto-selection in generate command"
```

---

### Task 6: vpn-tunnel — Update status command to show credential field

**Files:**
- Modify: `src/vpn-tunnel/src/main.rs`

- [ ] **Step 1: Add helper to read CREDENTIAL_FIELD from .env**

Add a function after `find_running_tunnels` in `src/vpn-tunnel/src/main.rs`:

```rust
fn read_credential_field(dir: &Path) -> Option<String> {
    let env_path = dir.join(".env");
    let content = fs::read_to_string(env_path).ok()?;
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("CREDENTIAL_FIELD=") {
            return Some(value.to_string());
        }
    }
    None
}
```

Add `use std::fs;` if not already imported (it's not currently imported in main.rs).

- [ ] **Step 2: Update the `Status` handler to show credential field**

Change the `Commands::Status` arm in `main.rs`:

```rust
        Commands::Status { dir } => {
            ensure_compose_exists(&dir)?;
            docker_compose(&dir, &["ps"])?;
            if let Some(field) = read_credential_field(&dir) {
                println!("Credential: {}", field.cyan());
            }
            show_vpn_ip(&dir)?;
        }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p vpn-tunnel`
Expected: Builds without errors.

- [ ] **Step 4: Commit**

```bash
git add src/vpn-tunnel/src/main.rs
git commit -m "feat(vpn-tunnel): show credential field in status output"
```

---

### Task 7: Integration verification

**Files:** None (verification only)

- [ ] **Step 1: Run all tests**

Run: `cargo test -p op-cache -p vpn-tunnel`
Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p op-cache -p vpn-tunnel -- -D warnings`
Expected: No warnings.

- [ ] **Step 3: Verify both binaries build in release mode**

Run: `cargo build --release -p op-cache -p vpn-tunnel`
Expected: Builds without errors.

- [ ] **Step 4: Final commit (if any clippy/warning fixes needed)**

```bash
git add -A
git commit -m "fix: address clippy warnings"
```
