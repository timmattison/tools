# beta - Terminal Session Recorder and Player

`beta` is a terminal session recorder and player that captures all terminal output with high-resolution timestamps, allowing you to record and replay terminal sessions exactly as they happened.

## Features

- Records all terminal input and output with microsecond precision timestamps
- Plays back recordings with accurate timing
- Supports playback speed control (slow down or speed up)
- Pause/resume functionality during playback
- Rewind and fast-forward by 5 seconds using arrow keys
- Optional gzip compression for recordings
- JSON-based recording format for easy parsing and manipulation
- Terminal size tracking and restoration

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

### Record a build process
```bash
beta record -o build.json -c "cargo build --release"
```

### Record an interactive session
```bash
beta record -o demo.json
# ... do your work ...
# Press Ctrl-D or type 'exit' to finish
```

### Play back at half speed
```bash
beta play demo.json -s 0.5
```

### Create a compressed recording
```bash
beta record --compress -o session.json.gz
```

## Technical Details

- Uses pseudo-terminals (PTY) to capture all terminal I/O
- Preserves ANSI escape sequences and terminal control codes
- Records terminal dimensions for accurate playback
- Thread-based I/O handling for efficient recording
- Async playback engine for smooth performance

## Limitations

- Currently only supports Unix-like systems (Linux, macOS)
- Requires a terminal environment for recording
- Does not capture terminal resize events during recording (uses initial size)