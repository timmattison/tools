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
pub struct ItemField {
    pub label: String,
    pub value: String,
}

pub fn read_item_fields(
    &self,
    op_path: &OpPath,
    field_prefix: &str,
) -> Result<Vec<ItemField>>
```

**Behavior:**
- Runs `op item get --vault <vault> --format json -- <item>` through the 1Password CLI. The run goes through `OpRun`, the one entrance to `op`, as a read does. So each attempt writes an `op_start` and an `op_end` record with the operation `item-get` to the audit trail, and shows the notification that names the caller. The item comes after `--`, so op never reads a title that starts with `-` as a flag
- Parses the JSON response and keeps the fields whose `label` equals `field_prefix` or starts with `field_prefix-`
- Returns a `Vec<ItemField>` sorted alphabetically by label (`credential` before `credential-2`, etc.)
- Caches only the matching fields in `.op-cache.json`, under the synthetic key `<item path>/__item_fields__/<field_prefix>`, for example `op://Private/ProtonVPN WireGuard key/__item_fields__/credential`. The cached value is the serialized list of `(label, value)` pairs. The entry has no expiry. `op-cache invalidate <item path>` removes it, with each other field list of that item, so that the next call reads a credential added in 1Password
- Uses the same retry logic as `read()`: 3 attempts, with a linear wait of `RETRY_DELAY_MS * attempt` (1 s, then 2 s) between them. Output that is not JSON is an error at once, with no further attempt
- Returns an error if no fields match the prefix
- The `op-cache list-fields <item path> [--prefix credential]` subcommand prints the labels of the matching fields, and never their values

**OpPath changes:**
- `--op-path` now expects an item-level path: `op://vault/item` (no trailing field segment)
- The default changes from `op://Private/ProtonVPN WireGuard key/credential` to `op://Private/ProtonVPN WireGuard key`
- This is a breaking change for anyone who customized `--op-path` with a field-level path

### 2. vpn-tunnel Credential Auto-Selection

The `generate` command flow becomes:

1. **Enumerate credentials:** Call `op_cache.read_item_fields(&op_path, "credential")` to get all matching fields
2. **Error if none found:** Clear error about expected 1Password item structure
3. **Detect in-use keys:** Run `docker ps -q --no-trunc` to get the ID of every running container. With no ID, stop here and run no `docker inspect`, because `docker inspect` with no target fails. Otherwise run one `docker inspect --format '{{json .Name}}\t{{json .Config.Env}}'` over all IDs. Docker prints one line per container, and JSON escapes a tab or a newline inside a value, so the tab is a safe separator. A container whose environment carries `WIREGUARD_PRIVATE_KEY` holds that key, whatever its image. The image is not read: after a pull moves a tag, `docker ps` shows the image ID in place of `qmcgaw/gluetun:<tag>`, and a mirror such as `ghcr.io/qdm12/gluetun` has another name, so an image filter hides a running tunnel and hands its key to a second one. A line that cannot be parsed is an error, and the error names the line number and never the line, because the line holds environment values
4. **Stop on a docker failure:** A `docker ps` or `docker inspect` that cannot be started or exits non-zero stops `generate`, reporting the docker stderr. A stopped daemon or a permission error must not read as "no tunnels are running", because that hands out a key a running tunnel already holds. One non-zero exit is not a failure: a `docker inspect` whose stderr holds only `no such object: <id>` lines, each for an ID that `docker ps` listed. Such a container was removed between the two calls, so it holds no key, and docker still prints the other containers
5. **Reuse what the directory already names:** Read `CREDENTIAL_FIELD` from `<output_dir>/.env`. When a credential carries that exact label, and no running container of another name holds its key, select it and skip the match below. "Another name" is a name other than the one this run writes, `<container_prefix>-gluetun`, which the generated compose file pins with `container_name`. The credential stays when a running container of this run's name holds the key. Docker refuses two containers of one name, so a second run into the directory does not move its tunnel to another key. When a running container of another name holds the key, another tunnel already uses it. The match below then selects a free credential, and the output names that container. A label that no credential carries any more (the credential was removed from 1Password) falls through to the match below rather than failing. **No label names a credential** when the `.env` has no `CREDENTIAL_FIELD` line (a directory that was generated before this line existed) or when no credential carries the label. Then a credential whose key a running container of this run's name holds, and no running container of another name holds, is selected and the match below is skipped. This is the same Docker-name argument. The tunnel of the directory does not move to another key, and a directory with one credential does not fail with the all-in-use error
6. **Match and select:** Compare available credential values against in-use keys. Pick the first unused one
7. **Error if all in use:** Display which container is using each key:
   ```
   error: every WireGuard credential is held by a running tunnel

     credential           -> held by scraper-gluetun
     credential-2         -> held by vpn-gluetun

   Add another credential to the item in 1Password, then run
     op-cache invalidate 'op://Private/ProtonVPN WireGuard key'
   and run vpn-tunnel generate again. op-cache keeps the old list of credentials until then.
   Or stop an existing tunnel with: vpn-tunnel down --dir <path>
   ```
8. **Generate:** Pass selected key + field name to the generator

### 3. .env and Status Changes

**.env file** gains a new variable, after a comment that says which commands read it:
```
WIREGUARD_PRIVATE_KEY=<selected key>
# Used by vpn-tunnel status and generate (not a Docker variable)
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

A run that kept the key of its own running tunnel, because the `.env` names no credential, names that tunnel on the next line:
```
Kept credential-2, because the running tunnel vpn-gluetun already holds its key.
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
- A label no credential carries any more -> falls back to the first free credential, unless the own tunnel holds a key (next line)
- No label, or a label no credential carries, and a running container of this run's name holds the key of a credential that no container of another name holds -> selects that credential and names the container, also when it is the only credential
- No label, and a container of this run's name and a container of another name both hold a key -> the first free credential is selected
- A label matches a whole label, never a prefix of one (`credential-2` does not select `credential-20`)
- Credential field name stored correctly in .env output
- The compose file pins the container name that credential selection compares
- The note after `Using credential:` names a reused credential, or names the `.env` directory, the named label, the holder, and the selected label
- A container is found by its `WIREGUARD_PRIVATE_KEY`, whatever its image: one whose `docker ps` row shows a bare image ID, and one from a GHCR mirror, are found. A container without the variable, or with no environment, is skipped
- No running container means no `docker inspect`; otherwise one `docker inspect` reads every ID
- A line of the `docker inspect` answer that cannot be parsed is an error that names the line and does not show the key
- A failed `docker ps -q`, a failed `docker inspect`, and a docker that cannot be started each return an error, never an empty list
- A container removed between `docker ps` and `docker inspect` is skipped, for the lower-case and the capitalized error text. A second error line, or the error for an ID that `docker ps` did not list, stays an error

The matching logic (available keys vs in-use keys -> selection) is extracted into pure functions testable without live docker or 1Password. Detection takes an injected command runner for the same reason, so the docker failure paths are tested without a docker daemon.
