# The `viewport` filter of nle: static crop and row/column hiding

Issue: #165 (branch `issue-165`). Parent: #148. Decision: H-165-2 on issue
#165 (2026-11-27), option A.

The dynamic zoom and focus (zoom level, ease in and out, dim or blur of the
rest) is not in this branch. It goes to a follow-up issue that waits for
issue #151 and issue #158, because no escape sequence can carry it.

## The idea

The filter runs the `termstate` emulator at the size of the screen. After each
output event, it compares the cells that the output must show with the cells
that it already wrote, and it writes only the changed cells, as escape
sequences. Thus each player and each exporter (`beta play`, `beta export web`,
`beta export video`, the preview of nle) shows the crop, and none of them
changes.

The output shows the screen that `termstate` models. A sequence that
`termstate` does not model (for example the insert mode, the origin mode, a
second screen buffer for mode 1049) has no effect in the output.

## Part 1: the source of each color in `termstate`

`termstate` resolves each SGR color to RGB when it reads the SGR. It keeps the
RGB value for each existing caller, with no change. Next to each RGB value, it
also keeps where the color came from:

```rust
pub enum ColorSource {
    Default,          // the default color of the theme
    Base(u8),         // 0..=15: SGR 30-37, 40-47, 90-97, 100-107
    Indexed(u8),      // SGR 38;5;n or 48;5;n
    Rgb(u8, u8, u8),  // SGR 38;2;r;g;b or 48;2;r;g;b
}
```

- `Cell` gets `fg_source` and `bg_source`.
- `TerminalState` gets `current_fg_source` and `current_bg_source`, next to
  `current_fg` and `current_bg`.
- Each place that sets `current_fg` or `current_bg` also sets the source. SGR
  0 and RIS set `Default`. A cell that a clear, an erase, a scroll, an insert,
  or a delete makes gets `Default`. A printed cell copies the pen.
- SGR 39 and 49 set the source of the foreground or the background to
  `Default`. The RGB value stays the same, because decision H-165-2 keeps the
  RGB output of termstate for each existing caller. The filter writes the
  default colors through the reset `SGR 0`, and never through 39 or 49.

## Part 2: the screen filter form of the pipeline

A fourth form in `src/nle/src/pipeline.rs`, next to the event, in-place, and
span forms:

```rust
pub struct ScreenSize { pub width: u16, pub height: u16 }

pub struct ScreenView<'a> {
    pub region: RegionView,   // as for an in-place filter
    pub size: ScreenSize,     // the size of the screen at this step
    pub before: &'a [Event],  // the events before the region, times on the timeline
}

pub struct Screen { pub events: Vec<Event>, pub size: ScreenSize }

pub type ScreenFilterFn =
    dyn Fn(&[Event], ScreenView<'_>, &FilterOptions) -> Result<Screen, String> + Send + Sync;
```

`FilterRegistry::register_screen` registers one. The rules:

1. Steps 1 to 3 and step 7 of `FilterFn` are the same.
2. The size of the first step is the width and the height of the header that
   `render` takes. The size of a later step is the size that the step before
   it answered.
3. An `Err` fails the render with `filter "<kind>" failed: <message>`.
4. The answered times obey step 3 of `InPlaceFilterFn`, and nothing moves, as
   in step 4 of `InPlaceFilterFn`.
5. The answered width and height must each be at least 1. On a region that is
   not `All`, the answered size must be the size of the view. Else the render
   fails with an error that names the kind and the size.
6. The final size is the `width` and the `height` of the rendered recording.
7. `FilterRegistry::run` has no header. It refuses a screen filter with a
   render error that names the kind.

## Part 3: the `viewport` filter

`src/nle/src/filters/viewport.rs`, kind `viewport`, registered in
`filters::registry()`. Options, with their keys in the project file:

```json
{
  "rows":         { "start": 1, "end": 24 },
  "columns":      { "start": 0, "end": 80 },
  "hide_rows":    [0],
  "hide_columns": [79]
}
```

- `rows` and `columns`: the crop. Counted from 0. The start is in the crop,
  and the end is not. A missing key keeps all rows or all columns.
- `hide_rows` and `hide_columns`: rows and columns of the full screen that do
  not show, also when they are not next to each other. An index outside the
  crop has no effect.
- The visible rows are the rows of the crop that are not hidden. The visible
  columns are the same for columns.
- An unknown key fails the render. A start that is not before its end, an end
  after the edge of the screen, a hidden index that is not on the screen, no
  visible row, and no visible column each fail the render, with a message that
  names the option.
- A screen of width 0 or height 0, or larger than the limits of the preview
  (500 by 500), fails the render. Thus a bad header cannot cause a panic or a
  very large allocation.
- When all rows and all columns are visible, the filter answers the events
  with no change, at the same size.

### Region `All`

The output size is the count of visible columns by the count of visible rows.
The visible cells move together: a hidden row or column goes out, and the
cells after it move up or left.

