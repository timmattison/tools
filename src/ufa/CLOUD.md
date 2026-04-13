# UniFi Cloud Console Support

The `ufa` tool now supports discovering and working with UniFi cloud-hosted consoles through the UniFi Site Manager API.

## Understanding Console IDs

The hexadecimal string in UniFi cloud URLs (e.g., `70A741667C3000000000066DC7C00000000006BABC5A000000006289D202:1320847833`) is a unique Console/Host ID used by UniFi's Site Manager to identify specific UniFi consoles in the cloud.

URL format: `https://unifi.ui.com/consoles/[CONSOLE_ID]/network/default/dashboard`

## Setup

### Option 1: Interactive Setup
```bash
# Full setup including cloud credentials
ufa config setup

# Or just cloud credentials
ufa config cloud
```

### Option 2: Environment Variables
```bash
export UNIFI_SITE_MANAGER_API_KEY="your-site-manager-api-key"
```

### Option 3: Configuration File
Add to your config file at `~/.config/ufa/config.toml`:
```toml
site_manager_api_key = "your-site-manager-api-key"
```

## Getting Your Site Manager API Key

1. Go to [unifi.ui.com](https://unifi.ui.com)
2. Sign in with your Ubiquiti account
3. Navigate to the API section from the left navigation bar
4. Generate a new API key
5. Copy and store the key securely (it's only shown once)

## Cloud Commands

### List All Cloud Consoles
```bash
# List all your cloud-managed consoles
ufa cloud hosts

# With custom output format
ufa cloud hosts --output json
```

### Get Console Details
```bash
# Get details for a specific console
ufa cloud host "70A741667C3000000000066DC7C00000000006BABC5A000000006289D202:1320847833"
```

## Example Output

```bash
$ ufa cloud hosts

┌─────────────────────────────────┬────────────┬─────────────┬──────────┬──────────────┬────────┬─────────────────────┐
│ id                              │ name       │ model       │ firmware │ ip_address   │ type   │ owner │ last_seen           │
├─────────────────────────────────┼────────────┼─────────────┼──────────┼──────────────┼────────┼───────┼─────────────────────┤
│ 70A74166...D202:1320847833      │ Home-UDM   │ UDM-Pro     │ 3.2.9    │ 192.168.1.1  │ console│ true  │ 2024-01-15T10:30:00Z│
│ 900A6F00...9853:123456789       │ Office-UDR │ Dream Router│ 3.2.9    │ 192.168.2.1  │ console│ true  │ 2024-01-15T10:45:00Z│
└─────────────────────────────────┴────────────┴─────────────┴──────────┴──────────────┴────────┴───────┴─────────────────────┘

Total hosts: 2

To get details for a specific host, use: ufa cloud host <id>
```

## Integration with Regular Commands

Currently, the cloud console URLs cannot be used directly with regular `ufa` commands. You need to use the local IP address of your controller for API operations:

```bash
# This won't work yet:
ufa --url "https://unifi.ui.com/consoles/ABC123/..." devices list

# Use this instead:
ufa --url "https://192.168.1.1" devices list
```

Future versions may support automatic resolution of cloud console URLs to their local API endpoints.