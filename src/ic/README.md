# Terminal Image Display Utility

A fast Rust-based terminal image display utility

## Features

- Fast image loading and processing using the Rust `image` crate
- Support for multiple image formats (PNG, JPEG, GIF, WebP, BMP, TIFF, etc.)
- iTerm2 inline image protocol implementation
- Image resizing with aspect ratio preservation
- Support for reading from files or stdin
- Video playback support (experimental)
- Kitty terminal support (experimental)

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

### Play a video file:
```bash
./ic video.mp4
```

When the video is playing you can press `q` or `ESC` or `Ctrl+C` to stop playback.
Press space to pause and resume playback.
Press left arrow to go back one frame, right arrow to go forward one frame.
Press up arrow to go forward 10 seconds, down arrow to go back 10 seconds.
Press `a` to go back 1 second, `d` to go forward 1 second.
Press `w` to go back 1 minute, `s` to go forward 1 minute.

### Display an image with specific dimensions:
```bash
./ic -w 80 --height 24 image.gif
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
- `-h, --help` - Print help information
- `-V, --version` - Print version information

## Compatibility

This utility uses iTerm2's inline image protocol or the Kitty image protocol. It may not work correctly with other terminal emulators.

Kitty support is experimental.

### tmux Support

The utility automatically detects when running inside tmux and reports an error. This program does not work in tmux (yet).