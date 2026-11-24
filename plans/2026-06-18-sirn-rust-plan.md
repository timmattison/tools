# Plan: sirn (Rust reimplementation)

> Source PRD/spec: `specs/2026-06-18-sirn-rust-design.md`

A Rust reimplementation of the Java `sirn` HTTP file server. The listening port
defaults to `portplz`'s derivation (no `portplz` install required); the shared
logic moves into a new `portplz-core` library crate. Built in vertical slices,
each independently verifiable. Every phase is TDD (red → commit → green → commit)
and parallel-safe (temp dirs keyed on pid+nanos; servers bound to `127.0.0.1:0`).

## Architectural decisions

Durable decisions that apply across all phases:

- **Crates** (under `src/`, auto-included by `members = ["src/*"]`):
  - `src/portplz-core/` — library: `derive()`, `PortSource`, `DerivedPort`, hashing.
  - `src/portplz/` — binary, thin shell over `portplz-core` (behavior unchanged).
  - `src/sirn/` — binary, HTTP server, depends on `portplz-core`.
  - Declared in `[workspace.dependencies]` as `portplz-core = { path = "src/portplz-core" }`.
- **Port resolution**: `portplz_core::derive(cwd, no_git)` by default; `--port <N>`
  overrides (raw `u16`, may be privileged). Derived port arithmetic is unchanged
  from current `portplz`, so existing ports stay stable.
- **Bind**: `127.0.0.1` by default; `--bind <ADDR>` (e.g. `0.0.0.0`) to change.
- **Routes**:
  - Files mode (>=1 positional file): each file at `/<basename>`; duplicate
    basenames are a hard startup error.
  - Directory mode (no positional files): request path resolved under `cwd`,
    canonicalized, and confined to the root (escape → `403`).
- **HTTP**: `tiny_http` (`= "0.12"`, new workspace dep). `Arc<tiny_http::Server>`
  shared across a small fixed worker-thread pool. Responses stream from disk with
  `Content-Length` and a `Content-Type` from a hand-rolled extension→MIME table
  (`text/*` carries `; charset=utf-8`; unknown → `application/octet-stream`).
- **Response semantics** (modernized): present file (even empty) → `200`; only a
  truly-missing file → `404`.
- **Conventions**: `--version`/`-V` via `buildinfo::version_string!()`; `[lints]
  workspace = true` on all crates; UTF-8-safe string handling; no `--shell-setup`
  (not load-bearing); no `termbar`.

---

## Phase 1: Extract `portplz-core`; refactor `portplz`

**User stories / goals**: Goal 2 (default the port from portplz's derivation
without requiring portplz installed) — establishes the shared library foundation
the rest of the plan builds on.

### What to build

Create the `src/portplz-core` library crate containing all port-derivation logic
currently inlined in `portplz/src/main.rs`: the `PortSource` enum (with `\n`
separator semantics), the `DerivedPort` newtype (invariant 1024..=65534), the
hash-to-port arithmetic (`unprivileged_port_from_string`), repo-root-name and
branch discovery via `gix`, and a single `derive(path, no_git) -> Result<Derivation,
DeriveError>` entry point that hides discovery + hashing behind a narrow interface.

Refactor the `portplz` binary into a thin shell: resolve the path (arg or
`current_dir()`), call `portplz_core::derive`, and print either `port` or
`source.describe(port)` for `--verbose`. The CLI surface (`path`, `--verbose`,
`--no-git`), the printed output, and the derived port numbers must be **identical**
to today. Migrate `portplz`'s existing unit tests into `portplz-core`.

### Acceptance criteria

- [ ] `src/portplz-core` exists with the public interface from the spec
      (`DerivedPort`, `PortSource`, `Derivation`, `DeriveError`, `derive`,
      `unprivileged_port_from_string`), `[lints] workspace = true`, and
      dependencies `gix`, `sha2`, `thiserror`.
- [ ] `portplz-core` carries the migrated tests (port range/determinism/divergence,
      `PortSource` hash inputs + newline-separator collision guard, verbose
      `describe()` for all three variants, `get_repo_root_name` validity, worktree
      ↔ main-repo same root name) plus a new `derive()` test on a freshly
      `git init`'d temp repo (parallel-safe temp dir).
- [ ] `portplz` binary is a thin shell over `portplz_core::derive`; no derivation
      logic remains duplicated in it.
- [ ] `portplz` output is byte-for-byte unchanged: same port for the same
      repo/branch/dir, same `--verbose` wording, same `--no-git` behavior.
- [ ] `[workspace.dependencies]` includes `portplz-core = { path = "src/portplz-core" }`.
- [ ] `cargo test -p portplz-core -p portplz` and `cargo clippy` are clean.

---

## Phase 2: sirn files mode (core tracer bullet)

**User stories / goals**: Goals 1–3; divergences 1 (port from portplz), 2 (basename
routing), 3 (streaming + Content-Type/Length), 4 (empty→200), 5 (localhost default).

### What to build

The end-to-end tracer bullet: `sirn <file>...` stands up a real HTTP server on the
resolved port and serves the named files. CLI surface complete: positional
`[FILES]...`, `-p/--port`, `-b/--bind`, `--no-git`, `-v/--verbose`, `-V/--version`.
Port resolves from `portplz_core::derive(current_dir, no_git)` unless `--port` is
given; bind `127.0.0.1` unless `--bind` overrides. Each file is routed at
`/<basename>`; duplicate basenames abort startup with a clear error. Requests
stream the file from disk with `Content-Length` and an extension-derived
`Content-Type`; a missing file → `404`, a present-but-empty file → empty `200`. A
startup banner prints the version, resolved port (and derivation source under
`--verbose`), bind address, and the full URL of every served file.

