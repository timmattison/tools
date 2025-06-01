# Terminal Image Display Utility

A fast Rust-based terminal image display utility for iTerm2, designed as a high-performance alternative to `imgcat`.

## Features

- Fast image loading and processing using the Rust `image` crate
- Support for multiple image formats (PNG, JPEG, GIF, WebP, BMP, TIFF, etc.)
- iTerm2 inline image protocol implementation
- Command-line interface with flexible options
- Image resizing with aspect ratio preservation
- Support for reading from files or stdin
- Optimized performance compared to the original `imgcat`

## Installation

```bash
cd ic
cargo build --release
```

The binary will be available at `target/release/ic`.

## Usage

### Display an image file:
```bash
./ic image.png
```

### Display an image with specific width:
```bash
./ic -w 80 image.jpg
```

### Display an image with specific dimensions and JPEG format:
```bash
./ic -w 80 --height 24 -f jpeg image.gif
```

### Display with custom JPEG quality:
```bash
./ic -f jpeg -q 75 image.png
```

### Read image from stdin:
```bash
cat image.png | ./ic --stdin
```

### Download and display an image:
```bash
curl -s https://example.com/image.jpg | ./ic --stdin
```

## Command Line Options

- `FILE` - Image file to display (optional if using --stdin)
- `-w, --width <WIDTH>` - Width in characters (defaults to auto-sizing)
- `--height <HEIGHT>` - Height in characters (defaults to auto-sizing)
- `--preserve-aspect` - Preserve aspect ratio when resizing (default: true)
- `--stdin` - Read from stdin instead of file
- `-n, --no-newline` - Don't output newline after image
- `-q, --quality <QUALITY>` - Image quality for JPEG output (1-100, default: 90)
- `-f, --format <FORMAT>` - Output format: 'auto', 'png', or 'jpeg' (default: 'auto')
- `-h, --help` - Print help information
- `-V, --version` - Print version information

## Supported Image Formats

- PNG
- JPEG/JPG
- GIF
- WebP
- BMP
- TIFF
- ICO
- And more through the Rust `image` crate

## Performance

This utility is designed to be significantly faster than the original `imgcat` by:
- Using efficient Rust image processing libraries
- Minimizing memory allocations with pre-allocated buffers
- Optimized base64 encoding
- Direct memory operations where possible
- Smart format selection (auto-chooses JPEG for opaque images, PNG for transparent)
- Configurable JPEG quality for size vs. quality trade-offs

## Compatibility

This utility is specifically designed for iTerm2 and uses iTerm2's inline image protocol. It may not work correctly with other terminal emulators.

### tmux Support

The utility automatically detects when running inside tmux and wraps the iTerm2 escape sequences in tmux's passthrough mechanism. This allows images to display correctly when using iTerm2 through tmux sessions.

## Technical Details

The utility works by:
1. Loading the image file using the `image` crate
2. Optionally resizing the image based on specified dimensions
3. Converting the image to the optimal format (PNG for transparency, JPEG for opaque images)
4. Base64 encoding the image data
5. Outputting the encoded data using iTerm2's escape sequence protocol

The format selection logic:
- `auto` (default): Chooses PNG for images with transparency, JPEG for opaque images
- `png`: Always outputs PNG format (preserves transparency)
- `jpeg`: Always outputs JPEG format (smaller file size, no transparency)

The iTerm2 protocol format used is:
```
\x1b]1337;File=inline=1;width=<width>px;height=<height>px:<base64_data>\x07
```