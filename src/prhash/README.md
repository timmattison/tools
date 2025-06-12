# prhash

Rust implementation of a tool to hash files with progress display.

## Features

- Support for multiple hash algorithms (md5, sha1, sha256, sha512, blake3)
- Progress bar with throughput display
- Pause/resume functionality
- Formatted number display

## Usage

```
prhash <hash type> <input file(s)>
```

Valid hash types:
- md5
- sha1
- sha256
- sha512
- blake3

## Examples

Hash a file with SHA-256:
```
prhash sha256 myfile.iso
```

Hash multiple files with Blake3:
```
prhash blake3 file1.zip file2.iso file3.bin
```

## Keyboard Controls

- **Space**: Pause/resume hashing
- **Ctrl+C**: Abort hashing

## Installation

```
cargo install --git https://github.com/timmattison/tools prhash
```