# Clipboard Random

A Rust command-line utility that generates random data or text with diacritics (Zalgo text) and copies it to the clipboard/paste buffer.

## Features

### Binary Data Generation
- Generate a user-defined number of bytes of random data
- Multiple output formats: hexadecimal, base64, or raw bytes
- Cross-platform clipboard support (works on macOS, Linux, and Windows)
- Raw binary data is copied as actual binary to clipboard (not hex-encoded)

### Text Generation with Diacritics (Zalgo Text)
- Generate random text with Unicode combining diacritical marks
- Configurable probability for diacritics per character
- Adjustable number of diacritics per character (min/max range)
- Control word spacing with configurable word lengths
- Built-in presets: mild, scary, insane, zalgo, doom

### General Features
- Dry run mode for testing without clipboard access
- Data preview for verification
- Comprehensive input validation and error handling

## Prerequisites

- Rust (for building from source)
- A graphical environment for clipboard access (not required for dry-run mode)

## Building

```bash
cd clipboard-random
cargo build --release
```

The executable will be available at `target/release/clipboard-random`

## Usage

```
clipboard-random [OPTIONS] <COMMAND>
```

### Commands

#### Binary Data Generation
```
clipboard-random binary [OPTIONS] <BYTES>
```

**Arguments:**
- `<BYTES>` - Number of bytes of random data to generate (must be greater than 0)

**Options:**
- `-f, --format <FORMAT>` - Output format for the random data (default: hex)
  - `hex` - Hexadecimal representation (e.g., "a1b2c3")
  - `base64` - Base64 encoding
  - `raw` - Raw bytes as binary data

#### Text Generation with Diacritics
```
clipboard-random text [OPTIONS] <CHARS>
```

**Arguments:**
- `<CHARS>` - Number of characters of text to generate (must be greater than 0)

**Options:**
- `-p, --probability <PROBABILITY>` - Probability (0.0-1.0) that each character will have diacritics (default: 0.5)
- `--min-diacritics <MIN_DIACRITICS>` - Minimum number of diacritics per character (default: 1)
- `--max-diacritics <MAX_DIACRITICS>` - Maximum number of diacritics per character (default: 3)
- `--min-word-length <MIN_WORD_LENGTH>` - Minimum number of characters between spaces (default: 3)
- `--max-word-length <MAX_WORD_LENGTH>` - Maximum number of characters between spaces (default: 8)
- `--preset <PRESET>` - Use a preset configuration:
  - `mild` - Mild diacritics effect (30% probability, 1-2 diacritics)
  - `scary` - Moderate diacritics effect (60% probability, 1-4 diacritics)
  - `insane` - Heavy diacritics effect (80% probability, 2-6 diacritics)
  - `zalgo` - Extreme diacritics effect (90% probability, 3-8 diacritics)
  - `doom` - Apocalyptic diacritics effect (100% probability, 5-12 diacritics)

### Global Options

- `-d, --dry-run` - Generate and display data without copying to clipboard
- `-h, --help` - Print help information
- `-V, --version` - Print version information

## Examples

### Binary Data Examples

Generate 16 bytes of random data in hexadecimal format and copy to clipboard:
```bash
./clipboard-random binary 16
```

Generate 32 bytes in base64 format:
```bash
./clipboard-random binary --format base64 32
```

Generate raw binary data:
```bash
./clipboard-random binary --format raw 12
```

Test without copying to clipboard (useful in headless environments):
```bash
./clipboard-random --dry-run binary 8
```

### Text Generation Examples

Generate 50 characters of text with default settings:
```bash
./clipboard-random text 50
```

Generate text with custom diacritics settings:
```bash
./clipboard-random text --probability 0.8 --min-diacritics 2 --max-diacritics 5 30
```

Use presets for different effects:
```bash
# Mild effect
./clipboard-random text --preset mild 40

# Classic Zalgo text
./clipboard-random text --preset zalgo 25

# Maximum chaos
./clipboard-random text --preset doom 20
```

Test text generation without clipboard:
```bash
./clipboard-random --dry-run text --preset insane 30
```

## Cross-Platform Support

This utility is designed to work on:

- **macOS** - Primary target platform
- **Linux** - Requires X11 or Wayland display server
- **Windows** - Should work out of the box

## Note

The clipboard functionality requires a graphical environment. If you're running in a headless environment (like CI/CD), use the `--dry-run` flag to test the functionality without clipboard access.