### Region `Range`

The size stays the same, and each cell keeps its place. During the range,
each cell outside the visible set is blank: a space with the default colors
and no attribute.

The filter feeds `view.before` to the emulator first, so it knows the screen
at the start of the range. At the start of the range (relative time 0) it
writes the mask. At the end of a range that is not open (relative time
`view.region.length`) it writes the full screen of the emulator again, and it
sets the state of the terminal back to the state of the emulator:

1. The cells that differ from the full screen.
2. The scrolling region, with DECSTBM.
3. The autowrap mode (mode 7): `CSI ? 7 h` or `CSI ? 7 l`.
4. The pen, with one SGR.
5. The saved cursor position: CUP to it, then `ESC 7`.
6. The pending wrap, when the emulator has one: the cell at the cursor again,
   with its look. Then the real terminal holds the pending wrap too.
7. The line-drawing character set, with `ESC ( 0`, when the emulator uses it.
8. The cursor position, with CUP, but not when a wrap is pending, because a
   CUP ends a pending wrap. Then the visibility, with mode 25.
9. The start of the escape sequence that is open at the end of the range.
   Thus an event after the range that finishes the sequence finishes it in
   the real terminal too.

A sequence that the start of a range splits needs no such step. The first
sequence that the filter writes in the range starts with ESC, and an ESC stops
the open sequence in the real terminal. No character goes before it, because
the cursor of the output is not known at the start.

At the start of the range, the filter writes `ESC ( B` when the emulator uses
the line-drawing character set, because the filter writes each line-drawing
character as its Unicode box-drawing character.

### The writer

The writer keeps a model of the output terminal: each cell (the character,
the two color sources, bold, dim, italic, underline, blink, reverse, hidden,
strikethrough), the pen (or unknown), the cursor position (or unknown), and
the visibility of the cursor.

- For region `All`, the model starts as a blank terminal of the output size,
  with the default pen and the cursor at the top left, visible.
- For region `Range`, the model starts as the emulator after `view.before`.
  The pen and the cursor of the model are unknown at the start. The output
  terminal can hold an attribute that `termstate` does not model, and a full
  SGR removes it. Thus the first cell gets a full SGR, and the first write
  gets a CUP.

A paint compares the target cells with the model in row order. For each cell
that differs:

1. CUP to the cell, when the cursor of the model is not on it.
2. One SGR, when the pen of the model differs from the look of the cell:
   `CSI 0`, then `1` for bold, `2` for dim, `3` for italic, `4` for
   underline, `5` for blink, `7` for reverse, `8` for hidden, `9` for
   strikethrough, then the foreground and the background, then `m`. `Default` adds nothing. `Base(i)`
   adds `30+i` or `40+i` for i below 8, and `90+i-8` or `100+i-8` for 8 to
   15. `Indexed(n)` adds `38;5;n` or `48;5;n`. `Rgb` adds `38;2;r;g;b` or
   `48;2;r;g;b`.
3. The character. After a character whose display width is 1, and that is not
   in the last column, the cursor of the model moves one cell right. After
   each other character, the cursor of the model is unknown.

Then the paint sets the cursor: CUP to the visible place of the emulator
cursor, and mode 25 when the visibility changes. The cursor of the output is
hidden when the emulator hides it, or when its row or its column is not
visible.

### Each output event

The filter writes, at the time of the event:

1. The sequences that do not change a cell, in their order:
   - each OSC, with its terminator. Thus the title, the palette (OSC 4 and
     104), the default colors (OSC 10, 11, 110, 111), and the cursor color
     (OSC 12, 112) stay.
   - each DECSCUSR (`CSI Ps SP q`), the shape of the cursor.
   - mode 12 (`CSI ? 12 h` or `l`), the blink of the cursor, also from a list
     of modes.
   - BEL.
2. The paint of the screen after the event.

The filter removes each other byte of the event, and the paint takes its
place. The filter removes an output event whose new data is empty. An input
event stays as it is.

A `Scanner` of `src/nle/src/ansi.rs` finds the sequences of step 1. It also
reads `view.before`, so a sequence that the start of the range splits is read
correctly.

## Known limits

- The output shows the model of `termstate`. See "The idea".
- The order of the sequences of step 1 and the cells in one event is not
  kept: the sequences go first. For example, an OSC 4 in the middle of an
  event also changes the cells before it in that event.
- An OSC 8 hyperlink applies to the cells that the paint writes after it.
- `ESC 7` in a real terminal also saves the pen. The restore saves the pen of
  the emulator at the end of the range, and not the pen at the original
  `ESC 7`, because `termstate` saves only the position.
- After SGR 39 or 49, `beta export video` and the preview of `nle` still draw
  the color before it, because `termstate` keeps the RGB value (decision
  H-165-2). The output of the filter writes the default color.
- An SGR attribute that `termstate` does not model (for example SGR 6, 21, or
  53) does not show in the output.
