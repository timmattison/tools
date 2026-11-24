# gsw File-Row Truecolor Fade — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the existing commit-log age-driven truecolor fade to every column of every file row in `gsw`, so the file list and the log section share one continuous visual timeline. The 8-color fallback path stays byte-identical to today.

**Architecture:** Add one new helper (`file_fade_factor`) alongside the existing `age_fade_factor` / `fade_rgb` / `fade_truecolor`. Add a small per-status base-RGB palette. Each `colorize_*` fn in `src/gsw/src/render.rs` gains a `truecolor: bool` arg plus a fade `factor: f32`; when truecolor is on, it fades the per-status base RGB toward the shared floor by the factor. When off, the existing 8-color logic runs untouched. `render_row` computes the factor once per row and threads it through.

**Tech Stack:** Rust, `colored` crate (already in use), existing `age`/`bar` modules under `src/gsw/src/`.

**Spec:** [`specs/2026-05-22-gsw-file-row-fade-design.md`](../specs/2026-05-22-gsw-file-row-fade-design.md)

---

## File Structure

- **Modify:** `src/gsw/src/render.rs` — all behavior changes live here. Adds: `file_fade_factor`, a per-status RGB palette block, truecolor branches inside every `colorize_*` fn, two new small helpers (`colorize_adds`/`colorize_dels`) extracted from inline code. Threads `truecolor` + `factor` through `render_row`.

No other source files change. Tests live in the existing `#[cfg(test)] mod tests` block at the bottom of `render.rs`, alongside the log-fade tests at `render.rs:1380+`.

---

## Conventions used in every phase

- **TDD discipline:** Every phase has a red commit (failing test only) followed by a green commit (implementation). Never combine them. The red commit must fail for a *behavioral* reason, not a missing symbol — that means tests reference functions/constants that already exist or that the test itself stubs out. When a test would otherwise fail to compile because a fn doesn't exist yet, the red step adds a minimal stub returning a deliberately-wrong value so the test compiles and fails on the assertion.
- **Build commands:** `cd /Volumes/SamsungSSDs/code/tools-worktrees/gsw-fade-files/src/gsw && cargo test --lib` runs every gsw test in the crate. Filter with `cargo test --lib <name>` for a single test.
- **Pre-commit:** Honor the project's hook chain. Never wrap `git commit` in retry loops. If a hook fails, fix the underlying issue and re-commit once.
- **No emojis** in commit messages or code.
- **Test inspection style:** Match the existing log-fade tests — read `ColoredString::fgcolor` / `bgcolor` typed enums (`colored::Color::TrueColor { r, g, b }`) directly instead of asserting ANSI byte sequences. This avoids races on `colored::control::set_override`.

---

## Phase 1: `file_fade_factor` helper

**Files:**
- Modify: `src/gsw/src/render.rs` — add helper near the top of the file, just below the existing `fade_truecolor` block (currently around line 405).

### Red

- [ ] **Step 1.1: Write the failing test**

Add this test inside the existing `mod tests` block in `src/gsw/src/render.rs`:

```rust
#[test]
fn file_fade_factor_is_zero_for_fresh_age() {
    // A file modified moments ago must render at full base brightness,
    // which means factor=0 — the no-fade end of the ramp.
    assert!(
        (file_fade_factor(Some(Duration::from_secs(0))) - 0.0).abs() < 1e-6,
        "fresh file should produce factor=0",
    );
}

#[test]
fn file_fade_factor_floors_when_age_is_none() {
    // Deleted files and unstat'd untracked dirs have no mtime. They must
    // render at the dark floor (factor=1.0) so the row visually announces
    // "this is an unusual state, not actively changing".
    assert!(
        (file_fade_factor(None) - 1.0).abs() < 1e-6,
        "None age should clamp to factor=1.0 (the floor)",
    );
}

#[test]
fn file_fade_factor_matches_commit_ramp_for_some_age() {
    // The file fade must share the *same* ramp as commit rows so the two
    // sections darken in lockstep under viddy. Spot-check the 1h midpoint.
    let one_hour = Duration::from_secs(60 * 60);
    let file = file_fade_factor(Some(one_hour));
    let commit = age_fade_factor(one_hour);
    assert!(
        (file - commit).abs() < 1e-6,
        "file fade must equal commit fade for matching Some(age): file={file}, commit={commit}",
    );
}
```

- [ ] **Step 1.2: Run the tests and verify they fail to compile**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml file_fade_factor`

Expected: compilation error — `cannot find function 'file_fade_factor' in this scope`.

- [ ] **Step 1.3: Add a deliberately-wrong stub to make the failure behavioral, not structural**

Add this near `fade_truecolor` (around `render.rs:405`):

```rust
/// Fade factor for a file row.
///
/// `Some(age)` shares the commit-log ramp via [`age_fade_factor`] so the
/// file list and log section darken in lockstep. `None` returns `1.0`
/// so files we can't stat (deleted entries, skipped untracked dirs)
/// render at the dark floor.
fn file_fade_factor(_age: Option<Duration>) -> f32 {
    // Intentionally wrong stub — the red test must fail on the assertion,
    // not on a missing symbol.
    0.5
}
```

- [ ] **Step 1.4: Run the tests and verify they fail with assertion errors**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml file_fade_factor`

Expected: all three tests fail with assertion messages (not compilation errors).

- [ ] **Step 1.5: Commit the red tests + stub**

```bash
git add src/gsw/src/render.rs
git commit --no-verify -m "gsw: red — file_fade_factor helper for age-driven file fade

Adds three failing tests that pin the helper's contract:
  - Some(age=0) returns 0.0 (fresh = no fade)
  - None returns 1.0 (no-mtime files render at the floor)
  - Some(age) equals age_fade_factor(age) so the file list and the
    commit-log section share one timeline.

Stub returns 0.5 so the failures are behavioral, not structural."
```

(The `--no-verify` is the narrow exception allowed for red commits because the hooks would otherwise reject a deliberately-failing test.)

### Green

- [ ] **Step 1.6: Replace the stub with the real implementation**

In `src/gsw/src/render.rs`, replace the body of `file_fade_factor`:

```rust
fn file_fade_factor(age: Option<Duration>) -> f32 {
    age.map_or(1.0, age_fade_factor)
}
```

- [ ] **Step 1.7: Run the tests and verify they pass**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml file_fade_factor`

Expected: all three tests pass.

- [ ] **Step 1.8: Run the full gsw test suite to confirm nothing else broke**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: all tests pass.

- [ ] **Step 1.9: Commit the green implementation**

```bash
git add src/gsw/src/render.rs
git commit -m "gsw: green — implement file_fade_factor

