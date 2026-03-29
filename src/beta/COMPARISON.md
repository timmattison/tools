# Beta vs SHELLcast: Comparison

## Name

Beta is named after Betamax in the VHS vs Betamax format war. The competing tool in this space is [VHS by Charm](https://github.com/charmbracelet/vhs), which takes a completely different approach: scripted `.tape` files that automate terminal input to produce GIFs, rather than actually recording real sessions. Beta, like Betamax, is the technically superior format.

## History

**SHELLcast** was written in Perl in March-April 2003 by ntheory. It consisted of three scripts totaling roughly 150 lines of code:

- `SHELLcast.pl` -- TCP broadcast server (port 2323) that read piped stdin and forwarded it to connected telnet clients
- `scr.pl` -- Recorder that connected to a running SHELLcast server and saved timestamped hex-encoded output to a file
- `scp.pl` -- Player that read a recording file and replayed it at original speed

**Beta** is a Rust rewrite (~3000+ lines) built around PTY-based capture, JSON recording, and multiple export formats.

## Architecture

| Aspect | SHELLcast (2003) | Beta (2025) |
|---|---|---|
| Language | Perl 5 | Rust |
| Capture method | Pipe-based (`PROGRAM \| perl SHELLcast.pl`) | PTY-based (`portable-pty`) |
| Recording format | Custom: `timestamp\|hex-encoded-bytes` per line | JSON (asciinema v2 compatible) |
| Networking | TCP server with `IO::Socket` / `IO::Select` | None |
| Async model | Single-threaded select loop + `ReadKey` polling | Multi-threaded with `tokio` async runtime |
| Dependencies | `IO::Socket`, `IO::Select`, `Term::ReadKey`, `Time::HiRes` | `portable-pty`, `crossterm`, `tokio`, `serde`, `flate2`, `vte`, `image`, `ffmpeg` (external) |

## Feature Comparison

| Feature | SHELLcast | Beta | Notes |
|---|---|---|---|
| **Recording** | Yes (pipe) | Yes (PTY) | Pipe approach worked for many interactive programs (e.g., ASCII Tetris), but programs that call `isatty()` won't behave correctly. PTY capture is transparent to the child process. |
| **Playback** | Fixed-speed only | Interactive controls | Beta supports pause, seek (rewind/fast-forward 5s), speed adjust (0.1x-10x), quit. SHELLcast had a busy-wait loop with `usleep` for timing. |
| **Network broadcasting** | Yes -- TCP server on port 2323 for live multi-viewer telnet sessions | No | SHELLcast's core feature. Viewers connected via `telnet host 2323`. Server sent `IAC WILL ECHO` to suppress client-side echo. |
| **Live remote recording** | Yes -- `scr.pl` connected to a running server and captured to file | No | Enabled recording someone else's broadcast, or your own from a separate machine. |
| **Rebroadcasting** | Yes -- `cat RECORDFILE \| perl SHELLcast.pl` | No | Replayed a recording through the broadcast server so telnet viewers could watch it. |
| **Web export** | No | Yes | Self-contained HTML with embedded xterm.js. Includes playback controls, progress bar, seek, speed selection, and theme support (Dracula, Monokai, Solarized Dark/Light). |
| **Video export** | No | Yes | MP4 (H.265 lossless or lossy) and GIF via FFmpeg. Font rendering with multi-font Unicode fallback. Configurable resolution and FPS. |
| **Terminal emulation** | None -- passed raw bytes through | Full VT100/ANSI emulation | 1400+ line terminal state machine using the `vte` parser. Handles cursor movement, scrolling regions, SGR attributes, 256-color and truecolor, tmux status bars, and more. |
| **Compression** | No | Yes (gzip) | Both recording files and web export data can be gzip-compressed. |
| **Terminal size capture** | No | Yes | Records terminal dimensions at start. Used for correct playback and export rendering. |
| **tmux awareness** | No | Yes | Detects tmux layouts, handles status bar rendering, validates cursor position within pane boundaries. |
| **Append mode** | No | Yes | Can append to an existing recording file. |
| **Signal handling** | No | Yes | Graceful Ctrl-C shutdown with proper raw mode cleanup. |
| **Format portability** | Custom format, no external tool support | asciinema v2 JSON, interoperable with the asciinema ecosystem | SHELLcast's `timestamp\|hex` format was simple but proprietary. |

## Recording Format

**SHELLcast** (`scr.pl` output):
```
0.123456|48656c6c6f
0.234567|0a
```
Each line: elapsed seconds since start, pipe delimiter, hex-encoded bytes.

**Beta** (JSON):
```json
{
  "version": 2,
  "width": 120,
  "height": 40,
  "timestamp": 1710000000.0,
  "duration": 30.5,
  "command": "/bin/zsh",
  "title": "Terminal recording at 2025-01-15 10:30:00",
  "events": [
    {"time": 0.123, "type": "o", "data": "Hello"},
    {"time": 0.234, "type": "i", "data": "\n"}
  ]
}
```

## What Beta Is Missing

Two features from the original SHELLcast have no equivalent in Beta:

1. **Network broadcasting** -- SHELLcast's defining feature was its TCP server that let multiple viewers watch a terminal session live via telnet. This enabled real-time demos, tutorials, and monitoring without screen sharing tools.

2. **Live remote recording** -- The `scr.pl` script could connect to any running SHELLcast server and record the session to a file. This decoupled recording from broadcasting and allowed third-party capture.

3. **Rebroadcasting** -- Piping a recorded file back through `SHELLcast.pl` let you replay sessions to live telnet viewers, functioning as a simple time-shifted broadcast.

These features made SHELLcast a networked collaboration tool, not just a local recorder. Beta currently operates as a local-only tool with offline export capabilities.
