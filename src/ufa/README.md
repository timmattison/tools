# UFA - UniFi API CLI Tool

A command-line interface for interacting with UniFi Network applications via the Integration API,
and with cloud-hosted consoles via the UniFi Site Manager API.

## Features

- **Sites Management**: List and filter sites
- **Device Operations**: List devices, get details, view statistics (one device or all of them), restart devices, power cycle ports
- **Client Management**: List clients, get details, authorize/unauthorize guest access
- **Voucher Management**: Create, list, view, and delete hotspot vouchers with full filtering support
- **Cloud Consoles**: List the consoles on your Ubiquiti account and look one up by ID
- **Application Info**: Get UniFi application version and details
- **Interactive Setup**: Find your controller on the network or in 1Password and write a config file
- **1Password-backed Secrets**: Keep API keys in 1Password instead of on disk

## Installation

```bash
cargo install --git https://github.com/timmattison/tools ufa
```

Or build from source:

```bash
cd src/ufa
cargo build --release
```

Check what you have:

```bash
$ ufa --version
ufa 0.1.0 (26fea00, clean)
```

The version carries the git commit the binary was built from and whether that working tree had
uncommitted changes (`clean` / `dirty`), so a bug report identifies an exact build. A build made
outside a git checkout reports `(unknown, unknown)`.

## Command-line shape

One thing trips people up, so it is worth stating before the examples:

