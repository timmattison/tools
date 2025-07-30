# R2 Bucket Cleaner

A Rust CLI tool that lists and optionally clears all objects from a Cloudflare R2 bucket. This tool uses `wrangler` under the hood to perform the operations, so you need to have `wrangler` installed and configured.

## Prerequisites

- Rust (latest stable version)
- `wrangler` CLI tool installed and authenticated (`npm install -g wrangler`)
- Valid Cloudflare credentials configured in wrangler

## Installation

```bash
cargo build --release
```

The binary will be available at `target/release/r2-bucket-cleaner`.

## Usage

```bash
# List all objects in a bucket
r2-bucket-cleaner BUCKET_NAME --list-only

# Delete objects in a bucket (with confirmation prompt)
r2-bucket-cleaner BUCKET_NAME

# Delete all objects without confirmation
r2-bucket-cleaner BUCKET_NAME --force

# Automatically continue deleting until all objects are removed (bypass 20 object limit)
r2-bucket-cleaner BUCKET_NAME --all

# Delete all objects without confirmation and bypass limit
r2-bucket-cleaner BUCKET_NAME --force --all
```

## Options

- `BUCKET_NAME`: The name of the R2 bucket to operate on (required)
- `-l, --list-only`: Only list objects, don't delete them
- `-f, --force`: Skip confirmation prompt and delete all objects
- `-a, --all`: Automatically continue until all objects are deleted (bypass 20 object limit)
- `-h, --help`: Print help information
- `-V, --version`: Print version information

## Safety

By default, the tool will:
1. List all objects in the bucket
2. Show you the count of objects to be deleted
3. Ask for confirmation before proceeding with deletion

Use the `--force` flag with caution as it will delete all objects without confirmation.

## Performance

The tool deletes objects with the following optimizations:
- Processes 10 objects concurrently for improved speed
- Implements retry logic with exponential backoff (200ms, 400ms, 600ms)
- Shows progress every 20 objects deleted
- Minimal delays between operations (50ms between batches, 200ms between passes)

## Known Limitations

**Wrangler CLI Pagination**: Due to a limitation in the wrangler CLI, only 20 objects can be listed at a time. The tool handles this by:
- Automatically detecting when there are more objects
- Using the `--all` flag to automatically continue deleting in batches of 20 until all objects are removed
- Showing progress after each batch

When using the `--all` flag:
- The tool displays "Found at least 20 objects" instead of an exact count when more objects exist
- A warning is shown that more files will be deleted than initially listed
- The detailed file list is skipped to avoid misleading users about the scope of deletion

Without the `--all` flag, you'll need to run the tool multiple times for buckets with more than 20 objects.

For very large buckets (thousands of objects), consider using [rclone with R2](https://developers.cloudflare.com/r2/examples/rclone/) for better performance.

## How it Works

This tool wraps the `wrangler r2` commands to provide a convenient way to clear out R2 buckets. It:
1. Uses `wrangler r2 object get --remote BUCKET/` to list objects (limited to 20 per request)
2. Uses `wrangler r2 object delete` to remove objects with retries
3. Reports any failures at the end

The tool inherits authentication from your existing wrangler configuration (typically stored in `~/Library/Preferences/.wrangler/config/default.toml` on macOS).