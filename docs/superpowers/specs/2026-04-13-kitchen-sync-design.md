# kitchen-sync Design Spec

## Purpose

A CLI tool that takes a git repo URL, discovers all Rust binary packages in the repo, and installs them via `cargo install --git <url> --package <name>`.

The name is a play on "kitchen sink" — it installs everything.

## CLI Interface

```
kitchen-sync <repo-url>
kitchen-sync --version
```

Single positional argument: the git repo URL (e.g., `https://github.com/user/repo`).

## Flow

1. **Parse CLI args** — clap derive, single positional `repo_url: String`.
2. **Shallow clone** — `git clone --depth 1 <url>` into a temp directory (`tempfile::tempdir()`).
3. **Parse root Cargo.toml** — read and parse with the `toml` crate. Extract `workspace.members` array.
4. **Resolve member globs** — workspace members can contain globs (e.g., `src/*`). Resolve these against the cloned directory using the `glob` crate.
5. **Filter to binary packages** — for each resolved member directory, check:
   - Does `Cargo.toml` contain a `[[bin]]` section? OR
   - Does `src/main.rs` exist?
   - If neither, skip (library-only crate).
6. **Extract package names** — read `package.name` from each member's `Cargo.toml`.
7. **Print plan** — list all packages that will be installed.
8. **Install each package** — run `cargo install --git <url> --package <name>` sequentially. Capture stdout/stderr.
9. **Track results** — collect successes and failures (package name + error message).
10. **Clean up** — temp directory is cleaned up automatically by `tempfile`.
11. **Print summary** — show count of successes and failures. List failed packages with error messages.

## Exit Code

- `0` — all packages installed successfully, or at least one succeeded.
- `1` — all installs failed, or clone/discovery failed.

## Crate Structure

Standard monorepo member at `src/kitchen-sync/`:

```
src/kitchen-sync/
├── Cargo.toml
└── src/
    └── main.rs
```

### Dependencies

- `clap` (workspace) — CLI argument parsing
- `buildinfo` (workspace) — version string with git hash
- `anyhow` (workspace) — error handling
- `toml` — parsing Cargo.toml files
- `glob` — resolving workspace member patterns
- `tempfile` — temporary directory for shallow clone

### Cargo.toml Pattern

```toml
[package]
name = "kitchen-sync"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow.workspace = true
buildinfo = { path = "../buildinfo" }
clap = { workspace = true, features = ["derive"] }
glob.workspace = true
tempfile.workspace = true
toml.workspace = true
```

## Error Handling

- **Clone failure** — exit with error message (bad URL, network issue, not a git repo).
- **No Cargo.toml at root** — exit with error: "Not a Rust project (no Cargo.toml found)".
- **No workspace members** — check if it's a single-package repo (has `[[bin]]` or `src/main.rs` at root). If so, just run `cargo install --git <url>`. If not, exit with error.
- **Individual install failure** — log the error, continue with remaining packages.
- **No binary packages found** — exit with error: "No binary packages found in repository".

## Output Format

```
Cloning https://github.com/user/repo...
Found 12 binary packages: freeport, hexfind, prcp, ...

Installing freeport (1/12)...
  Installed freeport
Installing hexfind (2/12)...
  Installed hexfind
Installing broken-tool (3/12)...
  FAILED: <error message>
...

Summary: 11 installed, 1 failed
  Failed: broken-tool
```

## Non-Goals

- No parallel installation (cargo install takes locks anyway).
- No include/exclude filtering.
- No caching or incremental installs.
- No support for non-Rust packages in the repo.