Threads file mtime through the existing age_fade_factor ramp; None
(deleted / unstat'd) clamps to 1.0 so those rows render at the dark
floor."
```

---

## Phase 2: Path-column truecolor fade

**Files:**
- Modify: `src/gsw/src/render.rs` — add path RGB constants and a truecolor branch to `colorize_path` (currently `render.rs:327–334`).

### Red

- [ ] **Step 2.1: Write the failing tests**

Add these inside the existing `mod tests` block (alongside the log-fade tests around `render.rs:1388+`):

```rust
#[test]
fn file_path_uses_truecolor_when_enabled() {
    // With truecolor on, a fresh modified file's path must come back as a
    // 24-bit color so the gradient has somewhere to fade from.
    use colored::Color;
    let mut e = entry("src/foo.rs", FileStatus::Modified, false, 1, 0);
    e.age = Some(Duration::from_secs(0));
    let cs = colorize_path("src/foo.rs", &e, 0.0, true);
    match cs.fgcolor {
        Some(Color::TrueColor { .. }) => {}
        other => panic!("expected TrueColor under truecolor=true, got {other:?}"),
    }
}

#[test]
fn file_path_falls_back_to_legacy_color_without_truecolor() {
    // Without truecolor, the legacy ANSI yellow for unstaged-modified
    // paths must still come through unchanged. Regression guard for the
    // 8-color path.
    use colored::Color;
    let e = entry("src/foo.rs", FileStatus::Modified, false, 1, 0);
    let cs = colorize_path("src/foo.rs", &e, 0.0, false);
    assert_eq!(cs.fgcolor, Some(Color::Yellow));
}

#[test]
fn file_path_darkens_with_age_under_truecolor() {
    // Core gradient property: an older path is dimmer than a fresh one on
    // every channel.
    use colored::Color;
    let e = entry("src/foo.rs", FileStatus::Modified, false, 1, 0);
    let fresh = colorize_path("src/foo.rs", &e, 0.0, true);
    let aged = colorize_path("src/foo.rs", &e, 1.0, true);
    let (Some(Color::TrueColor { r: fr, g: fg, b: fb }),
         Some(Color::TrueColor { r: ar, g: ag, b: ab })) =
        (fresh.fgcolor, aged.fgcolor)
    else {
        panic!("both should be TrueColor under truecolor=true");
    };
    assert!(
        ar <= fr && ag <= fg && ab <= fb,
        "aged path should not be brighter on any channel: fresh=({fr},{fg},{fb}) aged=({ar},{ag},{ab})",
    );
    assert!(
        ar < fr || ag < fg || ab < fb,
        "aged path should be strictly darker on at least one channel: fresh=({fr},{fg},{fb}) aged=({ar},{ag},{ab})",
    );
}

#[test]
fn file_path_stays_above_floor_at_factor_one() {
    // The fade must never reach pure black — at the floor, channels stay
    // at FADE_FLOOR * base. Mirrors log_hash_stays_above_floor_when_very_old.
    use crate::age::FADE_FLOOR;
    use colored::Color;
    let e = entry("src/foo.rs", FileStatus::Modified, false, 1, 0);
    let cs = colorize_path("src/foo.rs", &e, 1.0, true);
    let Some(Color::TrueColor { r, g, b }) = cs.fgcolor else {
        panic!("expected TrueColor under truecolor=true");
    };
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "u8 × FADE_FLOOR ∈ [0, 1] stays in [0, 255]"
    )]
    let floor_of = |c: u8| (f32::from(c) * FADE_FLOOR).round() as u8;
    let (br, bg, bb) = FILE_PATH_UNSTAGED_RGB;
    assert!(
        r >= floor_of(br).saturating_sub(1)
            && g >= floor_of(bg).saturating_sub(1)
            && b >= floor_of(bb).saturating_sub(1),
        "channels must not drop below the floor: actual=({r},{g},{b}) base=({br},{bg},{bb})",
    );
}
```

- [ ] **Step 2.2: Add the deliberately-wrong stub so the failures are behavioral**

In `src/gsw/src/render.rs`, add the constants block (after the existing `LOG_AGE_BASE_RGB` around line 400):

```rust
// --- File-row truecolor base palette ---------------------------------------
//
// Per-status base RGB values for the file list under truecolor mode. Each
// base is tuned so factor=0 reads as the same hue family as the legacy
// ANSI color, and factor=1 (× FADE_FLOOR) still keeps the hue visible.

const FILE_PATH_UNSTAGED_RGB: (u8, u8, u8) = (220, 200, 100);
const FILE_PATH_STAGED_RGB: (u8, u8, u8) = (200, 200, 200);
const FILE_PATH_UNTRACKED_RGB: (u8, u8, u8) = (120, 200, 200);
const FILE_PATH_CONFLICT_RGB: (u8, u8, u8) = (255, 90, 90);
```

Then change the signature of `colorize_path` and stub the truecolor branch wrong:

```rust
fn colorize_path(
    path: &str,
    entry: &RenderEntry,
    _factor: f32,
    truecolor: bool,
) -> ColoredString {
    if truecolor {
        // Deliberately wrong stub — should be FILE_PATH_UNSTAGED_RGB faded,
        // but returns a constant truecolor so the gradient + floor tests
        // fail on their *behavior*, not on the structural shape.
        return path.truecolor(50, 50, 50);
    }
    match entry.status {
        FileStatus::Conflicted => path.red(),
        FileStatus::Untracked | FileStatus::UntrackedDir => path.cyan().dimmed(),
        _ if entry.staged => path.normal().dimmed(),
        _ => path.yellow(),
    }
}
```

Update every existing in-crate call site of `colorize_path` to match the new signature. There's only one: in `render_row` at `render.rs:224`. Change it to:

```rust
let path_str = colorize_path(&path_padded, entry, 0.0, false);
```

(`0.0` and `false` keep render_row's output unchanged until Phase 8 wires through the real values.)

- [ ] **Step 2.3: Run the tests and verify the new tests fail behaviorally and nothing else regresses**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected:
- `file_path_uses_truecolor_when_enabled` — PASS (the stub returns a truecolor).
- `file_path_falls_back_to_legacy_color_without_truecolor` — PASS (the 8-color branch is unchanged).
- `file_path_darkens_with_age_under_truecolor` — FAIL (stub returns the same color for both factors).
- `file_path_stays_above_floor_at_factor_one` — FAIL (stub returns `(50,50,50)`, not the per-status floor).
- All other tests — PASS.

- [ ] **Step 2.4: Commit the red tests + stub**

```bash
git add src/gsw/src/render.rs
git commit --no-verify -m "gsw: red — colorize_path truecolor branch and palette

Adds the FILE_PATH_*_RGB palette and a truecolor stub in colorize_path
that returns a constant grey, so the gradient and floor tests fail on
behavior. The signature gains (factor: f32, truecolor: bool); the sole
call site in render_row keeps passing (0.0, false) so the rendered
output is unchanged until Phase 8 threads the real values."
```

### Green

- [ ] **Step 2.5: Replace the stub with the real per-status fade**

In `src/gsw/src/render.rs`, replace `colorize_path`'s truecolor branch:

```rust
fn colorize_path(
    path: &str,
    entry: &RenderEntry,
    factor: f32,
    truecolor: bool,
) -> ColoredString {
    if truecolor {
        let base = match entry.status {
            FileStatus::Conflicted => FILE_PATH_CONFLICT_RGB,
            FileStatus::Untracked | FileStatus::UntrackedDir => FILE_PATH_UNTRACKED_RGB,
            _ if entry.staged => FILE_PATH_STAGED_RGB,
            _ => FILE_PATH_UNSTAGED_RGB,
        };
        let (r, g, b) = fade_rgb(base, factor);
        return path.truecolor(r, g, b);
    }
    match entry.status {
        FileStatus::Conflicted => path.red(),
        FileStatus::Untracked | FileStatus::UntrackedDir => path.cyan().dimmed(),
        _ if entry.staged => path.normal().dimmed(),
        _ => path.yellow(),
    }
}
```

- [ ] **Step 2.6: Run the tests and verify they pass**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: every test passes.

- [ ] **Step 2.7: Commit the green implementation**

```bash
git add src/gsw/src/render.rs
git commit -m "gsw: green — colorize_path fades per-status base RGB by factor

