# Plan: kitchen-sync

> Source PRD: `docs/superpowers/specs/2026-04-13-kitchen-sync-design.md`

## Architectural decisions

Durable decisions that apply across all phases:

- **Crate location**: `src/kitchen-sync/` as a workspace member of the tools monorepo.
- **CLI shape**: `kitchen-sync <repo-url>` (single positional arg) and `kitchen-sync --version`.
- **Version output**: via `buildinfo::version_string!()` macro, matching every other tool in the workspace.
- **Dependencies**: `clap` (derive), `anyhow`, `buildinfo`, plus workspace `toml`, `glob`, `tempfile`.
- **Clone strategy**: shell out to `git clone --depth 1 <url>` into a `tempfile::tempdir()`. Temp directory is auto-cleaned on drop.
- **Install strategy**: shell out to `cargo install --git <url> <name>` per package (or bare `cargo install --git <url>` for single-package repos). Sequential, not parallel.
- **Binary detection**: a member is "binary" iff its `Cargo.toml` has a `[[bin]]` section OR `src/main.rs` exists. Library-only crates are silently skipped.
- **Exit code**: `0` if at least one install succeeded; `1` if all installs failed or clone/discovery failed.
- **Error model**: clone and discovery errors abort. Individual install failures are collected and reported in the final summary; the tool continues with remaining packages.

---

## Phase 1: Scaffold + single-package install

**User stories**: CLI interface, version output, clone flow, single-package repo install.

### What to build

A minimum-viable `kitchen-sync` that works end-to-end for non-workspace repos. Scaffold the new `src/kitchen-sync/` crate as a workspace member following the monorepo's standard layout. Implement CLI parsing with clap, wire up the `buildinfo` version macro, and accept a single positional repo URL.

When invoked, shallow-clone the URL into a temp directory. Detect whether the root is a single-package repo by checking for `[[bin]]` in `Cargo.toml` or for `src/main.rs`. If so, run `cargo install --git <url>` and stream its output to the user. Exit 0 on install success, 1 on failure.

Workspace repos are not yet supported — if the root `Cargo.toml` contains `[workspace]`, exit with a "not yet implemented" message. This phase is demoable against any single-binary repo (e.g., a simple Rust CLI on GitHub).

### Acceptance criteria

- [ ] `src/kitchen-sync/` exists as a workspace member in the root `Cargo.toml`
- [ ] `kitchen-sync --version` prints `kitchen-sync 0.1.0 (<hash>, clean|dirty)` via buildinfo
- [ ] `kitchen-sync --help` shows usage with the positional repo URL
- [ ] Running against a single-package repo URL clones it, runs `cargo install --git <url>`, and exits 0 on success
- [ ] `cargo install` failure propagates as exit code 1
- [ ] Running against a workspace repo prints a clear "workspace support not yet implemented" message (placeholder for Phase 2)
- [ ] Temp clone directory is cleaned up after the run
- [ ] `cargo check` and `cargo clippy` pass on the new crate

---

## Phase 2: Workspace discovery + multi-package install

**User stories**: workspace member parsing, glob resolution, binary-only filtering, per-package install loop, summary output.

### What to build

Extend `kitchen-sync` to handle Cargo workspaces end-to-end. When the root `Cargo.toml` declares a workspace, parse `workspace.members` and resolve any glob patterns (e.g., `src/*`) against the cloned directory using the `glob` crate.

For each resolved member, inspect its `Cargo.toml` and filesystem to decide whether it produces a binary. Members with a `[[bin]]` section or a `src/main.rs` file qualify; library-only members are silently skipped. Extract the `package.name` from each qualifying member's `Cargo.toml` and print the full list before installing.

Install each package sequentially via `cargo install --git <url> <name>`. Stream per-install progress to the user (e.g., `Installing freeport (1/12)...`). Track which packages succeeded and which failed, collecting error output for failures. After the loop, print a summary: `N installed, M failed` and list failed package names.

This phase is demoable against the `tools` monorepo itself — pointing `kitchen-sync` at it should install every binary tool it contains.

### Acceptance criteria

- [ ] Root `Cargo.toml` with `[workspace]` is parsed and `workspace.members` is extracted
- [ ] Glob patterns in `workspace.members` (e.g., `src/*`) are resolved to concrete member directories
- [ ] Members without `[[bin]]` and without `src/main.rs` are filtered out (library-only crates skipped)
- [ ] Package names are extracted from each binary member's `Cargo.toml`
- [ ] A plan line is printed listing the packages that will be installed
- [ ] Each package is installed via `cargo install --git <url> <name>` in sequence
- [ ] Per-package progress is printed in the format `Installing <name> (i/N)...`
- [ ] Individual install failures do not abort the run; remaining packages still attempt to install
- [ ] Final summary prints install count, failure count, and names of failed packages
- [ ] Exit code is `0` if at least one install succeeded, `1` if all failed
- [ ] `cargo check` and `cargo clippy` pass

---

## Phase 3: Error handling polish

**User stories**: clone-failure messages, "not a Rust repo" handling, "no binary packages" handling, clean user-facing error surface.

### What to build

Harden the error surface so users get clear, actionable messages for every documented failure mode in the spec.

Clone failures (bad URL, network error, non-git URL) should exit with a descriptive message including the underlying `git` error. If the cloned repo has no `Cargo.toml` at its root, exit with `Not a Rust project (no Cargo.toml found)`. If the repo is a workspace but every member is library-only (or `workspace.members` is empty), exit with `No binary packages found in repository`. If the repo is neither a workspace nor a single-package binary repo, exit with the same "no binary packages" message.

Make sure the summary output prints the captured error message for each failed package so users can diagnose without re-running. UTF-8 safe throughout — repo URLs and package names may contain non-ASCII characters; no byte-level string slicing.

This phase is demoable by walking through each failure path: bogus URL, URL to a non-Rust repo, workspace-with-only-libs, clean install, partial failure.

### Acceptance criteria

- [ ] Invalid or unreachable URLs produce a clear error message that includes the underlying git error, exit code 1
- [ ] Cloned repo with no root `Cargo.toml` exits with `Not a Rust project (no Cargo.toml found)`, exit code 1
- [ ] Workspace with zero binary members exits with `No binary packages found in repository`, exit code 1
- [ ] Repo that is neither a workspace nor a single-package binary crate exits with the same "no binary packages" message
- [ ] Failed-package entries in the final summary include the captured error output, not just the package name
- [ ] All string handling is UTF-8 safe (no `&s[..n]` byte slicing; use `chars()` where truncation is needed)
- [ ] Manual demo walks through each failure path and produces the expected message and exit code
- [ ] `cargo check`, `cargo clippy`, and `cargo fmt --check` all pass on the final crate
