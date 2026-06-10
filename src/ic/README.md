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
- `--protocol <auto|sixel|kitty|iterm2>` - Force a specific image protocol instead of auto-detecting from the terminal (default: `auto`). Also settable via the `IC_PROTOCOL` environment variable.
- `-h, --help` - Print help information
- `-V, --version` - Print version information

## Environment Variables

- `IC_PROTOCOL` - Same values as `--protocol` (`auto`, `sixel`, `kitty`, `iterm2`). The `--protocol` flag wins if both are set. Handy for pinning the protocol for a whole shell session — e.g. `export IC_PROTOCOL=sixel` in a web-terminal session.

## Compatibility

This utility uses iTerm2's inline image protocol or the Kitty image protocol. It may not work correctly with other terminal emulators.

Kitty support is experimental.

### Forcing a Protocol

By default `ic` auto-detects the protocol from the terminal (Zellij → Sixel; Kitty/Ghostty/WezTerm → Kitty; otherwise → iTerm2). When auto-detection guesses wrong, force it with `--protocol` (or `IC_PROTOCOL`).

Forcing a protocol **also bypasses the remote-transport (Mosh) check**. That check walks the process tree for a `mosh-server` ancestor, and under a multiplexer such as Zellij it can misattribute *another* client's Mosh session to yours, producing a false "Mosh detected" error even when your session isn't on Mosh. Forcing a protocol is the escape hatch.

### xterm.js Front Ends (ttyd, wetty, code-server, …)

Web terminals built on [xterm.js](https://xtermjs.org/) — including ttyd — only render images when the host page loads the [`@xterm/addon-image`](https://www.npmjs.com/package/@xterm/addon-image) addon, and that addon supports **Sixel and the iTerm2 inline-image protocol, but not the Kitty graphics protocol**. Stock builds (including default ttyd) ship without the image addon, so no protocol will render until it is added.

If your front end has the image addon, force a protocol it understands rather than letting auto-detection pick Kitty (which a stale inherited `TERM_PROGRAM=WezTerm` would otherwise select):

```bash
ic --protocol sixel image.png
# or, for the whole session:
export IC_PROTOCOL=sixel    # or: iterm2
ic image.png
```

### tmux Support

The utility automatically detects when running inside tmux and reports an error. This program does not work in tmux (yet).