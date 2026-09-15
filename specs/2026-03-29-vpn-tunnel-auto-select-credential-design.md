# vpn-tunnel: Auto-Select Unused WireGuard Credential

**Issue:** #191
**Date:** 2026-03-29

## Problem

`vpn-tunnel generate` always uses a single credential field from 1Password. When multiple projects need concurrent VPN tunnels with the same 1Password item, the second tunnel disconnects the first because both use the same WireGuard key.

## Solution Overview

1. Add field enumeration to `op-cache` library so it can list all credential fields on an item
2. Modify `vpn-tunnel generate` to auto-select the first unused credential by comparing available fields against keys in running gluetun containers, and to keep the credential an already-generated directory names instead of selecting a new one, unless a running tunnel of another name holds its key
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
3. **Detect in-use keys:** Run `docker ps --format '{{.Names}}\t{{.Image}}'` and keep the rows whose image is `qmcgaw/gluetun` at any tag or digest, then `docker inspect <name>` to extract `WIREGUARD_PRIVATE_KEY` from each container's environment. The `--filter ancestor=` form cannot be used: docker resolves an untagged reference to `:latest`, so on a machine that only ever pulled the pinned tag the filter matches nothing while `docker ps` still exits 0
4. **Stop on a docker failure:** A `docker ps` or `docker inspect` that cannot be started or exits non-zero stops `generate`, reporting the docker stderr. A stopped daemon or a permission error must not read as "no tunnels are running", because that hands out a key a running tunnel already holds
5. **Reuse what the directory already names:** Read `CREDENTIAL_FIELD` from `<output_dir>/.env`. When a credential carries that exact label, and no running container of another name holds its key, select it and skip the match below. "Another name" is a name other than the one this run writes, `<container_prefix>-gluetun`, which the generated compose file pins with `container_name`. The credential stays when a running container of this run's name holds the key. Docker refuses two containers of one name, so a second run into the directory does not move its tunnel to another key. When a running container of another name holds the key, another tunnel already uses it. The match below then selects a free credential, and the output names that container. A label that no credential carries any more (the credential was removed from 1Password) falls through to the match below rather than failing
6. **Match and select:** Compare available credential values against in-use keys. Pick the first unused one
7. **Error if all in use:** Display which container is using each key:
   ```
   error: every WireGuard credential is held by a running tunnel

     credential    -> held by scraper-gluetun
     credential-2  -> held by vpn-gluetun

   Add another credential to "ProtonVPN WireGuard key" in 1Password,
   or stop an existing tunnel with: vpn-tunnel down --dir <path>
   ```
8. **Generate:** Pass selected key + field name to the generator

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
Using credential: credential-2 (3 available, 1 held by a running tunnel)
```

A run that kept the credential the directory already names says so on the next line, so a user who regenerates a directory is not left wondering why the counts did not move:
```
Reused the credential that the .env in ./vpn already names.
```

A run that did not keep that credential, because a running tunnel of another name holds its key, names that tunnel on the next line instead:
```
The .env in ./b names credential, but the running tunnel a-gluetun holds its key. This run selected credential-2.
```

### 4. Known Limitations

- **Two generate runs into two different directories:** Detection reads running containers, so a credential a generated directory holds is free until that directory's tunnel starts. Two `generate` runs into two *different* directories, neither of them started, therefore both select the same key. A later `generate` run into one of them repairs the duplicate when the tunnel of the other runs. Its `.env` names the credential, and a running container of another name holds the key. So `generate` selects a free credential and names that container. A later run while neither tunnel runs keeps the duplicate, because no running container holds the key. Two directories with the same container prefix can also carry one key. Each one writes the same container name, so a run into one sees the tunnel of the other as its own. Docker refuses to run two containers of one name, so those two tunnels never run at the same time. Closing the remaining gap needs a record of where past tunnels were generated, which the tool does not keep. The VPN provider rejects the duplicate, which makes it diagnosable.
- **Docker must be running** for in-use detection. A docker that is down, unreachable, or refuses the command is not read as "no containers are running": `generate` stops and reports what docker said, because a credential chosen on unknown state can duplicate a live tunnel's key.

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
- A label an existing `.env` names -> selects that credential over the first free one
- A label an existing `.env` names, whose key only a running container of this run's name holds -> still selects that credential, and never reaches the all-in-use error
- A label an existing `.env` names, whose key a running container of another name holds -> selects the first free credential and names that container, or returns the all-in-use error when no credential is free
- A label whose key a container of this run's name and a container of another name both hold -> the container of another name decides, so the first free credential is selected
- A label no credential carries any more -> falls back to the first free credential
- A label matches a whole label, never a prefix of one (`credential-2` does not select `credential-20`)
- Credential field name stored correctly in .env output
- The compose file pins the container name that credential selection compares
- The note after `Using credential:` names a reused credential, or names the `.env` directory, the named label, the holder, and the selected label
- Every gluetun tag (pinned, `latest`, bare, digest) is detected; `evil/qmcgaw/gluetun` and `qmcgaw/gluetunnel` are not
- A failed `docker ps`, a failed `docker inspect`, and a docker that cannot be started each return an error, never an empty list

The matching logic (available keys vs in-use keys -> selection) is extracted into pure functions testable without live docker or 1Password. Detection takes an injected command runner for the same reason, so the docker failure paths are tested without a docker daemon.