- **`--site-id` belongs to the command group.** It sits on `devices` / `clients` / `vouchers`,
  before the leaf subcommand: `ufa devices --site-id <SITE_ID> list`, not
  `ufa devices list --site-id <SITE_ID>`. It is optional — see
  [Choosing a site and a device](#choosing-a-site-and-a-device).

```
ufa [--url URL] [--api-key KEY] [--insecure BOOL] [--output json|table] <COMMAND>
```

`--url`, `--api-key`, `--insecure` and `--output` are global, so they may appear anywhere on the
line — before the subcommand or after it. `ufa devices list --output json` and
`ufa --output json devices list` are the same command, and every subcommand's `--help` lists them.

## Getting API keys

`ufa` talks to two different APIs, each with its own key.

**Controller (Integration) API key** — for `sites`, `devices`, `clients`, `vouchers`, `info`:

1. Log into your UniFi controller
2. Navigate to **Settings -> Control Plane -> Integrations**
3. Create a new API key
4. Copy the key and save it securely

**Site Manager API key** — for the `cloud` commands:

1. Go to [unifi.ui.com](https://unifi.ui.com) and sign in with your Ubiquiti account
2. Navigate to the API section in the left navigation bar
3. Generate a new API key and copy it (it is shown only once)

## Configuration

Every setting is resolved in the same order, first match wins:

1. **Command-line flag** — `--url`, `--api-key`, `--insecure`, and `--site-manager-api-key` on the
   `cloud` group.
2. **Environment variable** — `UNIFI_URL`, `UNIFI_API_KEY`, `UNIFI_INSECURE`,
   `UNIFI_SITE_MANAGER_API_KEY`. A `.env` file in the working directory is loaded before the
   arguments are parsed, so its values count as environment variables.
3. **Config file** — `config.toml` in your OS configuration directory.

```bash
# 1. flags
ufa --url https://192.168.1.1 --api-key YOUR_API_KEY sites

# 2. environment
export UNIFI_URL=https://192.168.1.1
export UNIFI_API_KEY=YOUR_API_KEY
ufa sites

# 2. environment, from a .env file in the working directory
cp .env.example .env   # then edit it
ufa sites

# 3. config file, written once by the setup wizard
ufa config setup
ufa sites
```

### The config file

```bash
# Print the path without needing the file to exist
ufa config path
```

| Platform | Path                                             |
| -------- | ------------------------------------------------ |
| macOS    | `~/Library/Application Support/ufa/config.toml`   |
| Linux    | `~/.config/ufa/config.toml`                      |
| Windows  | `%APPDATA%\ufa\config.toml`                      |

```toml
# Controller
url = "https://192.168.1.1"
insecure = true

# Controller API key, in 1Password (recommended)
op_path = "op://Private/ufa/key - 192.168.1.1 port 443"

# Site Manager (cloud) API key, in 1Password (recommended)
sm_op_path = "op://Private/ufa/site manager key"

# Legacy plaintext alternatives to the two references above. See the
# security caveat below before using either.
# api_key = "..."
# site_manager_api_key = "..."
```

On Unix the file is written with mode `0600` (owner read/write only) every time `ufa` saves it,
because it can hold a key in cleartext. Windows has no mode bits to tighten; the file inherits the
per-user ACL of `%APPDATA%`.

`ufa` never rewrites a field it did not ask about. `ufa config cloud` changes only the Site Manager
credential and leaves the controller URL, key and TLS choice exactly as they were.

### `ufa config setup`

An interactive wizard that finds a controller, verifies the key against it, and writes the config
file. It needs a terminal — a piped run is told to use `--url` and `--api-key` instead of hanging on
a question nobody can answer.

It looks for a controller in this order:

1. **1Password.** Fields of the `Private/ufa` item labelled `key - <host> port <port>` are offered
   as ready-made controllers. Picking one stores its `op://` reference — the key itself never
   reaches the config file.
2. **The local network.** mDNS discovery, then each candidate is probed for the Integration API's
   `info` endpoint. A host is only offered if it actually answers that endpoint, so a device that
   merely mentions UniFi on a web page is not mistaken for a controller.
3. **A URL you type**, validated the same way.

It then asks whether to skip TLS verification, tests the connection, and finally offers to store a
Site Manager key for the `cloud` commands. If the key ends up in the config file rather than in
1Password, setup says so and tells you how to move it.

### `ufa config cloud`

Sets up the Site Manager (cloud) credential on its own, for when the controller is already
configured. The prompt accepts an `op://` reference, the key itself, or an empty line to leave the
current setting alone.

## 1Password storage (recommended)

The key fields `op_path` (controller) and `sm_op_path` (cloud) hold a 1Password secret reference
rather than a secret. `ufa` reads them on demand through
[`op-cache`](../op-cache), so the key stays in your vault and never lands in a file, a shell
history, or a backup.

```toml
op_path    = "op://Private/ufa/key - 192.168.1.1 port 443"
sm_op_path = "op://Private/ufa/site manager key"
```

A reference wins over the matching plaintext field. If it is set but cannot be read — 1Password CLI
missing, biometric prompt declined, item renamed — `ufa` reports that failure rather than quietly
falling back to a stale plaintext copy, so a broken reference cannot hide:

```
Error: Failed to read the configured API key

Caused by:
    ...
```

## Commands

### Sites

```bash
# List sites
ufa sites

# Page through them
ufa sites --limit 50 --offset 50

# Filter
ufa sites --filter "name.like('main*')"
```

### Devices

```bash
# List devices (site chosen automatically, or asked about)
ufa devices list

# ...on a specific site
ufa devices --site-id <SITE_ID> list --limit 100

# Device details
ufa devices get <DEVICE_ID>

# Statistics for one device
ufa devices stats <DEVICE_ID>

# Statistics for one device, chosen from a prompt
ufa devices stats

# Statistics for every device on the site
ufa devices stats --all

# Restart a device
ufa devices restart <DEVICE_ID>

# Power cycle a PoE port
ufa devices power-cycle-port <DEVICE_ID> <PORT_IDX>
```

`ufa devices stats --all` walks every page of the site's device list, so it covers all of them and
not just the first page. The statistics requests run concurrently (at most 8 in flight, because the
controller answering them is often a home router) and the rows still come back in device order.

`--all` and a device ID are mutually exclusive and the contradiction is rejected before any request
is sent:

```
$ ufa devices stats --all 0f8b...
error: the argument '--all' cannot be used with '[DEVICE_ID]'
```

Two different blanks appear in an `--all` listing, and they mean different things:

| Cell    | Meaning                                                                         |
| ------- | ------------------------------------------------------------------------------- |
| `N/A`   | The device reported no value for that field.                                     |
| `ERROR` | Its statistics could not be fetched at all. The reason is printed to stderr.     |

One unreachable device therefore never hides the rest of the site. `--output json` is honoured for
`--all` as well as for a single device.

### Clients

```bash
# List connected clients
ufa clients list

# Only guests
ufa clients list --filter "access.type.eq('GUEST')"

# Client details
ufa clients get <CLIENT_ID>

# Authorize guest access
ufa clients authorize-guest <CLIENT_ID> \
  --time-limit-minutes 1440 \
  --data-usage-limit-mbytes 1024 \
  --rx-rate-limit-kbps 5000 \
  --tx-rate-limit-kbps 5000

# Revoke it
ufa clients unauthorize-guest <CLIENT_ID>
```

### Vouchers

```bash
# List vouchers (default limit 100)
ufa vouchers list

# Create vouchers
ufa vouchers create --count 10 --name "Conference 2024" --time-limit-minutes 1440

# Voucher details
ufa vouchers get <VOUCHER_ID>

# Delete one voucher
ufa vouchers delete <VOUCHER_ID>

# Delete every voucher a filter matches
ufa vouchers delete-filtered --filter "expired.eq(true)"
```

`delete-filtered` is the destructive one, because the controller — not `ufa` — decides what the
filter selects. It therefore lists the matches first, then asks:

```
$ ufa vouchers delete-filtered --filter "expired.eq(true)"
┌──────────┬─────────────┬ ...
│ ID       │ Name        │ ...
└──────────┴─────────────┴ ...
Delete 37 voucher(s)? [y/N]:
```

| Flag        | Effect                                                                            |
| ----------- | --------------------------------------------------------------------------------- |
| *(none)*    | Lists the matches and asks for confirmation. Anything but `y` aborts.              |
| `-y`, `--yes` | Deletes without asking. Intended for scripts.                                    |
| `--dry-run` | Lists the matches and stops. Nothing is deleted and nothing is asked.              |

A filter that matches nothing says so and asks nothing. With stdin not attached to a terminal and
no `--yes`, `ufa` refuses rather than guessing:

```
Delete 37 voucher(s)? needs confirmation, but stdin is not a terminal.
Re-run with --yes to confirm without being asked.
```

### Cloud consoles

The `cloud` command family talks to the UniFi Site Manager API and needs the Site Manager key, not
the controller key. It works without a controller URL.

```bash
# List every console on your Ubiquiti account
ufa cloud hosts

# JSON
ufa cloud hosts --output json

# Look one up, including its unifi.ui.com dashboard URL
ufa cloud host "70A741667C30...6289D202:1320847833"

# One-off key, without touching the config file
ufa cloud --site-manager-api-key YOUR_KEY hosts
```

Host IDs run to 60-odd characters, so the listing shows them cut short (on character boundaries) to
keep the table readable. `--output json` answers with the hosts exactly as the API reported them,
every field intact, rather than only the columns the table shows.

See [CLOUD.md](CLOUD.md) for console IDs, the full setup options, and example output, and
[USAGE_EXAMPLES.md](USAGE_EXAMPLES.md) for scripting recipes.

### Application info

```bash
ufa info
```

## Choosing a site and a device

Most commands need a site, and `--site-id` is optional because `ufa` will work it out:

- **One site** on the controller: it is used automatically.
- **Several sites**: they are listed as a table and you pick one.
- **No terminal to ask at** (a piped or scripted run) with several sites: `ufa` stops and tells you
  to pass `--site-id`, rather than guessing which site to act on.

`ufa devices stats` treats the device ID the same way: given none, a single device is used
automatically and several are offered as a prompt.

## Output formats

`--output` is a global option, so it is accepted at any position — `ufa devices list --output json`
and `ufa --output json devices list` are equivalent.

- `--output table` (default): human-readable tables drawn with box-drawing characters. Listings get
  one row per item; a single item — `devices get`, `clients get`, `vouchers get`, `info`,
  `cloud host` — gets a two-column `Field` / `Value` table with nested objects flattened into
  dotted keys (`features.switching.enabled`) and array elements indexed (`uplinks[0]`).
- `--output json`: machine-readable JSON, pretty-printed.

Listings answer in JSON with the controller's own pagination envelope rather than a bare array, so
nothing the API said is lost — the items are under `.data`:

```bash
$ ufa devices list --output json
{
  "offset": 0,
  "limit": 25,
  "count": 4,
  "totalCount": 4,
  "data": [ ... ]
}

$ ufa devices list --output json | jq '.data[] | select(.state == "OFFLINE")'
```

Two commands answer with a bare array instead, because neither is paginated: `ufa cloud hosts` and
`ufa devices stats --all`.

## Security

### API key storage

In descending order of preference:

1. **1Password reference in the config file** (`op_path` / `sm_op_path`) — recommended. The key
   stays in your vault, is fetched on demand, and never appears in a file on disk, in a backup, or
   in a process listing. Set it up with `ufa config setup` or `ufa config cloud`.
2. **Environment variable** (`UNIFI_API_KEY`, `UNIFI_SITE_MANAGER_API_KEY`) — reasonable for CI/CD,
   where the secret comes from the runner's secret store. Note that a process's environment is
   readable by other processes running as the same user.
3. **A `.env` file** in the working directory — convenient for local development. Never commit it;
   `.gitignore` here ignores `.env*` and makes an exception only for `.env.example`.
4. **Plaintext in the config file** (`api_key` / `site_manager_api_key`) — a legacy fallback, kept
   so existing configs keep working, and what setup falls back to when you paste a key that is not
   in 1Password. `ufa` restricts the file to mode `0600` on Unix, but the key is still cleartext:
   readable by anything running as you, by anyone with your backups, and by anyone who reads the
   file over your shoulder. Move it to `op_path` when you can.
5. **Command line** (`--api-key`) — avoid outside of one-off debugging. Arguments are visible in
   process listings and land in shell history.

Whichever you choose:

- API keys have full access to your UniFi controller — treat them like passwords
- Rotate them regularly
- Never commit a `.env` file or a config file containing a plaintext key

### TLS certificate verification

By default, TLS certificates are verified. A controller with a self-signed certificate fails with
an "UnknownIssuer" error. To connect anyway:

```bash
# Flag — note that it takes a value
ufa --insecure true sites

# Environment variable
export UNIFI_INSECURE=true
ufa sites

# .env file, or config.toml
UNIFI_INSECURE=true
```

```toml
insecure = true
```

Accepted spellings are `true`, `1`, `yes`, `on` and `false`, `0`, `no`, `off`. Anything else is
rejected outright rather than silently ignored:

```
$ UNIFI_INSECURE=maybe ufa sites
error: invalid value 'maybe' for '--insecure <INSECURE>': Invalid boolean value: maybe. Use true/false, 1/0, yes/no, or on/off
```

Only disable verification for trusted networks and controllers you own.

## Filtering

Many commands support filtering using UniFi's filter syntax. The expression is passed to the
controller, which evaluates it.

### Basic examples

```bash
# Equal comparison
--filter "name.eq('guest-network')"

# Pattern matching
--filter "name.like('guest*')"

# Numeric comparisons
--filter "timeLimitMinutes.gt(60)"

# Boolean values
--filter "expired.eq(true)"

# Date/time comparisons
--filter "createdAt.gt('2024-01-01')"
```

### Advanced examples

```bash
# Multiple conditions with AND
--filter "and(name.like('guest*'), expired.eq(false))"

# Multiple conditions with OR
--filter "or(expired.eq(true), timeLimitMinutes.lt(60))"

# Negation
--filter "not(name.like('admin*'))"
```

### Supported operators

- `eq`, `ne`: equals, not equals
- `gt`, `ge`, `lt`, `le`: greater than, greater/equal, less than, less/equal
- `like`: pattern matching (`*` for wildcards)
- `in`, `notIn`: value in list
- `isNull`, `isNotNull`: null checks
- `and`, `or`: logical operators
- `not`: negation

## API version

This tool implements the UniFi Network Integration API v9.2.87 — see [integration.json](integration.json)
for the OpenAPI specification it is written against. It may work with other versions but full
compatibility is not guaranteed. Values the API adds later (new device states, new client types)
are tolerated rather than failing the whole response.

Cloud console URLs (`https://unifi.ui.com/consoles/...`) cannot be used as `--url`; `ufa` says so
explicitly and points you at the local address of your controller.

## Error handling

The tool provides detailed error messages for:

- Network connectivity issues, including a specific hint when the failure is a TLS trust problem
- Authentication failures
- API errors with specific error codes; a long response body is quoted as an excerpt rather than
  flooding the terminal
- Invalid parameters or filters
- A configured API key that cannot be read, reported as the underlying cause rather than as
  "not provided"

## Examples

### Daily operations

```bash
# What version is the controller running?
ufa info

# List all sites
ufa sites

# Monitor devices on a specific site
ufa devices --site-id 12345678-1234-5678-9abc-123456789012 list

# Health of every device on the site, at a glance
ufa devices stats --all

# Create guest vouchers for an event
ufa vouchers create \
  --count 50 \
  --name "Conference Day 1" \
  --time-limit-minutes 480 \
  --data-usage-limit-mbytes 1024

# See what a cleanup would remove, then do it
ufa vouchers delete-filtered --filter "expired.eq(true)" --dry-run
ufa vouchers delete-filtered --filter "expired.eq(true)"
```

### Troubleshooting

```bash
# Find offline devices
ufa devices list --output json | jq '.data[] | select(.state == "OFFLINE")'

# Check one device's statistics
ufa devices stats <DEVICE_ID>

# List unauthorized guests
ufa clients list --filter "and(access.type.eq('GUEST'), access.authorized.eq(false))"

# Which console am I even looking at?
ufa cloud hosts
```

### Raw API access, for comparison

```bash
curl -k -X GET 'https://192.168.0.1/proxy/network/integration/v1/sites' \
 -H 'X-API-KEY: YOUR_API_KEY' \
 -H 'Accept: application/json'
```
