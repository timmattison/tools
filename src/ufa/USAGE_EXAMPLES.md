# UniFi Cloud Console ID Discovery - Usage Examples

## Setup

First, configure your Site Manager API key:

```bash
# Interactive setup
ufa config setup

# Or just cloud credentials
ufa config cloud

# Or use environment variable
export UNIFI_SITE_MANAGER_API_KEY="your-api-key"
```

## Discovering Console IDs

### List All Your Cloud Consoles

```bash
# Table format (default)
ufa cloud hosts

# JSON format for programmatic use
ufa cloud hosts --output json
```

Example output:
```
ID                                                                Name           Model         Firmware  IP Address      Type     Owner  Last Seen           
------------------------------------------------------------------------------------------------------------------------------------------------------
70A741667C3000000000066DC7C00000000006BABC5A000000006289D202:...  Home-UDM      UDM-Pro       3.2.9     192.168.1.1     console  true   2024-01-15 10:30:00 
900A6F00301100000000074A6BA90000000007A3387E0000000063EC9853:...  Office-UDR    Dream Router  3.2.9     192.168.2.1     console  true   2024-01-15 10:45:00 

Total hosts: 2

To get details for a specific host, use: ufa cloud host <id>
```

### Get Specific Console Details

```bash
# Get detailed information about a specific console
ufa cloud host "70A741667C3000000000066DC7C00000000006BABC5A000000006289D202:1320847833"

# JSON output
ufa cloud host "70A741667C3000000000066DC7C00000000006BABC5A000000006289D202:1320847833" --output json
```

## Understanding the Console ID

The console ID format: `70A741667C3000000000066DC7C00000000006BABC5A000000006289D202:1320847833`

- **First part**: 64-character hexadecimal string (256 bits) - unique console identifier
- **Separator**: Colon (:)
- **Second part**: Numeric value (possibly timestamp or sequence number)

This ID is used in UniFi cloud URLs:
```
https://unifi.ui.com/consoles/[CONSOLE_ID]/network/default/dashboard
```

## Programmatic Usage

### Using with jq

```bash
# Get all console IDs
ufa cloud hosts --output json | jq -r '.[] | .id'

# Get console ID by name
ufa cloud hosts --output json | jq -r '.[] | select(.name=="Home-UDM") | .id'

# Get IP addresses of all consoles
ufa cloud hosts --output json | jq -r '.[] | "\(.name): \(.ip_address)"'
```

### Shell Script Example

```bash
#!/bin/bash

# Get console ID for a specific console name
CONSOLE_NAME="Home-UDM"
CONSOLE_ID=$(ufa cloud hosts --output json | jq -r ".[] | select(.name==\"$CONSOLE_NAME\") | .id")

if [ -n "$CONSOLE_ID" ]; then
    echo "Console ID for $CONSOLE_NAME: $CONSOLE_ID"
    echo "Cloud URL: https://unifi.ui.com/consoles/$CONSOLE_ID/network/default/dashboard"
else
    echo "Console not found: $CONSOLE_NAME"
fi
```

## Error Handling

If you try to use a cloud console URL directly:
```bash
# This will show an error message
ufa --url "https://unifi.ui.com/consoles/ABC123/..." devices list

# Error: Cloud console URLs are not yet supported for direct API access.
# To work with cloud-hosted consoles, use the 'ufa cloud' commands to discover console IDs.
# For direct API access, use the local IP address of your UniFi controller.
```

## Integration with Other Tools

Once you have the console ID, you can:

1. **Build cloud URLs programmatically**:
   ```bash
   CONSOLE_ID="70A741667C3000000000066DC7C00000000006BABC5A000000006289D202:1320847833"
   echo "https://unifi.ui.com/consoles/$CONSOLE_ID/network/default/dashboard"
   ```

2. **Use with browser automation**:
   ```bash
   # Open console in default browser (macOS)
   open "https://unifi.ui.com/consoles/$CONSOLE_ID/network/default/dashboard"
   ```

3. **Create bookmarks or shortcuts** for frequently accessed consoles

## API Key Management

Get your Site Manager API key from:
1. Log in to [unifi.ui.com](https://unifi.ui.com)
2. Navigate to the API section (left sidebar)
3. Generate a new API key
4. Copy and save it securely (shown only once)