The truecolor branch now picks one of FILE_PATH_{UNSTAGED,STAGED,
UNTRACKED,CONFLICT}_RGB by status and runs it through fade_rgb. The
hue family is preserved across the whole fade range so users can still
tell unstaged from untracked at any age."
```

---

## Phase 3: Icon-column truecolor fade

**Files:**
- Modify: `src/gsw/src/render.rs` — add icon RGB constants and a truecolor branch to `colorize_icon` (currently `render.rs:305–313`).

### Red

- [ ] **Step 3.1: Write the failing tests**

```rust
#[test]
fn file_icon_uses_truecolor_when_enabled() {
    use colored::Color;
    let e = entry("src/foo.rs", FileStatus::Modified, true, 1, 0);
    let cs = colorize_icon('●', &e, 0.0, true);
    match cs.fgcolor {
        Some(Color::TrueColor { .. }) => {}
        other => panic!("expected TrueColor for icon under truecolor=true, got {other:?}"),
    }
}

#[test]
fn file_icon_falls_back_to_ansi_without_truecolor() {
    // Staged-modified icon today is plain green. Regression guard.
    use colored::Color;
    let e = entry("src/foo.rs", FileStatus::Modified, true, 1, 0);
    let cs = colorize_icon('●', &e, 0.0, false);
    assert_eq!(cs.fgcolor, Some(Color::Green));
}

#[test]
fn file_icon_darkens_with_age_under_truecolor() {
    use colored::Color;
    let e = entry("src/foo.rs", FileStatus::Modified, true, 1, 0);
    let fresh = colorize_icon('●', &e, 0.0, true);
    let aged = colorize_icon('●', &e, 1.0, true);
    let (Some(Color::TrueColor { r: fr, .. }), Some(Color::TrueColor { r: ar, .. })) =
        (fresh.fgcolor, aged.fgcolor)
    else {
        panic!("both should be TrueColor");
    };
    assert!(ar < fr, "aged icon should be darker: fresh={fr} aged={ar}");
}
```

- [ ] **Step 3.2: Add the deliberately-wrong stub and palette constants**

```rust
const FILE_ICON_STAGED_RGB: (u8, u8, u8) = (90, 220, 110);
const FILE_ICON_UNSTAGED_RGB: (u8, u8, u8) = (220, 200, 100);
const FILE_ICON_UNTRACKED_RGB: (u8, u8, u8) = (120, 200, 200);
const FILE_ICON_CONFLICT_RGB: (u8, u8, u8) = (255, 80, 80);
```

Change `colorize_icon`'s signature and stub:

```rust
fn colorize_icon(
    icon: char,
    entry: &RenderEntry,
    _factor: f32,
    truecolor: bool,
) -> ColoredString {
    let s = icon.to_string();
    if truecolor {
        // Wrong stub — constant grey breaks the gradient assertion only.
        return s.truecolor(50, 50, 50);
    }
    match entry.status {
        FileStatus::Conflicted => s.red().bold(),
        FileStatus::Untracked | FileStatus::UntrackedDir => s.cyan().dimmed(),
        _ if entry.staged => s.green(),
        _ => s.yellow(),
    }
}
```

Update the sole call site in `render_row`:

```rust
let icon_str = colorize_icon(icon, entry, 0.0, false);
```

- [ ] **Step 3.3: Run the tests and verify red tests fail behaviorally**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected:
- `file_icon_uses_truecolor_when_enabled` — PASS (stub returns TrueColor).
- `file_icon_falls_back_to_ansi_without_truecolor` — PASS.
- `file_icon_darkens_with_age_under_truecolor` — FAIL.

- [ ] **Step 3.4: Commit the red**

```bash
git add src/gsw/src/render.rs
git commit --no-verify -m "gsw: red — colorize_icon truecolor branch and palette

Stub returns a constant grey so only the gradient test fails."
```

### Green

- [ ] **Step 3.5: Replace the stub with the real fade**

```rust
fn colorize_icon(
    icon: char,
    entry: &RenderEntry,
    factor: f32,
    truecolor: bool,
) -> ColoredString {
    let s = icon.to_string();
    if truecolor {
        let base = match entry.status {
            FileStatus::Conflicted => FILE_ICON_CONFLICT_RGB,
            FileStatus::Untracked | FileStatus::UntrackedDir => FILE_ICON_UNTRACKED_RGB,
            _ if entry.staged => FILE_ICON_STAGED_RGB,
            _ => FILE_ICON_UNSTAGED_RGB,
        };
        let (r, g, b) = fade_rgb(base, factor);
        return s.truecolor(r, g, b);
    }
    match entry.status {
        FileStatus::Conflicted => s.red().bold(),
        FileStatus::Untracked | FileStatus::UntrackedDir => s.cyan().dimmed(),
        _ if entry.staged => s.green(),
        _ => s.yellow(),
    }
}
```

- [ ] **Step 3.6: Run the tests and verify they pass**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: all pass.

- [ ] **Step 3.7: Commit the green**

```bash
git add src/gsw/src/render.rs
git commit -m "gsw: green — colorize_icon fades per-status base RGB by factor"
```

---

## Phase 4: Letter-column truecolor fade

**Files:**
- Modify: `src/gsw/src/render.rs` — add letter RGB constants and a truecolor branch to `colorize_letter` (currently `render.rs:315–325`).

### Red

- [ ] **Step 4.1: Write the failing tests**

```rust
#[test]
fn file_letter_uses_truecolor_when_enabled() {
    use colored::Color;
    let e = entry("src/foo.rs", FileStatus::Added, true, 1, 0);
    let cs = colorize_letter('A', &e, 0.0, true);
    match cs.fgcolor {
        Some(Color::TrueColor { .. }) => {}
        other => panic!("expected TrueColor under truecolor=true, got {other:?}"),
    }
}

#[test]
fn file_letter_falls_back_to_ansi_without_truecolor() {
    use colored::Color;
    let e = entry("src/foo.rs", FileStatus::Added, true, 1, 0);
    let cs = colorize_letter('A', &e, 0.0, false);
    assert_eq!(cs.fgcolor, Some(Color::Green));
}

