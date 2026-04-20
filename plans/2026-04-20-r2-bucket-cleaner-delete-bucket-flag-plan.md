# Plan: r2-bucket-cleaner `--delete-bucket` flag

> Source PRD: [specs/2026-04-20-r2-bucket-cleaner-delete-bucket-flag-design.md](../specs/2026-04-20-r2-bucket-cleaner-delete-bucket-flag-design.md)

## Architectural decisions

Durable decisions that apply across all phases:

- **Flag surface**: `--delete-bucket` / `-d` on `r2-bucket-cleaner`, mutually exclusive with `--list-only` via clap `conflicts_with`. `--force` suppresses all confirms including the bucket-delete confirm.
- **Backend**: continue shelling out to `wrangler` via `task::spawn_blocking` + `Command::new`, matching the existing `r2_wrangler.rs` pattern. No migration to `aws-sdk-s3`.
- **Wrangler argv**: `["r2", "bucket", "delete", "<bucket>"]`, built by a pure `delete_bucket_argv(bucket: &str) -> Vec<String>` helper that is unit-testable without executing wrangler.
- **Failure policy**: if emptying fails, exit non-zero *before* attempting bucket delete. If bucket delete fails, propagate stderr and exit non-zero — no retry.
- **Testing**: TDD red-green-refactor with separate commits per phase. Red commits may use `--no-verify` since the failing test will fail the pre-commit hook; nothing unrelated goes into the red commit.

---

## Phase 1: Delete an already-empty bucket with `-d --force`

**User stories**:

- As a user, I can run `r2-bucket-cleaner <bucket> -d --force` on an already-empty bucket and have the bucket removed in a single command.
- As a user, `--list-only` combined with `--delete-bucket` is rejected at parse time with a clear conflict error.

### What to build

The thinnest end-to-end slice: a user with `--force` can delete an empty bucket in one invocation. This phase exercises every layer touched by the feature — flag parsing, the new wrangler helper, and the main loop's early-return path — but deliberately skips interactive prompts and the non-empty workflow. After this phase, `r2-bucket-cleaner empty-bucket -d --force` should call `wrangler r2 bucket delete empty-bucket` and print `✅ Bucket '<name>' deleted.` on success.

TDD sequence per change (red commit → green commit):

1. Clap parser tests: `-d` parses, `--delete-bucket` parses, `--list-only --delete-bucket` returns a clap conflict error.
2. `delete_bucket_argv` unit tests: basic name and names with `-`, `.`, digits pass through verbatim.
3. `R2WranglerClient::delete_bucket` method wired to the helper (not unit-tested against real wrangler — the argv helper is the seam).
4. Main-loop wiring: on pass 1, if `keys.is_empty() && args.delete_bucket && args.force`, call `delete_bucket` and print the success line before returning.

### Acceptance criteria

- [ ] `Args::try_parse_from(["r2-bucket-cleaner", "b", "-d"])` succeeds with `delete_bucket == true`.
- [ ] `Args::try_parse_from(["r2-bucket-cleaner", "b", "--delete-bucket"])` succeeds with `delete_bucket == true`.
- [ ] `Args::try_parse_from(["r2-bucket-cleaner", "b", "--list-only", "--delete-bucket"])` returns a clap conflict error.
- [ ] `delete_bucket_argv("my-bucket")` returns `["r2", "bucket", "delete", "my-bucket"]`.
- [ ] `delete_bucket_argv` passes names with `-`, `.`, and digits through verbatim.
- [ ] On an empty bucket with `-d --force`, the tool invokes `wrangler r2 bucket delete <name>` and prints `✅ Bucket '<name>' deleted.`
- [ ] On a non-zero exit from `wrangler r2 bucket delete`, the tool propagates stderr and exits non-zero.
- [ ] Each red test is committed separately from its green implementation.
- [ ] `cargo test -p r2-bucket-cleaner` passes at the end of the phase; `cargo clippy` is clean.

---

## Phase 2: Full empty-and-delete workflow with prompt wording

**User stories**:

- As a user, I can run `r2-bucket-cleaner <bucket> -d` on a *non-empty* bucket and have it emptied and deleted, with confirmation wording that makes the bucket-delete intent explicit.
- As a user without `--force`, running `-d` on an already-empty bucket shows a dedicated `Delete bucket '<name>'?` prompt before proceeding.
- As a user reading the README, the new flag is documented alongside a usage example and the "How it Works" section explains the optional bucket-delete step.

### What to build

Extend Phase 1's slice to cover the interactive, non-forced, and non-empty paths — the flows a real user is most likely to hit. After this phase, `-d` behaves per the full spec across all three starting states (empty + force, empty + interactive, non-empty).

TDD sequence (red → green per change):

1. Update the existing `"This will permanently delete N objects!"` warning so that when `--delete-bucket` is set it reads `...and delete the bucket itself.` Driven by a small pure helper (e.g. `warning_text(count, delete_bucket) -> String`) that is unit-testable.
2. Wire the post-emptying delete path: after the existing success branch, when `args.delete_bucket` is set and the bucket was just emptied, call `delete_bucket` with no extra prompt (the warning already covered it) and print the success line.
3. Wire the already-empty + non-force prompt: when pass 1 returns empty and `args.delete_bucket && !args.force`, show `Delete bucket '<name>'?` via `dialoguer::Confirm`, then delete on confirm or abort on decline.
4. README updates: add the flag to the options list, add at least one usage example showing `-d`, and append a line to "How it Works" describing the optional bucket-delete step.

### Acceptance criteria

- [ ] With `--delete-bucket`, the pre-delete warning includes the phrase "and delete the bucket itself" (or equivalent) — verified by unit test on the pure wording helper.
- [ ] Without `--delete-bucket`, the pre-delete warning is unchanged — verified by unit test.
- [ ] After successfully emptying a non-empty bucket with `-d`, the tool calls `wrangler r2 bucket delete <name>` without asking a second confirmation and prints `✅ Bucket '<name>' deleted.`
- [ ] With `-d` (no `--force`) on an already-empty bucket, the tool shows a `Delete bucket '<name>'?` prompt; declining aborts without calling `wrangler r2 bucket delete`.
- [ ] If emptying fails, `wrangler r2 bucket delete` is not invoked and the tool exits non-zero.
- [ ] If `wrangler r2 bucket delete` fails after emptying, the tool propagates stderr and exits non-zero.
- [ ] `src/r2-bucket-cleaner/README.md` documents the `-d` / `--delete-bucket` flag in the options list, includes a usage example, and the "How it Works" section mentions the optional bucket-delete step.
- [ ] Manual verification completed per spec §Testing/Integration: throwaway bucket created, populated, cleaned with `-d --force`, confirmed absent from `wrangler r2 bucket list`; separate empty-bucket run without `--force` confirms prompt wording and deletion.
- [ ] Each red test is committed separately from its green implementation.
- [ ] `cargo test -p r2-bucket-cleaner`, `cargo clippy`, and `cargo fmt --check` are clean at end of phase.
