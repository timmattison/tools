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

### Ask whether an image can be displayed here:
```bash
ic --will-display && ic image.png
```

`--will-display` exits with code 0 when this session can show an image, and prints nothing. It exits with code 1 and prints the reason to stderr when it cannot, for example:

```
$ TERM=linux ic --will-display
Error: Image display is not supported in this terminal.
...
```

The flag asks the same question the display path asks — the terminal, the multiplexer, and the remote transport — so a session that passes cannot be refused by the next `ic image.png`. The flag does not ask whether stdout is a terminal, thus a redirected stdout does not change the answer.

`--will-display` is an input mode of its own. Do not combine it with a file, `--stdin`, or `--monitor`.

## Command Line Options

- `FILE` - Image file to display (optional if using --stdin)
- `-w, --width <WIDTH>` - Width in characters (defaults to auto-sizing)
- `--height <HEIGHT>` - Height in characters (defaults to auto-sizing)
- `--preserve-aspect` - Preserve aspect ratio when resizing (default: true)
- `--stdin` - Read from stdin instead of file
- `-n, --no-newline` - Leave the cursor to the caller and write only the image (see [Where the Cursor Ends](#where-the-cursor-ends))
- `--will-display` - Report whether this session can display an image, then exit (0 = yes, 1 = no with the reason on stderr)
- `-h, --help` - Print help information
- `-V, --version` - Print version information

## Compatibility

This utility uses iTerm2's inline image protocol or the Kitty image protocol. It may not work correctly with other terminal emulators.

Kitty support is experimental.

Inside Zellij, `ic` uses Sixel instead, since the Kitty protocol does not pass through Zellij's renderer.

### Where the Cursor Ends

`ic` puts the cursor at column 1 of the first row below the image. It does the
same thing for all three display routines, and the position does not depend on
how the renderer moves the cursor.

No inline-image protocol gives a usable contract for the position of the cursor
after an image. Sixel gives none at all, so each renderer decides for itself.
The Kitty protocol and the iTerm2 protocol each have a flag that holds the
cursor still, but then the caller must move it. `ic` therefore states the
position itself. Each routine writes four parts:

1. One newline for each row of the image, bounded by the height of the terminal.
   The reservation makes an image at the bottom of the screen scroll the
   terminal instead of run off it. An image taller than the screen cannot have a
   row below it, because the cursor stops at the edge of the screen. The bound
   therefore holds the scroll to one screen.
2. CUU, which goes back to the top of the reservation.
3. DECSC, the image, and DECRC. The brackets make sure that cursor motion inside
   the image cannot change the final position.
4. CUD by the row count, and a carriage return.

`-n, --no-newline` suppresses all four parts, and `ic` then writes only the
image. Video playback uses that mode, because it puts the cursor where it wants
it before every frame.

The auto-fit size also pays for the rows that `ic` prints above the image. The
file name rows, the image, and the row that the shell prompt returns to together
fit inside the height of the terminal. A long file name is wider than the
terminal, and the terminal then wraps it onto more than one row. `ic` measures
the display width of each header line and counts the rows that the line takes,
so a wrapped name pays for every row that it occupies.

All three display routines obey that row count. The Kitty protocol and the
iTerm2 protocol take the size of the image in character cells, so they take the
count as it is. Sixel takes the size in pixels, and `ic` multiplies the count by
the height of one character cell. Some terminals also report their own size in
pixels, and `ic` keeps a margin inside that size. The image then gets the
smaller of the two, because the margin knows nothing about the header rows and
the row count knows nothing about the edge of the screen.

### muxiavelli Panels

Inside a [muxiavelli](https://github.com/timmattison/muxiavelli) panel, the web terminal is ttyd's xterm.js with `@xterm/addon-image`, which renders **Sixel and iTerm2's inline image protocol (IIP) only — not the Kitty graphics protocol**.

A panel's PTY inherits the environment of whatever terminal launched the muxiavelli server, so the usual host-terminal detection would mis-fire (e.g. emit the Kitty protocol the addon cannot render). To avoid this, muxiavelli advertises its real capability and `ic` honors it:

| Variable | Value | Meaning |
|----------|-------|---------|
| `MUXIAVELLI` | `1` | The PTY is inside a muxiavelli panel. Checked before every host-terminal signal so leaked env vars cannot win. |
| `MUXIAVELLI_IMAGE_PROTOCOLS` | e.g. `sixel,iterm2` | Ordered preference of inline-image protocols the panel renders. `ic` picks the first one it supports. Absent, empty, or naming nothing supported falls back to Sixel. The Kitty protocol is intentionally never used in muxiavelli panels. |

### tmux Support

The utility automatically detects when running inside tmux and reports an error. This program does not work in tmux (yet).