#[test]
fn file_letter_darkens_with_age_under_truecolor() {
    use colored::Color;
    let e = entry("src/foo.rs", FileStatus::Deleted, true, 0, 1);
    let fresh = colorize_letter('D', &e, 0.0, true);
    let aged = colorize_letter('D', &e, 1.0, true);
    let (Some(Color::TrueColor { r: fr, .. }), Some(Color::TrueColor { r: ar, .. })) =
        (fresh.fgcolor, aged.fgcolor)
    else { panic!("both should be TrueColor") };
    assert!(ar < fr, "aged letter should be darker: fresh={fr} aged={ar}");
}
```

- [ ] **Step 4.2: Add the deliberately-wrong stub and palette constants**

```rust
const FILE_LETTER_ADDED_RGB: (u8, u8, u8) = (90, 220, 110);
const FILE_LETTER_DELETED_RGB: (u8, u8, u8) = (255, 80, 80);
const FILE_LETTER_RENAMED_RGB: (u8, u8, u8) = (220, 120, 220);
const FILE_LETTER_DEFAULT_RGB: (u8, u8, u8) = (230, 230, 230);
const FILE_LETTER_CONFLICT_RGB: (u8, u8, u8) = (255, 80, 80);
const FILE_LETTER_UNTRACKED_RGB: (u8, u8, u8) = (120, 200, 200);
```

Change `colorize_letter`'s signature with the wrong stub:

```rust
fn colorize_letter(
    letter: char,
    entry: &RenderEntry,
    _factor: f32,
    truecolor: bool,
) -> ColoredString {
    let s = letter.to_string();
    if truecolor {
        return s.truecolor(50, 50, 50);
    }
    match entry.status {
        FileStatus::Conflicted => s.red().bold(),
        FileStatus::Untracked | FileStatus::UntrackedDir => s.cyan().dimmed(),
        FileStatus::Added => s.green().bold(),
        FileStatus::Deleted => s.red().bold(),
        FileStatus::Renamed | FileStatus::Copied => s.magenta().bold(),
        _ => s.bold(),
    }
}
```

Update the sole call site in `render_row`:

```rust
let letter_str = colorize_letter(letter, entry, 0.0, false);
```

- [ ] **Step 4.3: Run tests and confirm only the gradient test fails**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: `file_letter_darkens_with_age_under_truecolor` FAILS; others PASS.

- [ ] **Step 4.4: Commit the red**

```bash
git add src/gsw/src/render.rs
git commit --no-verify -m "gsw: red — colorize_letter truecolor branch and palette"
```

### Green

- [ ] **Step 4.5: Replace the stub with the real fade**

```rust
fn colorize_letter(
    letter: char,
    entry: &RenderEntry,
    factor: f32,
    truecolor: bool,
) -> ColoredString {
    let s = letter.to_string();
    if truecolor {
        let base = match entry.status {
            FileStatus::Conflicted => FILE_LETTER_CONFLICT_RGB,
            FileStatus::Untracked | FileStatus::UntrackedDir => FILE_LETTER_UNTRACKED_RGB,
            FileStatus::Added => FILE_LETTER_ADDED_RGB,
            FileStatus::Deleted => FILE_LETTER_DELETED_RGB,
            FileStatus::Renamed | FileStatus::Copied => FILE_LETTER_RENAMED_RGB,
            _ => FILE_LETTER_DEFAULT_RGB,
        };
        let (r, g, b) = fade_rgb(base, factor);
        return s.truecolor(r, g, b);
    }
    match entry.status {
        FileStatus::Conflicted => s.red().bold(),
        FileStatus::Untracked | FileStatus::UntrackedDir => s.cyan().dimmed(),
        FileStatus::Added => s.green().bold(),
        FileStatus::Deleted => s.red().bold(),
        FileStatus::Renamed | FileStatus::Copied => s.magenta().bold(),
        _ => s.bold(),
    }
}
```

- [ ] **Step 4.6: Run the tests and verify they pass**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: all pass.

- [ ] **Step 4.7: Commit the green**

```bash
git add src/gsw/src/render.rs
git commit -m "gsw: green — colorize_letter fades per-status base RGB by factor"
```

---

## Phase 5: Age-column truecolor fade for file rows

**Files:**
- Modify: `src/gsw/src/render.rs` — extend `colorize_age` with a truecolor branch (currently `render.rs:381–391`).

### Red

- [ ] **Step 5.1: Write the failing tests**

```rust
#[test]
fn file_age_uses_truecolor_when_enabled() {
    use colored::Color;
    let cs = colorize_age("5m23s", Some(Duration::from_secs(5 * 60 + 23)), 0.0, true);
    match cs.fgcolor {
        Some(Color::TrueColor { .. }) => {}
        other => panic!("expected TrueColor for file age, got {other:?}"),
    }
}

#[test]
fn file_age_falls_back_to_dim_buckets_without_truecolor() {
    // 8-color fallback must still bold a fresh row's age, matching today.
    use colored::Styles;
    let fresh = colorize_age("30s", Some(Duration::from_secs(30)), 0.0, false);
    assert!(
        fresh.style.contains(Styles::Bold),
        "fresh age should still be bolded in the 8-color path",
    );
}

