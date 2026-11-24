# gsw: age-driven truecolor fade for file rows

**Status:** Approved
**Date:** 2026-05-22
**Author:** gsw maintainers

## Motivation

`gsw` already fades recent-commit rows from bright to dark as commits age, using a
24-bit (truecolor) ANSI gradient (`render.rs:407–448`). The file list still uses
the legacy bucketed dim styling (`AgeDim::Fresh/Recent/Aging/Stale`). Within one
`viddy gsw` frame the two stacked sections therefore speak different visual
languages: the log section reads as a continuous timeline, while the file list
reads as four flat brightness buckets. We want files to share the same gradient
so the eye can scan both sections under one rule — newest content is brightest,
oldest content sits at the dark floor.

## Goals

1. In truecolor mode, every cell of a file row (icon, status letter, path, bar,
   `+adds`, `-dels`, age) fades from a status-appropriate base RGB toward the
   shared `FADE_FLOOR` (30%) as the file's mtime ages, using the existing linear
   0..2h ramp.
2. Status semantics survive the fade: conflicts stay red-flavored, staged stays
   green-flavored, untracked stays cyan-flavored, etc. — the hue is preserved
   across the whole fade range; only brightness decreases with age.
3. Files with no available mtime (deleted files, untracked directories we don't
   stat) render at the floor.
4. In the 8-color fallback path, file-row rendering is byte-identical to today —
   no behavior change for terminals without truecolor.

## Non-goals

- Changing the file-list sort order. The newest-first sort already shipped in
  #246 and stays as-is.
- Reworking the `AgeDim` bucket enum. It still serves the 8-color fallback for
  both the log section and the file list, and is left untouched.
- Fading the header, separators, or `+N more files` footer. Chrome stays
  neutral so the eye has a stable frame.
- Fading any column on the log section differently than it already does.

## Design

### Shared fade helper

The existing `fade_truecolor(s, age, base)` (`render.rs:407–410`) generalizes
cleanly to file rows. We add one new helper alongside it:

```rust
/// Fade factor for a file row.
///
/// `Some(age)` uses the same `age_fade_factor(age)` ramp as commit rows so
/// the two sections share one timeline. `None` returns `1.0` — files we
/// can't stat (deleted entries, skipped untracked dirs) render at the
/// dark floor, consistent with today's `.dimmed()` styling for None ages.
fn file_fade_factor(age: Option<Duration>) -> f32 {
    age.map_or(1.0, age_fade_factor)
}
```

The existing `age_fade_factor` and `fade_rgb` are untouched.

### Per-status base RGB palette

A small set of constants near the existing `LOG_*_BASE_RGB` block. Each base is
chosen so that at `factor = 0` it visually approximates today's ANSI color, and
at `factor = 1` (× 30%) it still reads as the same hue family — never a
near-black blob.

| Element                     | Base RGB (approx) | Notes                                         |
|-----------------------------|-------------------|-----------------------------------------------|
| `FILE_ICON_STAGED_RGB`      | (90, 220, 110)    | Green `●`                                     |
| `FILE_ICON_UNSTAGED_RGB`    | (220, 200, 100)   | Yellow `○`                                    |
| `FILE_ICON_UNTRACKED_RGB`   | (120, 200, 200)   | Already-dim cyan `?`                          |
| `FILE_ICON_CONFLICT_RGB`    | (255, 80, 80)     | Red-bold `!`                                  |
| `FILE_LETTER_ADDED_RGB`     | (90, 220, 110)    | Green `A`                                     |
| `FILE_LETTER_DELETED_RGB`   | (255, 80, 80)     | Red `D`                                       |
| `FILE_LETTER_RENAMED_RGB`   | (220, 120, 220)   | Magenta `R` / `C`                             |
| `FILE_LETTER_DEFAULT_RGB`   | (230, 230, 230)   | Plain-bold `M` / `T`                          |
| `FILE_LETTER_CONFLICT_RGB`  | (255, 80, 80)     | Red `U`                                       |
| `FILE_LETTER_UNTRACKED_RGB` | (120, 200, 200)   | Cyan `?`                                      |
| `FILE_PATH_STAGED_RGB`      | (200, 200, 200)   | Approximates `.normal().dimmed()`             |
| `FILE_PATH_UNSTAGED_RGB`    | (220, 200, 100)   | Approximates `.yellow()`                      |
| `FILE_PATH_UNTRACKED_RGB`   | (120, 200, 200)   | Approximates `.cyan().dimmed()`               |
| `FILE_PATH_CONFLICT_RGB`    | (255, 90, 90)     | Approximates `.red()`                         |
| `FILE_ADDS_RGB`             | (90, 220, 110)    | Matches `.green()`                            |
| `FILE_DELS_RGB`             | (255, 90, 90)     | Matches `.red()`                              |
| `FILE_BAR_RGB`              | (60, 200, 200)    | Matches `.cyan()` fill                        |
| `FILE_BAR_CONFLICT_RGB`     | (255, 80, 80)     | Matches `.red()` fill for conflicted rows     |
| `FILE_BAR_PARTIAL_BG_RGB`   | (0, 48, 48)       | Existing `BAR_PARTIAL_BG_CYAN`, reused        |
| `FILE_BAR_PARTIAL_BG_RED`   | (48, 0, 0)        | Existing `BAR_PARTIAL_BG_RED`, reused         |
| `FILE_AGE_RGB`              | (190, 190, 190)   | Reuses `LOG_AGE_BASE_RGB` value for parity    |
| `FILE_BIN_RGB`              | (140, 140, 140)   | Approximates `.dimmed()` for the `bin` marker |

