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

No defect is recorded yet. Give each entry these parts:

- the finding ID,
- the location in `src/termstate/src/lib.rs`,
- the reproduction: the screen size, the bytes, the result of `termstate`,
  and the result of xterm.
