## `src/termstate` — a margin defect older than branch issue-165 goes to one follow-up issue

**Settled** 2026-11-27 (branch issue-165, Tim's answer to finding R-20261127T134053Z#N1)

Tim decided the scope of the scrolling-region rules of `termstate` on this
branch:

- Finding R-20261127T134053Z#N1 is fixed on this branch. CUD, CNL, CUU and
  CPL stop at the margin of the scrolling region, as CursorDown and CursorUp
  in xterm's `cursor.c` (red `07afcf55`, green `105acde5`).
- From now on, a margin defect of `termstate` that is older than this branch
  is not fixed on this branch. A fix round gives its finding the disposition
  `declined`, and it adds the defect to the list below in the commit that the
  disposition names. Each entry gives the finding ID and a reproduction.
- The merge step of issue-fleet makes one follow-up issue from this file:
  "termstate: the scrolling-region rules that are older than issue #165". The
  body of PR #803 names that follow-up.

**The boundary.** A margin defect is a rule of DECSTBM, of the scrolling
region, or of a move or an edit at a margin, that is different in xterm. The
defect is older than this branch when `main` has its code before this branch.
A defect that this branch adds or changes is not older. Fix it on this branch.

## The follow-up issue

1. Fix each defect of the list below, with a red test and then a green commit.
2. Compare the other rules of the scrolling region in `termstate` with
   xterm's `cursor.c` and `charproc.c`. Fix each difference in the same way.

## Recorded defects

Give each entry these parts:

- the finding ID,
- the location in `src/termstate/src/lib.rs`,
- the reproduction: the screen size, the bytes, the result of `termstate`,
  and the result of xterm.

Each row number below starts at 0, as `scroll_region()` gives it.

### 1. RIS keeps the scrolling region

- **Finding:** R-20261127T195703Z#N1.
- **Location:** `src/termstate/src/lib.rs:1995-2015`, the arm `b'c'` (RIS) of
  the escape dispatch. The arm resets the pen, the attributes, autowrap, the
  pending wrap and the line-drawing set, but not `scroll_top` and
  `scroll_bottom`.
- **Reproduction:** a screen of 80 columns and 6 rows. The bytes are
  `ESC [ 2 ; 4 r`, `ESC c`, `ESC [ 4 ; 1 H` and `LF`. In `termstate`,
  `scroll_region()` gives `(1, 3)` after `ESC c`, and the line feed on row 3
  scrolls rows 1 to 3. In xterm, RIS sets the margins to the full screen, and
  the line feed moves the cursor to row 4.
- **Effect on the output of `nle`:** the restore of the viewport filter
  (`src/nle/src/filters/viewport.rs:510-515`) writes `scroll_region()` with
  DECSTBM. Thus a range after RIS writes the stale region to the real
  terminal.
- **Fix:** in the RIS arm, set `scroll_top` to 0 and `scroll_bottom` to the
  last row.
