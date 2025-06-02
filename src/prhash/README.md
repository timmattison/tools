# prhash (Rust)

A progress hash calculator with a terminal user interface (TUI) using Ratatui. This is a Rust port of the original Go version.

## Features

- **Multiple hash algorithms**: MD5, SHA1, SHA256, SHA512, Blake3
- **Progress tracking**: Visual progress bar and throughput display
- **Pause/Resume**: Press space to pause/resume hashing
- **Multiple files**: Process multiple files sequentially
- **TUI mode**: Interactive terminal interface when TTY is available
- **Simple mode**: Non-interactive mode for scripts/automation

## Usage

```bash
# Hash a single file
prhash sha256 file.txt

# Hash multiple files
prhash md5 file1.txt file2.txt file3.txt

# Use different hash algorithms
prhash blake3 large_file.iso
```

## Supported Hash Types

- `md5`
- `sha1` 
- `sha256`
- `sha512`
- `blake3`

## Interactive Controls (TUI mode)

- **Space**: Pause/Resume hashing
- **Ctrl+C**: Abort and exit

## Output Format

The output format matches standard hash tools:
```
<hash_value>  <filename>
```

## Building

```bash
cargo build --release
```

## Installation

```bash
cargo install --path .
```