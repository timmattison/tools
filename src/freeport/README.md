# freeport

A Rust tool that finds a free TCP port on localhost (127.0.0.1) quickly and cross-platform.

## Description

This tool scans for available TCP ports on localhost and returns a free port. By default, it searches randomly through unprivileged ports (1024-65535) but can be configured to search privileged ports (1-1023), custom port ranges, or find the first available port sequentially.

## Usage

```
freeport [OPTIONS]
```

### Options

- `--allow-privileged`: Allow searching privileged ports (1-1023). Note: This may require elevated permissions.
- `--start-port <PORT>`: Start of port range to search (default: 1024, or 1 if --allow-privileged is used)
- `--end-port <PORT>`: End of port range to search (default: 65535)
- `--first-available`: Find the first available port instead of a random one
- `-h, --help`: Print help information
- `-V, --version`: Print version information

### Examples

```bash
# Find a random free unprivileged port (default behavior)
freeport

# Find the first available free unprivileged port
freeport --first-available

# Find a free port including privileged ports (may need root/admin)
freeport --allow-privileged

# Find a free port in a specific range
freeport --start-port 8000 --end-port 9000

# Find the first available port starting from a specific port
freeport --start-port 3000 --first-available
```

### Output

The tool outputs a single line containing a free port number found:

```bash
$ freeport
8743
$ freeport --first-available
1024
```

If no free port is found in the specified range, the tool exits with an error code and message.

## Behavior

- **Default (Random Search)**: The tool randomly shuffles the port range and tests ports in random order. This helps avoid conflicts when multiple instances run simultaneously.
- **Sequential Search (`--first-available`)**: The tool tests ports sequentially from the start of the range, returning the first available port found. This provides consistent, predictable results.

## Building

```bash
cargo build --release
```

The binary will be available at `target/release/freeport`.

## Cross-Platform Compatibility

This tool works on all platforms supported by Rust's standard library networking features:
- Linux
- macOS
- Windows
- Other Unix-like systems

## Use Cases

- Finding an available port for development servers
- Automated testing environments
- Service discovery and deployment scripts
- Network service configuration
