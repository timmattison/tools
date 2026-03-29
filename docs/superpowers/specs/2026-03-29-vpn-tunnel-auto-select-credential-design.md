# vpn-tunnel: Auto-Select Unused WireGuard Credential

**Issue:** #191
**Date:** 2026-03-29

## Problem

`vpn-tunnel generate` always uses a single credential field from 1Password. When multiple projects need concurrent VPN tunnels with the same 1Password item, the second tunnel disconnects the first because both use the same WireGuard key.

## Solution Overview

1. Add field enumeration to `op-cache` library so it can list all credential fields on an item
2. Modify `vpn-tunnel generate` to auto-select the first unused credential by comparing available fields against keys in running gluetun containers
3. Store the selected credential field name in `.env` for status reporting

## Design

### 1. op-cache Library Changes

**New method on `OpCache`:**

```rust
pub fn read_item_fields(
    &self,
    op_path: &OpPath,
    field_prefix: &str,
) -> Result<Vec<(String, String)>>
```

**Behavior:**
- Calls `op item get "<vault/item>" --format json` via the 1Password CLI
- Parses the JSON response and filters fields whose `label` equals `field_prefix` or starts with `field_prefix-`
- Returns `Vec<(field_label, field_value)>` sorted alphabetically by label (`credential` before `credential-2`, etc.)
- Caches the full item field data in `.op-cache.json` under a synthetic key like `op://Private/ProtonVPN WireGuard key/__item_fields__` (the item-level path with `/__item_fields__` appended) to avoid repeated CLI calls. The cached value is the serialized list of `(label, value)` pairs.
- Uses the same retry logic (3 attempts, exponential backoff) as existing `read()`
- Returns an error if no fields match the prefix

**OpPath changes:**
- `--op-path` now expects an item-level path: `op://vault/item` (no trailing field segment)
- The default changes from `op://Private/ProtonVPN WireGuard key/credential` to `op://Private/ProtonVPN WireGuard key`
- This is a breaking change for anyone who customized `--op-path` with a field-level path

### 2. vpn-tunnel Credential Auto-Selection

The `generate` command flow becomes:

1. **Enumerate credentials:** Call `op_cache.read_item_fields(&op_path, "credential")` to get all matching fields
2. **Error if none found:** Clear error about expected 1Password item structure
3. **Detect in-use keys:** Run `docker ps --filter ancestor=qmcgaw/gluetun --format '{{.Names}}'` to find running gluetun containers, then `docker inspect <name>` to extract `WIREGUARD_PRIVATE_KEY` from each container's environment
4. **Match and select:** Compare available credential values against in-use keys. Pick the first unused one
5. **Error if all in use:** Display which container is using each key:
   ```
   error: all WireGuard credentials are in use

     credential    -> used by scraper-gluetun
     credential-2  -> used by vpn-gluetun

   Add another credential to "ProtonVPN WireGuard key" in 1Password,
   or stop an existing tunnel with: vpn-tunnel down --dir <path>
   ```
6. **Generate:** Pass selected key + field name to the generator

### 3. .env and Status Changes

**.env file** gains a new variable:
```
WIREGUARD_PRIVATE_KEY=<selected key>
CREDENTIAL_FIELD=credential-2
```

**`vpn-tunnel status`** enhanced to:
- Read `CREDENTIAL_FIELD` from `.env` in the target directory
- Display which credential field is in use (e.g., `Credential: credential-2`)

**`vpn-tunnel generate`** success output includes:
```
Using credential: credential-2 (1 of 3 available, 1 in use)
```

### 4. Known Limitations

- **Race condition at generate time:** If two `generate` commands run simultaneously before either calls `up`, both could select the same key. This is a narrow window and the VPN provider will reject the duplicate, making it diagnosable. No lockfile mitigation for now.
- **Docker must be running** for in-use detection. If docker is down, no containers are running, so there are no conflicts — this is fine.

## Testing

### op-cache library tests (pure, no external dependencies)

- Parse item JSON with single `credential` field -> returns one entry
- Parse item JSON with multiple `credential*` fields -> returns sorted entries
- Parse item JSON with no `credential` fields -> returns error
- Non-matching fields (`username`, `password`) are excluded

### vpn-tunnel tests (pure selection logic, no docker/1Password)

- Single credential, none in use -> selects it
- Multiple credentials, none in use -> selects first
- Multiple credentials, first in use -> selects second
- All credentials in use -> returns error with container names
- Credential field name stored correctly in .env output

The matching logic (available keys vs in-use keys -> selection) is extracted into pure functions testable without live docker or 1Password.
