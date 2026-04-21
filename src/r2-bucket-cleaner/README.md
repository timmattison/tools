# R2 Bucket Cleaner

A Rust CLI tool that lists and optionally clears all objects from a Cloudflare R2 bucket, and can delete the bucket itself once it's empty. It talks to R2's S3-compatible endpoint via `aws-sdk-s3` — no `wrangler` required.

## Prerequisites

- Rust (latest stable version)
- R2 API credentials (account id + S3-compatible access key id + secret access key)

### Credentials

The tool loads credentials from the first source that satisfies all three values:

1. **Environment variables** (all three must be set and non-empty):
   - `R2_ACCOUNT_ID`
   - `R2_ACCESS_KEY_ID`
   - `R2_SECRET_ACCESS_KEY`
2. **1Password via [op-cache](../op-cache)**, from the item `R2 Credentials` in the `Private` vault:
   - `op://Private/R2 Credentials/R2_ACCOUNT_ID`
   - `op://Private/R2 Credentials/R2_ACCESS_KEY_ID`
   - `op://Private/R2 Credentials/R2_SECRET_ACCESS_KEY`

Generate R2 access keys under *R2 → Manage API Tokens* in the Cloudflare dashboard.

## Installation

```bash
cargo build --release
```

The binary will be available at `target/release/r2-bucket-cleaner`.

## Usage

```bash
# List the first page of objects in a bucket
r2-bucket-cleaner BUCKET_NAME --list-only

# Delete objects in a bucket (with confirmation prompt)
r2-bucket-cleaner BUCKET_NAME

# Delete all objects without confirmation
r2-bucket-cleaner BUCKET_NAME --force

# Automatically continue deleting until every object is removed (bypass preview cap)
r2-bucket-cleaner BUCKET_NAME --all

# Delete all objects without confirmation and without the preview cap
r2-bucket-cleaner BUCKET_NAME --force --all

# Empty the bucket and then delete the bucket itself
r2-bucket-cleaner BUCKET_NAME --delete-bucket --all

# Delete an empty (or emptied) bucket without any prompts
r2-bucket-cleaner BUCKET_NAME -d --force --all
```

## Options

- `BUCKET_NAME`: The name of the R2 bucket to operate on (required)
- `-l, --list-only`: Only list objects, don't delete them
- `-f, --force`: Skip confirmation prompts and delete without asking
- `-a, --all`: Automatically continue until all objects are deleted (bypass the 20-object preview cap)
- `-d, --delete-bucket`: After emptying, delete the bucket itself (conflicts with `--list-only`)
- `-h, --help`: Print help information
- `-V, --version`: Print version information

## Safety

By default, the tool will:
1. List a preview of objects in the bucket (up to 20)
2. Show you the count of objects to be deleted
3. Ask for confirmation before proceeding with deletion

With `--delete-bucket`, the confirmation prompt's wording is updated to make it clear the bucket itself will also be removed.

Use the `--force` flag with caution — it suppresses every confirmation, including the bucket-delete prompt.

## Performance

- Listing uses `ListObjectsV2` with a page size of 1000 when `--all` is set.
- Deletion uses batched `DeleteObjects` — up to 1000 keys per request.
- A progress bar tracks batch deletion.

For very large buckets, [rclone with R2](https://developers.cloudflare.com/r2/examples/rclone/) is still a reasonable alternative.

## Preview cap

Without `--all`, the tool shows only the first 20 objects and, for buckets with more, asks you to re-run with `--all` (or re-invoke manually). Pass `--all` to skip the preview cap and delete everything in one run.

## How it Works

1. Builds an S3 client pointed at `https://<account-id>.r2.cloudflarestorage.com` with `region = "auto"` and the credentials described above.
2. Calls `ListObjectsV2` to enumerate keys (paginated with a continuation token when `--all` is set).
3. Calls `DeleteObjects` in batches of up to 1000 keys each to remove them, reporting any per-object failures.
4. If `--delete-bucket` is set and emptying succeeded, calls `DeleteBucket` to remove the now-empty bucket.
