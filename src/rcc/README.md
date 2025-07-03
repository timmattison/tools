# rcc - Rust Cross Compiler

A command-line tool that simplifies Rust cross-compilation by automating target detection, configuration management, and build execution. Say goodbye to memorizing target triples and Docker image names!

## Installation

```bash
cargo install --git https://github.com/timmattison/tools rcc
```

## Quick Start

```bash
# Let rcc figure out the target from a remote host
rcc --uname "$(ssh remote-host uname -a)"

# Or specify the target directly
rcc --target aarch64-unknown-linux-gnu

# Build in release mode
rcc --release
```

## Features

- **Automatic target detection** from `uname -a` output
- **Cross.toml management** - creates and manages configuration automatically
- **libc detection** - distinguishes between `gnu` and `musl` targets
- **Build execution** - runs the appropriate `cross build` command
- **Multi-target support** - handles projects with multiple targets

## How It Works

### 1. Target Detection

When you provide `--uname` with the output from `uname -a`, rcc automatically determines:
- **Architecture**: aarch64, x86_64, armv7l, or i686
- **libc**: gnu (default) or musl (for Alpine Linux)
- **Target triple**: Combines architecture and libc into proper target format

**Architecture Mapping:**
- `aarch64` → `aarch64-unknown-linux-{gnu|musl}`
- `x86_64` → `x86_64-unknown-linux-{gnu|musl}`
- `armv7l` → `armv7-unknown-linux-{gnu|musl}eabihf`
- `i686` → `i686-unknown-linux-{gnu|musl}`

**libc Detection:**
- Contains "alpine" → `musl`
- Everything else → `gnu`

### 2. Cross.toml Management

If `Cross.toml` doesn't exist, rcc creates it:

```toml
[target.aarch64-unknown-linux-gnu]
image = "ghcr.io/cross-rs/aarch64-unknown-linux-gnu:edge"
```

If `Cross.toml` exists:
- **Single target**: Uses it automatically
- **Multiple targets**: Lists them and requires `--target` selection

### 3. Build Execution

Runs the appropriate cross command:
```bash
cross build --target <detected-target> [--release]
```

## Usage Examples

### Cross-compile for Raspberry Pi

```bash
# Get uname from Pi
ssh pi@raspberrypi.local uname -a
# "Linux raspberrypi 5.15.84-v8+ #1621 SMP PREEMPT Thu Jan 12 13:05:08 GMT 2023 aarch64 GNU/Linux"

# Let rcc handle everything
rcc --uname "Linux raspberrypi 5.15.84-v8+ #1621 SMP PREEMPT Thu Jan 12 13:05:08 GMT 2023 aarch64 GNU/Linux"

# rcc automatically:
# 1. Detects target: aarch64-unknown-linux-gnu
# 2. Creates Cross.toml with correct Docker image
# 3. Runs: cross build --target aarch64-unknown-linux-gnu
```

### Cross-compile for Alpine Linux

```bash
# Alpine server uname
rcc --uname "Linux alpine-server 5.15.74-0-lts #1-Alpine SMP Wed Sep 7 07:17:17 UTC 2022 x86_64 Linux"

# rcc detects:
# - Architecture: x86_64
# - OS: Alpine (musl)
# - Target: x86_64-unknown-linux-musl
```

### Direct target specification

```bash
# If you know the target
rcc --target aarch64-unknown-linux-gnu

# With release build
rcc --target aarch64-unknown-linux-gnu --release
```

### Working with existing Cross.toml

```bash
# Single target - runs automatically
rcc

# Multiple targets - shows options
rcc
# Output:
# Multiple targets found in Cross.toml:
#   - aarch64-unknown-linux-gnu
#   - x86_64-unknown-linux-musl
# Error: Please specify which target to use with --target <target>

# Select specific target
rcc --target aarch64-unknown-linux-gnu
```

## Command Line Options

```
USAGE:
    rcc [OPTIONS]

OPTIONS:
        --uname <UNAME>    Parse uname string to determine target architecture
        --target <TARGET>  Specify the target triple directly
        --release          Build in release mode
    -h, --help             Print help
    -V, --version          Print version
```

## Prerequisites

rcc requires the `cross` tool to be installed. If it's missing, rcc will show:

```
Error: cross is not installed. Please install it by running:
cargo install cross --git https://github.com/cross-rs/cross
```

## Real-World Workflow

### Scenario: Deploy to multiple servers

```bash
# Production ARM64 server
rcc --uname "$(ssh prod-arm uname -a)" --release

# Staging x86_64 Alpine
rcc --uname "$(ssh staging-alpine uname -a)" --release

# Development Raspberry Pi
rcc --uname "$(ssh dev-pi uname -a)"
```

### Scenario: CI/CD Pipeline

```bash
# In your CI script
UNAME_OUTPUT=$(ssh $DEPLOY_HOST uname -a)
rcc --uname "$UNAME_OUTPUT" --release
```

## Error Handling

rcc provides clear error messages for common issues:

- **Missing cross**: Instructions to install cross
- **Invalid uname**: Can't parse architecture from uname string
- **No target specified**: Need --target or --uname when no Cross.toml exists
- **Multiple targets**: Lists available targets when Cross.toml has multiple entries
- **Build failures**: Reports cross build errors

## Development

To build rcc from source:

```bash
git clone https://github.com/timmattison/tools.git
cd tools/src/rcc
cargo build --release
```

## Contributing

Issues and pull requests welcome at: https://github.com/timmattison/tools

## License

This project follows the same license as the main tools repository.