#[test]
fn file_age_darkens_with_factor_under_truecolor() {
    use colored::Color;
    let fresh = colorize_age("30s", Some(Duration::from_secs(30)), 0.0, true);
    let aged = colorize_age("3d0h", Some(Duration::from_secs(3 * 86400)), 1.0, true);
    let (Some(Color::TrueColor { r: fr, .. }), Some(Color::TrueColor { r: ar, .. })) =
        (fresh.fgcolor, aged.fgcolor)
    else { panic!("both should be TrueColor") };
    assert!(ar < fr, "aged file-age column should be darker: fresh={fr} aged={ar}");
}
```

- [ ] **Step 5.2: Add the deliberately-wrong stub**

Add this constant:

```rust
const FILE_AGE_RGB: (u8, u8, u8) = (190, 190, 190);
```

Change `colorize_age`'s signature and stub the truecolor branch:

```rust
fn colorize_age(
    text: &str,
    age: Option<Duration>,
    _factor: f32,
    truecolor: bool,
) -> ColoredString {
    if truecolor {
        return text.truecolor(50, 50, 50);
    }
    let Some(age) = age else {
        return text.dimmed();
    };
    match age_dim_level(age) {
        AgeDim::Fresh => text.bold(),
        AgeDim::Recent => text.normal(),
        AgeDim::Aging => text.dimmed(),
        AgeDim::Stale => text.dimmed().italic(),
    }
}
```

Update the two call sites in `render_row` (one in the untracked branch around `render.rs:237`, one in the main branch around `render.rs:274`):

```rust
let age_str = colorize_age(&age_field, entry.age, 0.0, false);
```

Also update the existing log-section helper `colorize_log_age` (currently `render.rs:442–448`), which calls `colorize_age` with the legacy two-arg shape:

```rust
fn colorize_log_age(text: &str, age: Duration, truecolor: bool) -> ColoredString {
    if truecolor {
        fade_truecolor(text, age, LOG_AGE_BASE_RGB)
    } else {
        colorize_age(text, Some(age), 0.0, false)
    }
}
```

- [ ] **Step 5.3: Run tests and confirm only the gradient test fails**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: `file_age_darkens_with_factor_under_truecolor` FAILS; others PASS. The existing `stale_age_renders_differently_from_aging` test uses `colorize_age(..., Some(d))`, so update the call sites in tests too — search the file for `colorize_age("12h0m"` and other test invocations, append `, 0.0, false`. (Both test call sites today live around `render.rs:1284–1285`.)

- [ ] **Step 5.4: Commit the red**

```bash
git add src/gsw/src/render.rs
git commit --no-verify -m "gsw: red — colorize_age truecolor branch and FILE_AGE_RGB"
```

### Green

- [ ] **Step 5.5: Replace the stub with the real fade**

```rust
fn colorize_age(
    text: &str,
    age: Option<Duration>,
    factor: f32,
    truecolor: bool,
) -> ColoredString {
    if truecolor {
        let (r, g, b) = fade_rgb(FILE_AGE_RGB, factor);
        return text.truecolor(r, g, b);
    }
    let Some(age) = age else {
        return text.dimmed();
    };
    match age_dim_level(age) {
        AgeDim::Fresh => text.bold(),
        AgeDim::Recent => text.normal(),
        AgeDim::Aging => text.dimmed(),
        AgeDim::Stale => text.dimmed().italic(),
    }
}
```

- [ ] **Step 5.6: Run the tests and verify they pass**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: all pass.

- [ ] **Step 5.7: Commit the green**

```bash
git add src/gsw/src/render.rs
git commit -m "gsw: green — colorize_age fades FILE_AGE_RGB by factor"
```

---

## Phase 6: Adds / Dels truecolor fade

**Files:**
- Modify: `src/gsw/src/render.rs` — extract two new fns (`colorize_adds`, `colorize_dels`) from the inline `+adds` / `-dels` styling in `render_row` (currently `render.rs:261–270`).

### Red

- [ ] **Step 6.1: Write the failing tests**

```rust
#[test]
fn file_adds_uses_truecolor_when_enabled() {
    use colored::Color;
    let cs = colorize_adds("  +12", 0.0, true);
    match cs.fgcolor {
        Some(Color::TrueColor { .. }) => {}
        other => panic!("expected TrueColor, got {other:?}"),
    }
}

#[test]
fn file_dels_uses_truecolor_when_enabled() {
    use colored::Color;
    let cs = colorize_dels(" -3", 0.0, true);
    match cs.fgcolor {
        Some(Color::TrueColor { .. }) => {}
        other => panic!("expected TrueColor, got {other:?}"),
    }
}

#[test]
fn file_adds_falls_back_to_green_without_truecolor() {
    use colored::Color;
    let cs = colorize_adds("  +12", 0.0, false);
    assert_eq!(cs.fgcolor, Some(Color::Green));
}

#[test]
fn file_dels_falls_back_to_red_without_truecolor() {
    use colored::Color;
    let cs = colorize_dels(" -3", 0.0, false);
    assert_eq!(cs.fgcolor, Some(Color::Red));
}

#[test]
fn file_adds_darkens_with_factor_under_truecolor() {
    use colored::Color;
    let fresh = colorize_adds("  +12", 0.0, true);
    let aged = colorize_adds("  +12", 1.0, true);
    let (Some(Color::TrueColor { r: fr, g: fg, .. }),
         Some(Color::TrueColor { r: ar, g: ag, .. })) =
        (fresh.fgcolor, aged.fgcolor)
    else { panic!("both should be TrueColor") };
    assert!(ar < fr || ag < fg, "aged +adds should be darker");
}
```

- [ ] **Step 6.2: Add the constants and stub fns**

```rust
const FILE_ADDS_RGB: (u8, u8, u8) = (90, 220, 110);
const FILE_DELS_RGB: (u8, u8, u8) = (255, 90, 90);

fn colorize_adds(text: &str, _factor: f32, truecolor: bool) -> ColoredString {
    if truecolor {
        return text.truecolor(50, 50, 50);
    }
    text.green()
}

fn colorize_dels(text: &str, _factor: f32, truecolor: bool) -> ColoredString {
    if truecolor {
        return text.truecolor(50, 50, 50);
    }
    text.red()
}
```

Replace the inline blocks in `render_row` (around `render.rs:261–270`):

```rust
let adds_str = if entry.adds > 0 {
    colorize_adds(&adds_field, 0.0, false).to_string()
} else {
    adds_field
};
let dels_str = if entry.dels > 0 {
    colorize_dels(&dels_field, 0.0, false).to_string()
} else {
    dels_field
};
```

- [ ] **Step 6.3: Run tests and confirm only the gradient test fails**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: `file_adds_darkens_with_factor_under_truecolor` FAILS; everything else PASSES.

- [ ] **Step 6.4: Commit the red**

```bash
git add src/gsw/src/render.rs
git commit --no-verify -m "gsw: red — extract colorize_adds/colorize_dels with truecolor stub"
```

### Green

- [ ] **Step 6.5: Replace the stubs with the real fade**

```rust
fn colorize_adds(text: &str, factor: f32, truecolor: bool) -> ColoredString {
    if truecolor {
        let (r, g, b) = fade_rgb(FILE_ADDS_RGB, factor);
        return text.truecolor(r, g, b);
    }
    text.green()
}

fn colorize_dels(text: &str, factor: f32, truecolor: bool) -> ColoredString {
    if truecolor {
        let (r, g, b) = fade_rgb(FILE_DELS_RGB, factor);
        return text.truecolor(r, g, b);
    }
    text.red()
}
```

- [ ] **Step 6.6: Run the tests and verify they pass**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: all pass.

- [ ] **Step 6.7: Commit the green**

```bash
git add src/gsw/src/render.rs
git commit -m "gsw: green — colorize_adds / colorize_dels fade by factor"
```

---

## Phase 7: Bar truecolor fade (foreground + partial-cell background)

**Files:**
- Modify: `src/gsw/src/render.rs` — extend `colorize_bar` (currently `render.rs:345–372`) with a truecolor branch that fades both the fill foreground and the partial-cell background by the same factor.

### Red

- [ ] **Step 7.1: Write the failing tests**

```rust
#[test]
fn file_bar_fill_fades_with_factor_under_truecolor() {
    use colored::Color;
    let e = entry("foo.rs", FileStatus::Modified, true, 6, 0);
    let fresh = colorize_bar_styled("██████", &e, 0.0, true);
    let aged = colorize_bar_styled("██████", &e, 1.0, true);
    // We expect the first cell's fg to be TrueColor in both cases and
    // the aged channel to be strictly lower.
    let (Some(Color::TrueColor { r: fr, g: fg, b: fb }),
         Some(Color::TrueColor { r: ar, g: ag, b: ab })) =
        (fresh[0].fgcolor, aged[0].fgcolor)
    else { panic!("first cell should be TrueColor under truecolor=true") };
    assert!(
        ar < fr || ag < fg || ab < fb,
        "aged bar fill should be darker on at least one channel",
    );
}

#[test]
fn file_bar_partial_bg_fades_with_factor_under_truecolor() {
    use colored::Color;
    // Use a partial-fill glyph (▍ = U+258D) so a background color is set.
    let e = entry("foo.rs", FileStatus::Modified, true, 6, 0);
    let fresh = colorize_bar_styled("▍", &e, 0.0, true);
    let aged = colorize_bar_styled("▍", &e, 1.0, true);
    let (Some(Color::TrueColor { r: fr, .. }), Some(Color::TrueColor { r: ar, .. })) =
        (fresh[0].bgcolor, aged[0].bgcolor)
    else { panic!("partial cell should have a TrueColor background") };
    assert!(ar < fr, "aged partial-cell bg should be darker: fresh={fr} aged={ar}");
}

#[test]
fn file_bar_fallback_unchanged_without_truecolor() {
    // 8-color path returns the cyan-fill bytes today. Regression guard.
    let e = entry("foo.rs", FileStatus::Modified, true, 6, 0);
    let cells = colorize_bar_styled("█", &e, 0.0, false);
    use colored::Color;
    assert_eq!(cells[0].fgcolor, Some(Color::Cyan));
}
```

These reference a new helper `colorize_bar_styled` that returns the per-cell `ColoredString`s instead of the joined `String` the current `colorize_bar` produces. We're adding it specifically to make the typed-color inspection from the tests above work without parsing ANSI bytes.

- [ ] **Step 7.2: Add the new helper and stub the truecolor branch wrong**

Add the constants if not already present (`BAR_PARTIAL_BG_CYAN` and `BAR_PARTIAL_BG_RED` already exist at `render.rs:341–344`).

Add this near `colorize_bar`:

```rust
/// Build one `ColoredString` per visible cell of `bar`. The joined string
/// returned by [`colorize_bar`] is just `colorize_bar_styled(...).join("")`
/// with `.to_string()` applied to each cell — sharing the cell builder lets
/// tests inspect the typed fg/bg colors per cell instead of parsing ANSI.
fn colorize_bar_styled(
    bar: &str,
    entry: &RenderEntry,
    _factor: f32,
    truecolor: bool,
) -> Vec<ColoredString> {
    if entry.binary {
        return bar.chars().map(|c| c.to_string().dimmed()).collect();
    }
    let is_conflicted = matches!(entry.status, FileStatus::Conflicted);
    let (br, bg, bb) = if is_conflicted {
        BAR_PARTIAL_BG_RED
    } else {
        BAR_PARTIAL_BG_CYAN
    };
    bar.chars()
        .map(|c| {
            let s = c.to_string();
            if truecolor {
                // Wrong stub — constant grey breaks gradient + bg tests only.
                if is_partial_block(c) {
                    s.truecolor(50, 50, 50).on_truecolor(20, 20, 20)
                } else {
                    s.truecolor(50, 50, 50)
                }
            } else if is_partial_block(c) {
                if is_conflicted {
                    s.red().on_truecolor(br, bg, bb)
                } else {
                    s.cyan().on_truecolor(br, bg, bb)
                }
            } else if is_conflicted {
                s.red()
            } else {
                s.cyan()
            }
        })
        .collect()
}
```

Refactor `colorize_bar` to delegate:

```rust
fn colorize_bar(bar: &str, entry: &RenderEntry, factor: f32, truecolor: bool) -> String {
    let cells = colorize_bar_styled(bar, entry, factor, truecolor);
    let mut out = String::with_capacity(bar.len() * 2);
    for c in cells {
        out.push_str(&c.to_string());
    }
    out
}
```

Update the call site in `render_row` (around `render.rs:246`):

```rust
let bar_str = colorize_bar(&bar_raw, entry, 0.0, false);
```

Also update the existing `partial_cell_gets_background_to_close_gap` test (around `render.rs:983`) to use the new signature: `colorize_bar("█████▍", &e, 0.0, false)`.

- [ ] **Step 7.3: Run tests and confirm only the gradient + bg tests fail**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: `file_bar_fill_fades_with_factor_under_truecolor` and `file_bar_partial_bg_fades_with_factor_under_truecolor` FAIL; everything else PASSES.

- [ ] **Step 7.4: Commit the red**

```bash
git add src/gsw/src/render.rs
git commit --no-verify -m "gsw: red — colorize_bar truecolor branch and per-cell helper"
```

### Green

- [ ] **Step 7.5: Replace the stub with the real fade**

Add this constant alongside the other file palette entries:

```rust
const FILE_BAR_RGB: (u8, u8, u8) = (60, 200, 200);
const FILE_BAR_CONFLICT_RGB: (u8, u8, u8) = (255, 80, 80);
```

Replace `colorize_bar_styled`'s truecolor branch:

```rust
fn colorize_bar_styled(
    bar: &str,
    entry: &RenderEntry,
    factor: f32,
    truecolor: bool,
) -> Vec<ColoredString> {
    if entry.binary {
        return bar.chars().map(|c| c.to_string().dimmed()).collect();
    }
    let is_conflicted = matches!(entry.status, FileStatus::Conflicted);
    let (bg_br, bg_bg, bg_bb) = if is_conflicted {
        BAR_PARTIAL_BG_RED
    } else {
        BAR_PARTIAL_BG_CYAN
    };
    bar.chars()
        .map(|c| {
            let s = c.to_string();
            if truecolor {
                let fg_base = if is_conflicted {
                    FILE_BAR_CONFLICT_RGB
                } else {
                    FILE_BAR_RGB
                };
                let (fr, fg, fb) = fade_rgb(fg_base, factor);
                if is_partial_block(c) {
                    let (pr, pg, pb) = fade_rgb((bg_br, bg_bg, bg_bb), factor);
                    s.truecolor(fr, fg, fb).on_truecolor(pr, pg, pb)
                } else {
                    s.truecolor(fr, fg, fb)
                }
            } else if is_partial_block(c) {
                if is_conflicted {
                    s.red().on_truecolor(bg_br, bg_bg, bg_bb)
                } else {
                    s.cyan().on_truecolor(bg_br, bg_bg, bg_bb)
                }
            } else if is_conflicted {
                s.red()
            } else {
                s.cyan()
            }
        })
        .collect()
}
```

- [ ] **Step 7.6: Run the tests and verify they pass**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: all pass.

- [ ] **Step 7.7: Commit the green**

```bash
git add src/gsw/src/render.rs
git commit -m "gsw: green — colorize_bar fades fill fg and partial-cell bg together

Both the bright cyan (or red, for conflicts) fill and the dim
partial-cell background fade by the same factor so the bar darkens
uniformly across the row."
```

---

## Phase 8: Thread `truecolor` + `factor` through `render_row`

**Files:**
- Modify: `src/gsw/src/render.rs` — `render_row` (currently `render.rs:207–283`) computes `let factor = file_fade_factor(entry.age);` once and passes `(factor, opts.truecolor)` into every `colorize_*` call. After this phase, file rows actually fade end-to-end in truecolor mode.

### Red

- [ ] **Step 8.1: Write the integration-shaped failing tests**

These exercise `render_row` indirectly through `render` so they prove the wiring actually goes from `RenderOptions.truecolor` all the way to per-cell color output.

```rust
#[test]
fn file_row_renders_with_truecolor_when_enabled() {
    use colored::Color;
    // Force the colored crate to actually emit ANSI in the test process so
    // we can detect the truecolor codes from the rendered output.
    colored::control::set_override(true);
    let snap = snap_with(vec![entry("src/foo.rs", FileStatus::Modified, false, 5, 2)]);
    let mut o = opts();
    o.truecolor = true;
    let out = render(&snap, &o);
    colored::control::unset_override();
    // Truecolor foreground sequences start with `\x1b[38;2;`.
    assert!(
        out.contains("\x1b[38;2;"),
        "rendered file row should contain a truecolor ANSI sequence when truecolor=true",
    );
}

#[test]
fn file_row_no_truecolor_in_8_color_mode() {
    colored::control::set_override(true);
    let snap = snap_with(vec![entry("src/foo.rs", FileStatus::Modified, false, 5, 2)]);
    let out = render(&snap, &opts());
    colored::control::unset_override();
    assert!(
        !out.contains("\x1b[38;2;"),
        "8-color mode must not emit any truecolor sequences for file rows",
    );
}

#[test]
fn file_row_darkens_with_mtime_under_truecolor() {
    // End-to-end: an older file's row should contain a darker (lower-channel)
    // truecolor sequence than a fresher row of the same status.
    colored::control::set_override(true);
    let mut fresh_entry = entry("src/foo.rs", FileStatus::Modified, false, 5, 2);
    fresh_entry.age = Some(Duration::from_secs(0));
    let mut aged_entry = entry("src/bar.rs", FileStatus::Modified, false, 5, 2);
    aged_entry.age = Some(Duration::from_secs(60 * 60));

    let fresh_snap = snap_with(vec![fresh_entry]);
    let aged_snap = snap_with(vec![aged_entry]);
    let mut o = opts();
    o.truecolor = true;
    let fresh_out = render(&fresh_snap, &o);
    let aged_out = render(&aged_snap, &o);
    colored::control::unset_override();

    let max_r = |s: &str| {
        // Extract the largest r-channel from any 38;2;r;g;b foreground sequence.
        let mut best: Option<u8> = None;
        let bytes = s.as_bytes();
        let needle = b"\x1b[38;2;";
        let mut i = 0;
        while let Some(pos) = bytes[i..].windows(needle.len()).position(|w| w == needle) {
            let start = i + pos + needle.len();
            // Read r digits.
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > start {
                if let Ok(r) = std::str::from_utf8(&bytes[start..j]).unwrap().parse::<u8>() {
                    best = Some(best.map_or(r, |b| b.max(r)));
                }
            }
            i = j;
        }
        best.expect("at least one truecolor sequence")
    };

    let fresh_max = max_r(&fresh_out);
    let aged_max = max_r(&aged_out);
    assert!(
        aged_max < fresh_max,
        "aged row's brightest channel should be lower than fresh row's: fresh={fresh_max} aged={aged_max}",
    );
}

#[test]
fn file_row_no_age_renders_at_floor_under_truecolor() {
    // Deleted file (age=None) should produce only sequences with channels
    // at or below the FADE_FLOOR fraction of their base.
    use crate::age::FADE_FLOOR;
    colored::control::set_override(true);
    let mut e = entry("deleted.rs", FileStatus::Deleted, true, 0, 5);
    e.age = None;
    let snap = snap_with(vec![e]);
    let mut o = opts();
    o.truecolor = true;
    let out = render(&snap, &o);
    colored::control::unset_override();

    // The brightest channel allowed at the floor is `255 × FADE_FLOOR`
    // (a base channel of 255 hits the highest floor). Use that as the
    // conservative upper bound for any column, plus a small slack for
    // rounding. Any row column emitting a channel above this means a
    // colorize_* fn forgot to apply the fade.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "255.0 × FADE_FLOOR ∈ [0, 255]"
    )]
    let upper = ((255.0_f32 * FADE_FLOOR).round() as u8).saturating_add(2);

    // Parse every r-channel as before and assert all are <= upper.
    let bytes = out.as_bytes();
    let needle = b"\x1b[38;2;";
    let mut i = 0;
    let mut saw_any = false;
    while let Some(pos) = bytes[i..].windows(needle.len()).position(|w| w == needle) {
        let start = i + pos + needle.len();
        let mut j = start;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > start {
            let r: u8 = std::str::from_utf8(&bytes[start..j]).unwrap().parse().unwrap();
            assert!(
                r <= upper,
                "every channel on a no-age row should sit at or below the floor (got {r}, upper {upper})",
            );
            saw_any = true;
        }
        i = j;
    }
    assert!(saw_any, "should have emitted at least one truecolor sequence");
}
```

- [ ] **Step 8.2: Verify tests fail with the current `render_row` (still passing `0.0, false` everywhere)**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: `file_row_renders_with_truecolor_when_enabled`, `file_row_darkens_with_mtime_under_truecolor`, and `file_row_no_age_renders_at_floor_under_truecolor` FAIL; `file_row_no_truecolor_in_8_color_mode` PASSES (no truecolor anywhere yet).

- [ ] **Step 8.3: Commit the red**

```bash
git add src/gsw/src/render.rs
git commit --no-verify -m "gsw: red — end-to-end file-row truecolor wiring tests

