# bm (bulk move)

- Bulk Move - recursively find and move files matching a pattern to a destination directory. Named "bm" because
  moving lots of files is shitty. Much simpler than `find ... -exec mv`, especially for common tasks like moving
  all files of a certain type. Collision-safe by default and handles moves across volumes (where `rename` fails).
- To install: `cargo install --git https://github.com/timmattison/tools bm`

Recursively find and move files matching a pattern (suffix, prefix, or substring) to a destination directory. Named "bm" because moving lots of files is shitty.

Unlike a bare `mv`/`rename`, bm is collision-safe by default (it refuses to silently overwrite) and transparently handles **moves across volumes** — where `rename(2)` fails with a cross-device error — by copying then deleting, with a progress bar.

## Basic Usage

```bash
bm --suffix .jpg --destination ~/Pictures/photos
bm --prefix IMG_ --destination ~/Pictures/camera ~/Downloads
bm --substring 2024 --destination ~/archive/2024
```

## Options

- `-s`, `--suffix <SUFFIX>`: Match files ending with this string (e.g., `.jpg`, `.mkv`)
- `-p`, `--prefix <PREFIX>`: Match files starting with this string (e.g., `IMG_`, `video_`)
- `--substring <SUBSTRING>`: Match files containing this string anywhere in the name
- `-d`, `--destination <DIR>`: Directory to move matching files to (required; must already exist)
- `--on-collision <POLICY>`: What to do when a destination name already exists or repeats within the batch — `abort` (default), `skip`, `rename`, or `overwrite`
- `--dry-run`: Show what would be moved without moving anything
- `[DIR]...`: Directories to search (defaults to the current directory)

**Note:** Exactly one of `--suffix`, `--prefix`, or `--substring` must be specified.

## Collision handling

If two matched files would land on the same destination name — because the destination already contains that name, or because two source files share a basename — bm does not silently clobber. Choose the behavior with `--on-collision`:

- **`abort`** (default): report every collision and move nothing.
- **`skip`**: move the non-colliding files, leave the colliding ones in place.
- **`rename`**: move everything, disambiguating names (`foo.mkv` → `foo-1.mkv`), preserving extensions.
- **`overwrite`**: move everything, letting later files clobber earlier ones (lossy).

## Cross-volume moves

A plain `rename(2)` cannot move a file between filesystems (e.g. internal disk → an external `/Volumes/...` drive); it fails with a cross-device error. bm detects this and falls back to a chunked copy (with a progress bar) followed by deleting the source, so the same command works whether or not the destination is on the same volume:

```bash
bm --suffix .mkv --destination /Volumes/Backup/videos
```

## Why use bm instead of mv?

```bash
# Moving all .mkv files to a backup drive, with mv + find (verbose, error-prone,
# and find's mv fails file-by-file across volumes):
find . -name "*.mkv" -exec mv {} /Volumes/Backup/videos/ \;

# With bm (simple, collision-safe, cross-volume aware):
bm --suffix .mkv --destination /Volumes/Backup/videos
```

## Examples

Preview moving every file from 2024 without touching anything:
```bash
bm --substring 2024 --destination ~/archive/2024 --dry-run
```

Organize photos, keeping every file even on name clashes:
```bash
bm --prefix IMG_ --on-collision rename --destination ~/Pictures/iphone ~/Downloads
```

Move PDFs from several directories at once:
```bash
bm --suffix .pdf --destination ~/Documents/pdfs ~/Downloads ~/Desktop /tmp
```

## Output

On completion, bm prints a summary:
```
Move complete: 42 moved (40 renamed, 2 copied across volumes), 0 skipped in 1.23s (34 files/sec)
```
