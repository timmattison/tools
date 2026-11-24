# sirn (Rust) — Design

**Date:** 2026-06-18
**Status:** Approved (brainstorming)
**Author:** Tim Mattison (with Claude)

## Summary

`sirn` ("Serve It Right Now") is a tiny, zero-config HTTP file server. This is a
Rust reimplementation of the original Java tool
(<https://github.com/timmattison/sirn>). The key change: the listening port is no
longer a required positional argument. By default the port is **derived from the
git repo root + branch name** using `portplz`'s algorithm — but `portplz` itself
is **not** required to be installed. The shared logic lives in a new internal
library crate, `portplz-core`, that both `portplz` and `sirn` depend on.

## Background — the original Java tool

The Java `sirn` (`com.timmattison.sirn.Main`):

- Usage: `java -jar sirn.jar <port> <file1> <file2> ...`
- First arg is the port; remaining args are files to serve.
- Each file is served at `/<path>` (the literal argument string).
- A request reads the **entire file into memory** and returns it.
- If the file is **missing or empty** (`file.length() == 0`) it returns `404`.
- No `Content-Type` header is set.
- A 1-second polling loop logs availability transitions:
  - `Ready to serve <path>` when a previously-missing file appears.
  - `Warning!  File <path> not found...` when a file disappears.
- Binds the wildcard address (all interfaces).

## Goals

1. Faithful spirit of the original, reimplemented in idiomatic Rust.
2. Default the port from `portplz`'s derivation, without requiring `portplz` installed.
3. Modernize the rough edges (streaming, content types, correct empty-vs-missing).
4. Full test coverage (TDD), parallel-safe tests, repo conventions (buildinfo version, etc.).

## Non-goals

- No authentication, TLS, uploads, or write endpoints — read-only static serving.
- No `--shell-setup` shell integration: `sirn` does not mutate the parent shell,
  so per the repo's shell-integration rule it ships none.
- No progress bars, so no `termbar` usage.

## Architecture

Three crates under `src/` (workspace `members = ["src/*"]` picks them up automatically):

```
src/portplz-core/   library: PortSource, hashing, derive()   <- the deep module
src/portplz/        binary:  thin CLI over portplz-core (behavior unchanged)
src/sirn/           binary:  HTTP file server; uses portplz-core for the default port
```

### `portplz-core` (new library crate)

Holds all logic currently inlined in `portplz/src/main.rs`. Narrow interface that
hides SHA-256 hashing and `gix` repo discovery behind a single `derive()` entry point.

```rust
/// A derived, guaranteed-unprivileged port (1024..=65534).
/// Constructed only by this crate's derivation functions.
pub struct DerivedPort(u16);
impl DerivedPort {
    pub fn get(&self) -> u16;
}

/// How the port's hash input was determined.
pub enum PortSource {
    GitRepo { repo_name: String, branch: String }, // hash input: "repo\nbranch"
    DetachedHead { repo_name: String },             // hash input: "repo"
    Directory { dirname: String },                  // hash input: "dirname"
}
impl PortSource {
    pub fn hash_input(&self) -> String;
    pub fn describe(&self, port: DerivedPort) -> String; // "Port N for ..."
}

pub struct Derivation {
    pub port: DerivedPort,
    pub source: PortSource,
}

#[derive(Debug, thiserror::Error)]
pub enum DeriveError {
    #[error("invalid path: no basename")]
    NoBasename,
}

/// Derive the port for `path`. When `no_git` is true (or `path` is not in a repo),
/// the directory basename is used; otherwise the repo-root name + branch.
pub fn derive(path: &Path, no_git: bool) -> Result<Derivation, DeriveError>;

/// The hashing primitive: SHA-256 of `input`, first 2 bytes -> u16, lifted to >=1024.
pub fn unprivileged_port_from_string(input: &str) -> DerivedPort;
```

Internals migrated verbatim from `portplz/src/main.rs`:
`get_repo_root_name`, `get_git_branch`, the hash-to-port arithmetic, and the
`PortSource` variants/separator semantics (newline separator so repo/branch can't
collide). The port arithmetic is unchanged so existing derived ports stay stable.

Dependencies: `gix`, `sha2`, `thiserror` (all workspace deps).

### `portplz` (refactored binary)

`main.rs` becomes a thin shell:

1. Resolve `path` (arg or `current_dir()`).
2. `let d = portplz_core::derive(&path, cli.no_git)?;`
3. `--verbose` → print `d.source.describe(d.port)`, else print `d.port.get()`.

CLI surface (`path`, `--verbose`, `--no-git`), version string, and **output are
unchanged**. Its existing unit tests move into `portplz-core` alongside the code
they exercise (pure extraction performed under continuous green).

### `sirn` (new binary)

#### CLI

```
sirn [OPTIONS] [FILES]...

  [FILES]...         Files to serve. None -> serve the current directory as a browsable tree.
  -p, --port <N>     Override the portplz-derived port.
  -b, --bind <ADDR>  Bind address (default 127.0.0.1). Use 0.0.0.0 to expose on the LAN.
      --no-git       Ignore git branch when deriving the port (dir-name based).
  -v, --verbose      Print the port-derivation source at startup.
  -V, --version      buildinfo version string (git hash + clean/dirty).
```

Port resolution: if `--port` is given, bind that raw `u16` (may be privileged; OS
will reject if not permitted). Otherwise `portplz_core::derive(current_dir, no_git)`
supplies the port. `--verbose` prints the derivation source line at startup.

Default bind `127.0.0.1` (localhost only) — a deliberate, safer divergence from
the Java wildcard bind. `--bind 0.0.0.0` restores LAN exposure.

#### Two serving modes

Determined at startup by whether any positional files were given.

**Files mode** (>= 1 file):

- Build a route map `/<basename> -> PathBuf`.
- **Collision check at startup:** two arguments with the same basename is a hard
  error (predictable, self-documenting routes since every URL is printed).
- A 1-second monitor thread logs availability transitions, mirroring Java:
  `Ready to serve <name>` / `Warning!  File <name> not found...`.
- Per request:
  - Route not registered -> `404`.
  - File missing on disk -> `404`.
  - File present (including empty) -> stream it with `Content-Length` and a
    `Content-Type` from the extension. Empty file -> empty `200` (modernized; Java
    returned `404`).

**Directory mode** (no files):

- Serve `current_dir()` recursively.
- Resolve the request path against the root, **canonicalize, and confine to the
  root**; any traversal or symlink escape -> `403`.
- A directory -> an HTML listing (entries HTML-escaped, dirs marked, `..` link
  except at root).
- A file -> stream as above.
- Missing -> `404`.

#### HTTP backend

`tiny_http` (sync). Pattern: `Arc<tiny_http::Server>` shared across a small fixed
pool of worker threads, each looping on `server.recv()`. The availability monitor
(files mode) runs on its own thread. Streaming uses `tiny_http::Response::from_file`
/ a reader response so large files are not buffered fully in memory.

#### Content-Type

A hand-rolled, case-insensitive extension -> MIME table (no new dependency):
`html, htm, css, js, mjs, json, txt, md, csv, xml, png, jpg, jpeg, gif, svg, webp,
ico, pdf, wasm, woff, woff2, zip, gz, mp4, mp3, wav` with `application/octet-stream`
fallback. `text/*` types include `; charset=utf-8`.

#### Startup banner

Prints the version, the resolved port and (with `--verbose`) its derivation source,
the bind address, and the full URL for each served file (files mode) or the root URL
(directory mode).

### New dependencies

- `tiny_http = "0.12"` added to `[workspace.dependencies]`.
- `tempfile` (already a workspace dep) used as a `sirn` dev-dependency.
- `portplz-core = { path = "src/portplz-core" }` added to `[workspace.dependencies]`.

## Testing strategy (TDD)

All tests must be **parallel-safe** (a `bacon`/pre-commit `cargo test` may run a
second copy concurrently): temp dirs keyed on `std::process::id()` + nanosecond
timestamp (via `tempfile`), and servers bound to `127.0.0.1:0` (OS-assigned port,
read back from `server.server_addr()`).

### `portplz-core`

Migrated from `portplz` (same assertions, now exercising the library):
- port range / determinism / different-input divergence,
- `PortSource` hash-input formats and the newline-separator collision guard,
- verbose `describe()` strings for all three variants,
- `get_repo_root_name` returns a valid basename,
- worktree and main repo share the same root name.
- New: `derive()` on a freshly-`git init`'d temp repo returns a `GitRepo` source
  and a stable port.

### `sirn` unit tests

- Content-Type mapping: known extensions, case-insensitivity, unknown -> octet-stream,
  `charset=utf-8` on text types.
- Path confinement: `../`, absolute paths, and symlink-escape attempts are rejected;
  in-root paths accepted.
- Directory listing HTML: lists entries, HTML-escapes names (e.g. a file named
  `a<b>.txt`), marks directories.
- Basename-collision detection errors when two args share a basename.
- UTF-8 safety: filenames with multi-byte characters (日本語, 🎉, café) in listings
  and routes don't panic.

### `sirn` integration tests

Spin a real server on an ephemeral port; issue raw **HTTP/1.0** `GET`s over
`std::net::TcpStream` (request `Connection: close`, read to EOF — no async/reqwest
dependency). A small `http_get(addr, path) -> (status, headers, body)` helper.

- Existing file -> `200`, exact bytes, correct `Content-Type`, correct `Content-Length`.
- Large file streams correctly (bytes match a multi-chunk file).
- Empty file -> `200` with empty body (not `404`).
- Unregistered route / missing file -> `404`.
- Directory mode: root request -> `200` listing containing known entries; nested
  file fetch works; `GET /../../etc/passwd`-style traversal -> `403`.
- Port determinism: `derive()` for a temp repo path is stable across calls.

## Repo-convention checklist

- `--version` / `-V` via `buildinfo::version_string!()` on both binaries.
- No `--shell-setup` (not load-bearing for `sirn`).
- No `termbar` (no progress bars).
- UTF-8-safe string handling throughout (no byte-index slicing).
- `[lints] workspace = true` on all three crates.
- Update the repo-root tool listing/README if one enumerates the tools.

## Intentional divergences from the Java tool

1. Port defaults to the `portplz` derivation instead of being a required positional arg.
2. Basename routing (with startup collision detection) instead of raw-argument routing.
3. Streaming instead of full in-memory reads; `Content-Type` + `Content-Length` set.
4. Empty file -> empty `200` (only truly-missing files -> `404`).
5. Default bind `127.0.0.1` instead of the wildcard address.
6. No-argument invocation serves the current directory as a browsable tree (new feature).