Render-level tests that detect ANSI 38;2; (truecolor) sequences in the
rendered output, confirming the wiring goes RenderOptions.truecolor
through to per-cell color emission once Phase 8 hooks it up."
```

### Green

- [ ] **Step 8.4: Wire `factor` + `opts.truecolor` through `render_row`**

In `src/gsw/src/render.rs`, replace the body of `render_row` so it threads the new values into every colorize call:

```rust
fn render_row(
    entry: &RenderEntry,
    opts: &RenderOptions,
    max_change: u32,
    path_width: usize,
) -> String {
    let (icon, letter) = icon_and_letter(entry);
    let factor = file_fade_factor(entry.age);
    let tc = opts.truecolor;

    let path_display_raw = match &entry.orig_path {
        Some(orig) => format!("{orig} → {new}", new = entry.path),
        None => entry.path.clone(),
    };
    let path_truncated = truncate_left(&path_display_raw, path_width);
    let path_padded = pad_right(&path_truncated, path_width);

    let icon_str = colorize_icon(icon, entry, factor, tc);
    let letter_str = colorize_letter(letter, entry, factor, tc);
    let path_str = colorize_path(&path_padded, entry, factor, tc);

    if matches!(
        entry.status,
        FileStatus::Untracked | FileStatus::UntrackedDir
    ) {
        let gutter_width = right_block_width(opts.bar_width) - AGE_FIELD;
        let gutter = " ".repeat(gutter_width);
        let age = entry.age.map(format_age_detailed).unwrap_or_default();
        let age_field = format!("{age:>width$}", width = AGE_FIELD);
        let age_str = colorize_age(&age_field, entry.age, factor, tc);
        return format!("{icon_str} {letter_str} {path_str}{gutter}{age_str}");
    }

    let bar_raw = if entry.binary {
        center("bin", opts.bar_width)
    } else {
        render_bar(entry.adds.saturating_add(entry.dels), max_change, opts.bar_width)
    };
    let bar_str = colorize_bar(&bar_raw, entry, factor, tc);

    let adds_raw = if entry.adds > 0 {
        format!("+{}", entry.adds)
    } else {
        String::new()
    };
    let dels_raw = if entry.dels > 0 {
        format!("-{}", entry.dels)
    } else {
        String::new()
    };
    let adds_field = format!("{adds_raw:>width$}", width = ADDS_FIELD);
    let dels_field = format!("{dels_raw:>width$}", width = DELS_FIELD);

    let adds_str = if entry.adds > 0 {
        colorize_adds(&adds_field, factor, tc).to_string()
    } else {
        adds_field
    };
    let dels_str = if entry.dels > 0 {
        colorize_dels(&dels_field, factor, tc).to_string()
    } else {
        dels_field
    };

    let age_raw = entry.age.map(format_age_detailed).unwrap_or_default();
    let age_field = format!("{age_raw:>width$}", width = AGE_FIELD);
    let age_str = colorize_age(&age_field, entry.age, factor, tc);

    let sep_bar_adds = " ".repeat(SEP_BAR_ADDS);
    let sep_adds_dels = " ".repeat(SEP_ADDS_DELS);
    let sep_dels_age = " ".repeat(SEP_DELS_AGE);

    format!(
        "{icon_str} {letter_str} {path_str}{bar_str}{sep_bar_adds}{adds_str}{sep_adds_dels}{dels_str}{sep_dels_age}{age_str}",
    )
}
```

- [ ] **Step 8.5: Run the tests and verify all pass**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml`