### Acceptance criteria

- [ ] `src/sirn` binary exists; `tiny_http = "0.12"` added to `[workspace.dependencies]`;
      `sirn` depends on `portplz-core`, `clap`, `buildinfo`, `tiny_http`; dev-dep
      `tempfile`; `[lints] workspace = true`.
- [ ] CLI parses `[FILES]...`, `--port`, `--bind`, `--no-git`, `--verbose`,
      `--version`; `--version` emits the `buildinfo` string.
- [ ] Server binds `127.0.0.1` by default and the `--bind` address otherwise; uses
      `Arc<tiny_http::Server>` + a fixed worker-thread pool.
- [ ] Content-Type table (unit-tested): known extensions, case-insensitive lookup,
      `charset=utf-8` on `text/*`, unknown → `application/octet-stream`.
- [ ] Duplicate-basename arguments produce a startup error (unit-tested), including
      UTF-8 filenames.
- [ ] Integration tests (server on `127.0.0.1:0`, raw HTTP/1.0 GET helper over
      `std::net::TcpStream`): existing file → `200` with exact bytes + correct
      `Content-Type` + `Content-Length`; multi-chunk/large file streams correctly;
      empty file → `200` empty body; missing file and unregistered route → `404`.
- [ ] Startup banner prints each served file's full URL on the resolved port.
- [ ] `cargo test -p sirn` and `cargo clippy` are clean.

---

## Phase 3: Availability monitor

**User stories / goals**: faithful reproduction of the original Java availability
logging.

### What to build

A 1-second background polling thread (files mode) that tracks each served file's
existence and logs transitions, mirroring the Java wording: `Ready to serve <name>`
when a previously-missing file appears, and `Warning!  File <name> not found...`
when a served file disappears. Initial absence at startup is reported once. The
monitor runs alongside the worker pool and shuts down cleanly with the process.

### Acceptance criteria

- [ ] A monitor thread polls served files ~once per second and logs only on
      availability **transitions** (no repeated spam while state is unchanged).
- [ ] Log wording matches the spec: `Ready to serve <name>` and
      `Warning!  File <name> not found...`.
- [ ] The transition-detection logic is unit-tested (present→absent and
      absent→present produce exactly one message each; steady state produces none),
      using parallel-safe temp files.
- [ ] A file created after startup becomes fetchable (`200`) and logs
      `Ready to serve`; deleting it logs the not-found warning and a subsequent
      fetch returns `404` (integration-tested or via the transition unit).
- [ ] `cargo test -p sirn` and `cargo clippy` are clean.

---

## Phase 4: Directory mode

**User stories / goals**: divergence 6 (no-argument invocation serves the current
directory as a browsable tree).

### What to build

When `sirn` is invoked with no file arguments, serve `current_dir()` recursively.
Resolve each request path against the root, canonicalize it, and **confine it to
the root** — any traversal (`../…`) or symlink escape returns `403`. A request that
resolves to a directory renders an HTML listing (entries HTML-escaped, directories
marked, a `..` parent link except at the root); a file is streamed via the phase-2
serving path (same `Content-Type`/`Content-Length` behavior); anything missing →
`404`. The startup banner prints the root URL.

### Acceptance criteria

- [ ] No-file invocation enters directory mode and serves `current_dir()`.
- [ ] Path confinement is unit-tested: `../` sequences, absolute-path requests, and
      symlink-escape attempts are rejected; in-root paths resolve correctly.
- [ ] Directory-listing HTML generation is unit-tested: lists entries, HTML-escapes
      names (e.g. `a<b>.txt`), marks directories, includes a `..` link except at root;
      UTF-8 filenames don't panic.
- [ ] Integration tests: root request → `200` listing containing known entries;
      nested file fetch → `200` with correct bytes/headers; `GET /../../etc/passwd`-style
      traversal → `403`; missing path → `404`.
- [ ] `cargo test -p sirn` and `cargo clippy` are clean.

---

## Phase 5: Docs & convention sweep

**User stories / goals**: repo-convention checklist; discoverability.

### What to build

Wire `sirn` into the repository's documentation and run the final convention audit.
Update the repo-root tool listing/README (if one enumerates the tools) to include
`sirn` and `portplz-core`. Verify all repo conventions hold across the three crates:
`--version`/`-V` on both binaries, workspace lints enabled, UTF-8-safe string
handling (no byte-index slicing), no stray `--shell-setup`/`termbar`. Add concise
doc comments / usage text suitable for documentation generation.

### Acceptance criteria

- [ ] The repo-root tool listing/README includes `sirn` (and notes `portplz-core`)
      with a one-line description and example usage, if such a listing exists.
- [ ] Both binaries print the `buildinfo` version string for `--version`/`-V`.
- [ ] No byte-index string slicing in the new code; multi-byte filename tests pass.
- [ ] `cargo build`, `cargo test --workspace`, and `cargo clippy --workspace` are
      all clean.
- [ ] `sirn --help` reflects the full CLI surface from the spec.
