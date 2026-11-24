# r2-bucket-cleaner: `--delete-bucket` flag

**Date:** 2026-04-20
**Tool:** `src/r2-bucket-cleaner`
**Type:** Feature addition

> **Update (2026-04-21):** The "No migration to the S3 API" scope decision below was reversed mid-implementation. `wrangler r2 object get --remote <bucket>/` turned out to not be a listing command (it fetches a single object by key), so the existing tool was broken end-to-end. The fix was to swap the wrangler shell-out for `aws-sdk-s3` against R2's S3-compatible endpoint. The `--delete-bucket` flag surface described here is unchanged; only the backend it calls through to is different. See commit `4dde246` for the migration. Sections that reference `wrangler` (e.g. *Code changes → r2_wrangler.rs*, *Risks → Wrangler version drift*) are stale.

## Motivation

Today `r2-bucket-cleaner` empties the objects out of a Cloudflare R2 bucket but leaves the (now-empty) bucket behind. R2 requires a bucket to be empty before it can be deleted, so clearing objects is only half of the job for anyone who wants the bucket gone. Users have to follow the tool with a separate `wrangler r2 bucket delete <bucket>` to finish up.

This spec adds a `--delete-bucket` flag that, once the bucket is empty, deletes the bucket itself in the same invocation.

## Scope

- Extend `r2-bucket-cleaner` with a `--delete-bucket` / `-d` flag.
- After successful emptying, shell out to `wrangler r2 bucket delete <bucket>` and report the result.
- Keep the existing wrangler-based backend. No migration to the S3 API.
- Keep the existing 20-object listing behavior — no change to listing or per-object delete logic.

## Out of scope

- Switching the listing/delete backend to the `aws-sdk-s3` crate (tracked as a possible follow-up for large buckets).
- Deleting multiple buckets in one invocation.
- Changing authentication (the tool continues to inherit wrangler's ambient auth).
- A dedicated dry-run mode for bucket deletion.

## Behavior

### Happy path

1. User runs `r2-bucket-cleaner <bucket> --delete-bucket`.
2. Tool lists objects, shows the confirm prompt with updated wording ("...and delete the bucket itself").
3. On confirmation, tool empties the bucket using the existing pagination/retry logic.
4. Once `list_objects` returns empty, tool calls `wrangler r2 bucket delete <bucket>`.
5. Tool prints a final success line: `✅ Bucket '<name>' deleted.`

### Already-empty bucket

If the bucket is empty on pass 1 and `--delete-bucket` is set:
- No "delete N objects" prompt fires (there are none).
- Without `--force`: show a dedicated confirm — `Delete bucket '<name>'?`
- With `--force`: skip the prompt, delete immediately.

### `--force`

`--force` suppresses **all** interactive confirms, including the bucket-delete confirm.

### Flag conflicts

`--list-only` + `--delete-bucket` is a contradiction. Clap rejects it via `conflicts_with("list_only")` at parse time — no runtime check needed.

### Failure modes

- If emptying fails (any object delete ultimately errors), the tool exits non-zero **before** attempting `wrangler r2 bucket delete`. The bucket is left in whatever partially-emptied state the existing code leaves it in.
- If `wrangler r2 bucket delete` fails (e.g., wrangler reports the bucket is somehow non-empty, or the user lacks permission), the tool propagates stderr and exits non-zero. No retry — a bucket-level delete is rare enough that manual follow-up is acceptable.

## Code changes

### `src/r2-bucket-cleaner/src/r2_wrangler.rs`

Add a new method on `R2WranglerClient`:

```rust
pub async fn delete_bucket(&self, bucket_name: &str) -> Result<()>
```

It shells out to `wrangler r2 bucket delete <bucket>` via the same `task::spawn_blocking` + `Command::new` pattern already used in this file, returning `Err` with stderr on non-zero exit.

To make this unit-testable without running wrangler, extract a small pure helper:

```rust
pub(crate) fn delete_bucket_argv(bucket: &str) -> Vec<String>
```

— returns the argv (excluding the program name) that would be passed to `wrangler`. The method calls this to build its `Command` args, and tests assert on the returned vec.

### `src/r2-bucket-cleaner/src/main.rs`

- Add `#[arg(short = 'd', long, conflicts_with = "list_only")] delete_bucket: bool` to `Args`.
- Update the "WARNING: This will permanently delete N objects!" prompt so that when `--delete-bucket` is set, it says "...and delete the bucket itself."
- After the existing `return Ok(())` / `break` success paths:
  - If the bucket was empty on pass 1 and `--delete-bucket` is set: prompt (unless `--force`), then delete bucket.
  - If the bucket was emptied by the tool and `--delete-bucket` is set: delete bucket (no extra prompt — already confirmed).
  - Print `✅ Bucket '<name>' deleted.` on success.

### `src/r2-bucket-cleaner/README.md`

Document the new flag, add to the usage examples section, and add a line to the "How it Works" section explaining the optional bucket-delete step.

## Testing (TDD)

Per CLAUDE.md, every change is red-green-refactor with separate commits. Existing tool has no tests — this change introduces the first ones.

### Unit tests (in `src/r2-bucket-cleaner/src/r2_wrangler.rs`)

- `delete_bucket_argv_builds_correct_args`: asserts `delete_bucket_argv("my-bucket")` returns `["r2", "bucket", "delete", "my-bucket"]`.
- `delete_bucket_argv_handles_special_characters`: asserts bucket names with `-`, `.`, and digits are passed through verbatim (not shell-escaped or rewritten). R2 bucket names are restricted enough that this is a sanity check, not a security test.

### Unit tests (in `src/r2-bucket-cleaner/src/main.rs` or a new `args.rs`)

- `args_list_only_and_delete_bucket_conflict`: `Args::try_parse_from(["r2-bucket-cleaner", "bucket", "--list-only", "--delete-bucket"])` must return `Err` with clap's conflict error kind.
- `args_delete_bucket_parses`: `Args::try_parse_from(["r2-bucket-cleaner", "bucket", "--delete-bucket"])` succeeds and `delete_bucket` is `true`.
- `args_short_flag_parses`: `Args::try_parse_from(["r2-bucket-cleaner", "bucket", "-d"])` succeeds and `delete_bucket` is `true`.

### Integration / manual

The actual `wrangler r2 bucket delete` invocation is not covered by automated tests — it requires a live Cloudflare account. Manual verification plan:

1. Create a throwaway bucket via `wrangler r2 bucket create r2-cleaner-test-<timestamp>`.
2. Upload a handful of small objects.
3. Run `r2-bucket-cleaner r2-cleaner-test-<timestamp> --delete-bucket --force`.
4. Verify `wrangler r2 bucket list` no longer shows the bucket.
5. Also verify the already-empty path: create a new bucket, run with `-d` (no `--force`), confirm the prompt wording, accept, verify bucket is gone.

## Risks

- **Accidental bucket loss.** The whole point of the flag is destructive, but the existing confirm prompt + `--force`-only override is the right guard. We are not adding a "are you sure" on top — the existing one covers both object and bucket destruction.
- **Wrangler version drift.** `wrangler r2 bucket delete <bucket>` is a stable wrangler 3/4 command, but if a future wrangler version changes its argv or output, the tool breaks silently on parsing or loudly on exec. Acceptable — the existing tool has the same exposure on `wrangler r2 object get/delete`.
- **Bucket re-fills during emptying.** If another process writes to the bucket while we're emptying, `wrangler r2 bucket delete` will fail with a non-empty error. We propagate the error; the user can re-run. Not worth special-casing.