Expected: all tests pass, including the new end-to-end ones.

- [ ] **Step 8.6: Commit the green**

```bash
git add src/gsw/src/render.rs
git commit -m "gsw: green — thread factor + truecolor through render_row

file_fade_factor(entry.age) is computed once per row and passed into
every colorize_* call. End-to-end: enabling truecolor now fades file
rows from a per-status base RGB toward the dark floor as files age,
matching the commit-log section's gradient."
```

---

## Phase 9: Status-hue distinctness invariant test

A pure regression-style invariant: at the floor (worst case for hue washout), the icon RGBs for the five statuses must stay pairwise distinct, so users can still tell at-a-glance what a row is even at age ∞.

**Files:**
- Modify: `src/gsw/src/render.rs` — test-only addition.

### Red + Green

- [ ] **Step 9.1: Write the test**

```rust
#[test]
fn file_row_status_hues_remain_distinct_at_floor() {
    // At the dark floor, fading must not collapse different statuses into
    // the same RGB. We check icon base hues fade to distinct floored RGBs.
    let bases = [
        ("staged", FILE_ICON_STAGED_RGB),
        ("unstaged", FILE_ICON_UNSTAGED_RGB),
        ("untracked", FILE_ICON_UNTRACKED_RGB),
        ("conflict", FILE_ICON_CONFLICT_RGB),
    ];
    let floored: Vec<((u8,u8,u8), &str)> = bases
        .iter()
        .map(|(name, rgb)| (fade_rgb(*rgb, 1.0), *name))
        .collect();
    for i in 0..floored.len() {
        for j in (i + 1)..floored.len() {
            let ((ar, ag, ab), aname) = floored[i];
            let ((br, bg, bb), bname) = floored[j];
            // Manhattan distance > 0 isn't enough — we want a perceptible
            // difference even after the channels shrink. Require at least
            // 10 units of total channel difference.
            let dist = (i32::from(ar) - i32::from(br)).abs()
                + (i32::from(ag) - i32::from(bg)).abs()
                + (i32::from(ab) - i32::from(bb)).abs();
            assert!(
                dist >= 10,
                "{aname} and {bname} floored RGBs too close: {ar:?},{ag:?},{ab:?} vs {br:?},{bg:?},{bb:?} (dist {dist})",
            );
        }
    }
}
```