These are tuned by eye against the legacy ANSI palette; small adjustments during
implementation are fine as long as the fresh end still reads as the same hue
family.

### Threading `truecolor` through `render_row`

`render_row` already receives `&RenderOptions`, which carries the `truecolor`
flag. The change is mechanical:

1. Compute `let factor = file_fade_factor(entry.age);` once at the top of
   `render_row`.
2. Pass `factor` and `opts.truecolor` (or a small `FadeCtx { factor, truecolor }`
   struct, decided during implementation) into each `colorize_*` call.
3. Each `colorize_*` function gains the shape:
   ```rust
   fn colorize_path(path: &str, entry: &RenderEntry, factor: f32, truecolor: bool) -> ColoredString {
       if truecolor {
           let base = match entry.status { /* per-status RGB */ };
           let (r, g, b) = fade_rgb(base, factor);
           path.truecolor(r, g, b)
       } else {
           // existing 8-color logic, byte-identical to today
       }
   }
   ```
4. The bar colorizer fades both the fill foreground and the partial-cell
   background by the same factor so the bar darkens uniformly.

### Bar fade specifics

The bar today uses `.cyan()` (ANSI 8-color) for filled cells and a 24-bit
`on_truecolor(0, 48, 48)` background for partial cells. In truecolor mode:
- Filled-cell foreground becomes `truecolor(r, g, b)` where `(r,g,b)` is
  `fade_rgb(FILE_BAR_RGB, factor)`.
- Partial-cell background becomes `on_truecolor(r', g', b')` where `(r',g',b')`
  is `fade_rgb(BAR_PARTIAL_BG_CYAN, factor)`.
- Same swap for the conflicted-row red variant.

This keeps the gap-closing trick from #248 working while letting the whole bar
fade together.

### Untracked rows

Untracked rows skip the bar/adds/dels block and just pad the gutter. Their age
is set (we stat them in `git.rs`), so they fade normally on icon, letter, path,
and age. The gutter padding stays whitespace — no need to fade empty space.

### Binary files

The `bin` marker keeps its dim styling. In truecolor mode it fades from
`FILE_BIN_RGB` toward the floor — same fade contract as everything else.

## Data flow

```
RenderEntry.age  ──►  file_fade_factor(age)  ──►  factor: f32
                                                     │
RenderOptions.truecolor  ────────────────────────────┤
                                                     ▼
                                     each colorize_*(..., factor, truecolor)
                                                     │
                                  ┌──────────────────┴──────────────────┐
                                  ▼                                     ▼
                          truecolor: fade_rgb(base, factor)     8-color: existing path
                                  │                                     │
                                  └──────────────► ColoredString ◄──────┘
```

## Testing (TDD red → green)

Tests are added under `render.rs`'s existing `#[cfg(test)] mod tests` block,
following the pattern set by the log-fade tests (`render.rs:1388–1517`). All
tests inspect `ColoredString::fgcolor` / `bgcolor` typed enums rather than ANSI
bytes, so they don't race on `colored::control::set_override`.

Required tests (each commits as a red test first, then a green commit):

1. **`file_path_uses_truecolor_when_enabled`** — `colorize_path` for a modified
   file at age=0 with truecolor=true returns a `Color::TrueColor { .. }`.
2. **`file_path_falls_back_to_legacy_color_without_truecolor`** — same call with
   truecolor=false returns `Color::Yellow` (or the legacy color for that
   status); proves the 8-color path is untouched.
3. **`file_row_darkens_with_age_under_truecolor`** — render two snapshots of the
   same file (age=0 and age=1h) under truecolor; every column's R channel of
   the 1h row is strictly less than the corresponding fresh row's R channel.
4. **`file_row_stays_above_floor_when_very_old`** — at age=1 week, every column
   stays at or above its `FADE_FLOOR` fraction (mirrors
   `log_hash_stays_above_floor_when_very_old`).
5. **`file_row_no_age_renders_at_floor`** — a deleted file (age=None) under
   truecolor renders every column at the floor.
6. **`file_row_status_hues_remain_distinct_at_floor`** — at age=∞, the icon RGBs
   for Modified/Added/Deleted/Conflicted/Untracked are pairwise distinct, so
   fading never collapses status hues into one indistinguishable blob.
7. **`bar_fill_fades_with_age_under_truecolor`** — bar fill at age=0 is
   brighter (higher channel sum) than at age=1h.
8. **`bar_partial_background_fades_with_age_under_truecolor`** — the
   `on_truecolor` background of a partial-cell bar also darkens with age.
9. **`file_row_byte_identical_in_8_color_mode`** — render a snapshot with
   truecolor=false twice (once representing today's behavior, once
   post-change) and assert the ANSI byte sequence matches a golden string for a
   representative row covering each status. Acts as the regression guard for
   the fallback path. *(Implementation note: rather than committing a
   pre-change golden, we capture today's output via a small test fixture
   committed alongside the red test in the first TDD step.)*

## Out of scope

- Tunable per-status base colors via env vars or CLI flags.
- Color profile / theming systems beyond the existing truecolor on/off switch.
- Animating between brightness states across `viddy` refreshes.
- Any change to the log section, header, separator, or footer.

## Open questions

None. The design above resolves the only two genuinely ambiguous points
(scope = whole row, None-age = floor) per the brainstorming discussion.
