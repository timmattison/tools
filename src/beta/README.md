# beta - Terminal Session Recorder and Player

`beta` is a terminal session recorder and player that captures all terminal output with high-resolution timestamps, allowing you to record and replay terminal sessions exactly as they happened.

## Features

### Recording & Playback
- Records all terminal input and output with microsecond precision timestamps
- Plays back recordings with accurate timing
- Supports playback speed control (slow down or speed up)
- Pause/resume functionality during playback
- Rewind and fast-forward by 5 seconds using arrow keys
- Optional gzip compression for recordings
- JSON-based recording format for easy parsing and manipulation
- Terminal size tracking and restoration

### Export Options
- **Web Export**: Generate self-contained HTML files with embedded JavaScript player
- **Video Export**: Create MP4 or GIF videos from terminal recordings
- **High-quality text rendering**: Uses JetBrains Mono Nerd Font with programming ligatures and icons
- Multiple theme support (Dracula, Monokai, Solarized Dark/Light)
- Configurable video settings (FPS, resolution, optimization)
- No external dependencies for web playback

## Installation

```bash
cargo install --path .
```

Or from the tools repository:

```bash
cargo install --git https://github.com/timmattison/tools beta
```

## Usage

### Recording a Session

Basic recording:
```bash
beta record
```

Record with custom output file:
```bash
beta record -o session.json
```

Record a specific command:
```bash
beta record -c "npm test"
```

Record with compression:
```bash
beta record --compress -o session.json.gz
```

Record with custom stop hotkey:
```bash
beta record --stop-hotkey f12
beta record --stop-hotkey "ctrl-]"
```

#### Available Stop Hotkeys

- `ctrl-end` (default) - Ctrl + End Key ⭐ **Recommended**
- `f12` - Function Key 12
- `ctrl-]` - Ctrl + Right Square Bracket
- `ctrl-\\` - Ctrl + Backslash  
- `ctrl-c` - Ctrl + C (traditional interrupt)

#### Debugging Hotkeys

If hotkeys aren't working as expected, enable debug mode to see what key events are being received:

```bash
BETA_DEBUG=1 beta record
```

This will print debug information about each key press, helping identify how your terminal reports key combinations.

### Playing Back a Recording

Basic playback:
```bash
beta play session.json
```

Play at double speed:
```bash
beta play session.json -s 2.0
```

Start playback paused:
```bash
beta play session.json --paused
```

### Exporting Recordings

Export to self-contained HTML:
```bash
beta export web session.json -o session.html
```

Export to video with custom theme:
```bash
beta export video session.json -o session.mp4 --theme dracula --fps 30
```

Export to optimized GIF:
```bash
beta export video session.json -o session.gif --fps 15 --optimize-web
```

### Playback Controls

During playback, you can use the following keyboard controls:

- **Space**: Pause/Resume playback
- **←**: Rewind 5 seconds
- **→**: Fast-forward 5 seconds
- **↑**: Increase playback speed
- **↓**: Decrease playback speed
- **q** or **Esc**: Quit playback

## Recording Format

The recording format is JSON-based and compatible with asciinema v2 format structure:

```json
{
  "version": 2,
  "width": 80,
  "height": 24,
  "timestamp": 1234567890.123,
  "duration": 123.456,
  "command": "bash",
  "title": "Terminal recording at 2024-01-01 12:00:00",
  "env": {},
  "events": [
    {"time": 0.0, "type": "o", "data": "$ "},
    {"time": 1.234, "type": "i", "data": "ls\r"},
    {"time": 1.345, "type": "o", "data": "file1.txt  file2.txt\r\n$ "}
  ]
}
```

Event types:
- `"o"`: Output from the terminal
- `"i"`: Input to the terminal

## Examples

### Recording Examples

#### Record a build process
```bash
beta record -o build.json -c "cargo build --release"
```

#### Record an interactive session
```bash
beta record -o demo.json
# ... do your work ...
# Press Ctrl-D or type 'exit' to finish
```

#### Create a compressed recording
```bash
beta record --compress -o session.json.gz
```

### Playback Examples

#### Play back at half speed
```bash
beta play demo.json -s 0.5
```

#### Start playback paused
```bash
beta play demo.json --paused
```

### Export Examples

#### Export to Web (HTML)
```bash
# Basic web export
beta export web demo.json -o demo.html

# With Dracula theme
beta export web demo.json -o demo.html --theme dracula

# With compressed data embedding
beta export web demo.json -o demo.html --compress
```

#### Export to Video
```bash
# Basic MP4 export
beta export video demo.json -o demo.mp4

# Create an optimized GIF
beta export video demo.json -o demo.gif --fps 15 --optimize-web

# High-quality video with custom settings
beta export video demo.json -o demo.mp4 --fps 60 --resolution 1920x1080 --theme monokai

# Web-optimized MP4
beta export video demo.json -o demo.mp4 --optimize-web --theme solarized-dark
```

**Font Requirements for Video Export:**

**Recommended (Best Quality):**
- **JetBrains Mono Nerd Font** - Modern coding font with programming ligatures and icons
  - Download: https://github.com/ryanoasis/nerd-fonts/releases
  - macOS: `brew tap homebrew/cask-fonts && brew install font-jetbrains-mono-nerd-font`
  - Linux: Download and extract to `~/.local/share/fonts/`

**Fallback Options:**
- macOS: Monaco or Menlo (pre-installed)
- Linux: `sudo apt install fonts-dejavu fonts-liberation`

## Technical Details

- Uses pseudo-terminals (PTY) to capture all terminal I/O
- **Raw mode terminal capture** for proper keyboard input handling
- **Crossterm event handling** for graceful recording termination (Ctrl-End)
- **Thread synchronization** for reliable data capture and storage
- Preserves ANSI escape sequences and terminal control codes with VTE parser
- Records terminal dimensions for accurate playback
- Thread-based I/O handling with proper error recovery
- Async playback engine for smooth performance
- **Automatic recording save** on shell exit or interruption

## Recording Controls

- **Exit gracefully**: Press `Ctrl-End` to stop recording and save the session (default hotkey)
- **Custom hotkeys**: Configure with `--stop-hotkey` option (supports: `ctrl-end`, `f12`, `ctrl-]`, `ctrl-\\`, `ctrl-c`)
- **Shell exit**: Type `exit` in the shell to end the session normally
- **Interactive programs**: All keyboard input (including `q` to quit programs like `btop`) works correctly
- **Recovery**: Recording is automatically saved even if the shell exits unexpectedly

## Limitations

- Currently only supports Unix-like systems (Linux, macOS)
- Requires a terminal environment for recording
- Does not capture terminal resize events during recording (uses initial size)