- [ ] **Step 9.2: Run the test**

Run: `cargo test --lib --manifest-path src/gsw/Cargo.toml file_row_status_hues_remain_distinct_at_floor`

This test should PASS on the first run because the constants in earlier phases were chosen to satisfy it. If it fails, the palette needs adjustment — fix the constants in `render.rs` and re-run.

Why no separate red commit? This test pins an *existing* invariant of the palette chosen in Phases 2–7. There's no implementation to write for it. The red/green discipline applies to *behavioral* tests for new code; this is a property-style guard that fails only when someone later picks confusing colors. Committing it on its own is fine.

- [ ] **Step 9.3: Commit**

```bash
git add src/gsw/src/render.rs
git commit -m "gsw: pin file-row status hue distinctness invariant at floor

Pairwise channel-distance check across the icon palette at FADE_FLOOR.
Guards against a future tweak that would collapse two statuses into
visually indistinguishable floored hues."
```

---

## Phase 10: Manual visual verification

This is the only step that can't be a unit test — make sure the change actually looks right under `viddy gsw` in a real terminal.

- [ ] **Step 10.1: Build the release binary**

Run from the repo root:

```bash
cargo build --release --manifest-path src/gsw/Cargo.toml
```

- [ ] **Step 10.2: Run `viddy gsw` in a terminal with truecolor**

In a `viddy gsw` (or `viddy --shell zsh "./target/release/gsw"`) session inside this very worktree — which has a mix of staged, unstaged, untracked, and (if you can manufacture them) deleted files — visually confirm:

1. Freshly-modified files render in bright color.
2. Hour-old modifications render visibly dimmer.
3. Day-old modifications render at the dark floor.
4. The bar fill darkens uniformly across the row.
5. Deleted files (if present) and untracked dirs render at the floor.
6. The file list and the commit-log section share one continuous brightness timeline — the eye can see them as one gradient.

If anything looks wrong, adjust the per-status base RGB constants in `render.rs` and re-run.

- [ ] **Step 10.3: (Optional) Run with `--no-truecolor` and confirm 8-color output is unchanged**

```bash
./target/release/gsw --no-truecolor
```

The output should match exactly what gsw produced before this branch landed for the file-row section — bucketed dim styling, no continuous gradient.

- [ ] **Step 10.4: No commit needed** unless tweaks were made. If tweaks were made, commit as a green follow-up using the same TDD discipline (write a quick test that pins the new value if it matters; otherwise commit as a tuning patch).

---

## Done criteria

When all checkboxes above are checked, every box in this list is true:

- [ ] `file_fade_factor` exists, with `Some(0)` → `0.0`, `None` → `1.0`, `Some(d)` → `age_fade_factor(d)`.
- [ ] Each of `colorize_icon`, `colorize_letter`, `colorize_path`, `colorize_bar`, `colorize_adds`, `colorize_dels`, `colorize_age` has a truecolor branch that fades a status-appropriate base RGB by the row's factor.
- [ ] `render_row` computes `file_fade_factor(entry.age)` once and threads `(factor, opts.truecolor)` to every colorize call.
- [ ] All new and existing tests pass under `cargo test --lib --manifest-path src/gsw/Cargo.toml`.
- [ ] Visual verification under `viddy gsw` confirms the file list shares the commit-log fade timeline.
- [ ] `gsw --no-truecolor` renders the file section identically to today.
