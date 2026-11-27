## `specs/2026-11-26-nle-plugin-architecture-design.md` — the spec gives only the contract of the plugin log, and slice 2 designs the rest

**Settled** 2026-11-26 (branch issue-169, findings R-20261126T230540Z#I1,
R-20261126T230540Z#N1, R-20261126T230540Z#N2, R-20261126T231154Z#I1,
R-20261126T231154Z#N1, R-20261126T231154Z#N2)

Tim decided the scope through an answer to the stall of `review-loop` in the
issue-fleet run `F-20261126T155156Z`. The spec keeps only the part of the
plugin log that a plugin depends on:

- the `log` import, with its four levels,
- the bound of 64 KiB for each call, over `log`, standard output, and standard
  error together,
- what a plugin sees when it writes past that bound: the write returns with no
  error, the call continues, and the answer is the same as with no write. The
  `StdoutStream` of the host, the fix of R-20261126T225537Z#I1, stays.

The spec does not design the storage of the plugin log, its bound across calls,
the copy of the bytes of a call into it, or its display. Slice 2 builds the
log, thus it holds these items as one open question, and its pull request
records the answer.

Three rounds of review in a row found new defects in that design
(R-20261126T225537Z, R-20261126T230540Z, R-20261126T231154Z): a pipe that
failed the call, a log with no bound across calls, a cut character with no
test, a lost level, a log that nobody reads, two readings of one log, and a
wrong count of calls. Each fix made the design larger. The implementation of
slice 2 can answer these questions with code and a test, which a spec cannot.

The boundary: a finding that asks the spec to design one of the four removed
items is settled by this record. A defect in the kept contract is not settled,
for example a write past the bound that fails, waits, or changes the answer.
An open question of slice 2 that leaves out one of the four items is a new
occurrence, not a new argument.
