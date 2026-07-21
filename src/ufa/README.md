# UFA - UniFi API CLI Tool

A command-line interface for interacting with UniFi Network applications via the Integration API.

## Features

- **Sites Management**: List and filter sites
- **Device Operations**: List devices, get details, view statistics, restart devices, power cycle ports
- **Client Management**: List clients, get details, authorize/unauthorize guest access
- **Voucher Management**: Create, list, view, and delete hotspot vouchers with full filtering support
- **Application Info**: Get UniFi application version and details

## Installation

```bash
cargo install --git https://github.com/timmattison/tools ufa
```

Or build from source:

```bash
cd src/ufa
cargo build --release
```

## Usage

### API Key Generation

Before using this tool, you need to generate an API key:

1. Log into your UniFi controller
2. Navigate to **Settings -> Control Plane -> Integrations**
3. Create a new API key
4. Copy the key and save it securely

### Configuration

You can provide credentials in three ways (in order of precedence):

1. **Command line arguments**:
```bash
ufa --url https://192.168.1.1 --api-key YOUR_API_KEY <COMMAND>
```

2. **Environment variables**:
```bash
export UNIFI_URL=https://192.168.1.1
export UNIFI_API_KEY=YOUR_API_KEY
ufa <COMMAND>
```

3. **`.env` file** (recommended for security):
```bash
# Create a .env file in your working directory
cp .env.example .env
# Edit .env with your credentials
ufa <COMMAND>
```

Example `.env` file:
```env
UNIFI_URL=https://192.168.1.1
UNIFI_API_KEY=your_api_key_here
```

### Commands

#### Sites
```bash
# List all sites
ufa sites --limit 50

# List sites with filtering
ufa sites --filter "name.like('main*')"
```

#### Devices
```bash
# List devices on a site
ufa devices list <SITE_ID>

# Get device details
ufa devices get <SITE_ID> <DEVICE_ID>

# Get device statistics
ufa devices stats <SITE_ID> <DEVICE_ID>

# Restart a device
ufa devices restart <SITE_ID> <DEVICE_ID>

# Power cycle a port
ufa devices power-cycle-port <SITE_ID> <DEVICE_ID> <PORT_INDEX>
```

#### Clients
```bash
# List connected clients
ufa clients list <SITE_ID>

# List only guest clients
ufa clients list <SITE_ID> --filter "access.type.eq('GUEST')"

# Get client details
ufa clients get <SITE_ID> <CLIENT_ID>

# Authorize guest access
ufa clients authorize-guest <SITE_ID> <CLIENT_ID> --time-limit-minutes 1440

# Unauthorize guest access
ufa clients unauthorize-guest <SITE_ID> <CLIENT_ID>
```

#### Vouchers
```bash
# List vouchers
ufa vouchers list <SITE_ID>

# Create vouchers
ufa vouchers create <SITE_ID> --count 10 --name "Conference 2024" --time-limit-minutes 1440

# Get voucher details
ufa vouchers get <SITE_ID> <VOUCHER_ID>

# Delete specific voucher
ufa vouchers delete <SITE_ID> <VOUCHER_ID>

# Delete expired vouchers
ufa vouchers delete-filtered <SITE_ID> --filter "expired.eq(true)"
```

#### Application Info
```bash
# Get application information
ufa info
```

### Output Formats

- `--output table` (default): Human-readable table format
- `--output json`: Machine-readable JSON format

### Security

#### API Key Storage
- **`.env` file**: Recommended for local development. Make sure to add `.env` to your `.gitignore`.
- **Environment variables**: Good for CI/CD and production environments.
- **Command line**: Avoid in production as API keys may be visible in process lists.

#### TLS Certificate Verification
By default, TLS certificates are verified. If your UniFi controller uses a self-signed certificate, you'll see an error like "UnknownIssuer". To connect:

**Option 1: Command line flag**
```bash
ufa --insecure sites
```

**Option 2: Environment variable**
```bash
export UNIFI_INSECURE=true
ufa sites
```

**Option 3: .env file**
```env
UNIFI_INSECURE=true
```

The UNIFI_INSECURE variable accepts: `true`, `1`, `yes`, `on` (or `false`, `0`, `no`, `off`).

**Important**: 
- Never commit your `.env` file or expose API keys in command history
- API keys have full access to your UniFi controller - keep them secure
- Rotate API keys regularly for better security
- Only disable certificate verification for trusted networks and controllers you own

### Filtering

Many commands support filtering using UniFi's filter syntax:

#### Basic Examples
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

#### Advanced Examples
```bash
# Multiple conditions with AND
--filter "and(name.like('guest*'), expired.eq(false))"

# Multiple conditions with OR
--filter "or(expired.eq(true), timeLimitMinutes.lt(60))"

# Negation
--filter "not(name.like('admin*'))"
```

#### Supported Operators
- `eq`, `ne`: equals, not equals
- `gt`, `ge`, `lt`, `le`: greater than, greater/equal, less than, less/equal
- `like`: pattern matching (* for wildcards)
- `in`, `notIn`: value in list
- `isNull`, `isNotNull`: null checks
- `and`, `or`: logical operators
- `not`: negation

## API Version

This tool implements the UniFi Network Integration API v9.2.87. It may work with other versions but full compatibility is not guaranteed.

## Error Handling

The tool provides detailed error messages for:
- Network connectivity issues
- Authentication failures
- API errors with specific error codes
- Invalid parameters or filters

## Examples

### Daily Operations

```bash
# Example using the UniFi Integration API (like curl)
curl -k -X GET 'https://192.168.0.1/proxy/network/integration/v1/sites' \
 -H 'X-API-KEY: YOUR_API_KEY' \
 -H 'Accept: application/json'

# Using ufa CLI
ufa info

# List all sites
ufa sites

# Monitor devices on main site
ufa devices list 12345678-1234-5678-9abc-123456789012

# Create guest vouchers for an event
ufa vouchers create 12345678-1234-5678-9abc-123456789012 \
  --count 50 \
  --name "Conference Day 1" \
  --time-limit-minutes 480 \
  --data-usage-limit-mbytes 1024

# Clean up expired vouchers
ufa vouchers delete-filtered 12345678-1234-5678-9abc-123456789012 \
  --filter "expired.eq(true)"
```

### Troubleshooting

```bash
# Find offline devices
ufa devices list <SITE_ID> --output json | jq '.data[] | select(.state == "OFFLINE")'

# Check device statistics
ufa devices stats <SITE_ID> <DEVICE_ID>

# List unauthorized guests
ufa clients list <SITE_ID> --filter "and(access.type.eq('GUEST'), access.authorized.eq(false))"
```