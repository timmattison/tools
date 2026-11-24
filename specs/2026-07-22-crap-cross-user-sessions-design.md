# crap: cross-user session discovery

**Date:** 2026-07-22
**Status:** Approved (design); pending implementation plan
**Component:** `src/crap`
**Type:** Feature addition

## Motivation

`crap` resumes a Claude Code session from whatever directory you're standing in,
by looking the session id up under `~/.claude/projects`, recovering the directory
it ran in, and re-launching `claude --resume`. Every lookup is anchored on
`dirs::home_dir()` — it only ever sees the **current user's** sessions.

On a machine with more than one account, a session started under another user is
invisible to `crap`, even when the current user can read that user's files. The
concrete driving case: two accounts on one Mac, both in group `staff`; the user
wants to resume a session that another account (`scyloswork`) started, from their
own (`timmattison`) account.

This spec adds cross-user discovery: `crap` can find a session under another
user's home and resume it safely, without ever writing to the other user's files
and without escalating privilege on its own.

## The central constraint

`claude --resume <id>` reads the projects tree of **whoever runs it**. If
`timmattison` runs it, Claude looks under `/Users/timmattison/.claude/projects`
and nowhere else. So even after `crap` reads a foreign transcript, a plain
`claude --resume <id>` launched by the current user cannot find it — the file
lives under the *other* user's home.

The only mechanism that works cross-user is therefore:

1. **Copy** the foreign transcript into the current user's own projects tree, and
2. **Fork-resume** it (`--fork-session`, a fresh id), so the current user owns a
   clean new session and the original transcript is only ever read.

This is exactly the machinery `--here` already implements (symlink + fork +
cleanup). The feature is mostly: widen the *source* search across users, and
**copy** instead of symlink when the source belongs to another user.

## On-disk reality that shaped the design

Observed on the target machine (recorded here because it drives several
decisions):

- Two accounts (`timmattison`, `scyloswork`), both in group `staff`.
  `timmattison` is also in `admin`, so `sudo` works but is interactive.
- `/Users/scyloswork/.claude/projects` is world-readable (`755`), **but the
  per-project subdirectories are mixed**: some are `755` (readable), many are
  `700` (owner-only). Group `staff` gets nothing on a `700` directory, and the
  search bit is required to reach files inside it, so a `700` project dir is
  entirely opaque to the current user without `sudo`.
- A session in a `700` dir (e.g. the driving case's
  `1593d0cf-…` under `…-hypersive-newco-backend-worktrees-aws-accounts-reuse`)
  cannot be read by `crap` at all. Its recorded `cwd` is a shared
  `/Volumes/SamsungSSDs/…` path that *is* reachable — so the blocker is reading
  the transcript, not reaching the working directory.

Consequences baked into the design: (a) the source search must gracefully skip
unreadable dirs, (b) `crap` must *detect* the "it's probably in an owner-only
dir" case and guide the user, and (c) `crap` must never run `sudo` itself.

## Goals

1. Resume a session that lives under another user's `~/.claude/projects`, when the
   current user can read it — copying it into the current user's tree and
   forking, never touching the original.
2. Two discovery paths: an explicit `--user <name>` target, and, with no flag, an
   automatic fallback that scans other users' homes after the current user's own
   tree yields nothing.
3. When the requested id isn't found in any *readable* location but owner-only
   dirs were skipped, print actionable guidance (which user, how to make it
   readable) instead of a bare "not found". Never escalate privilege.
4. Preserve today's single-user behavior and its contracts exactly (see below).
5. Honor the repo's mandatory TDD workflow and parallel-safe test rules.

## Scope

- New `--user <NAME>` flag on `crap` (resume, `--here`, and `--status <id>`
  forms).
- Self-first discovery with automatic cross-user fallback when no `--user` is
  given.
- Cross-user resume via copy-into-own-tree + fork, landing at the **original
  cwd** (or, with `--here`, the current dir).
- Detection of skipped owner-only dirs and actionable guidance.
- New shell-protocol sentinel for "cd to original cwd **and** fork" (cross-user
  default resume).
- README + TLDR documentation; shell function passes `--user` through and grows
  the new sentinel branch.

## Out of scope

- **Privilege escalation.** `crap` never runs `sudo`. The user does that manually
  if a session sits in an owner-only dir (guidance is printed).
- **No-id `crap --status`** cross-user. That form lists sessions for the current
  directory, which is inherently the current user's; cross-user does not apply.
  Only `crap --status <id>` gains `--user`/fallback.
- **Writing to another user's tree.** Every write (the copy, the fork's new
  transcript) lands under the current user's home. The foreign transcript is only
  ever read.
- **Non-conventional home locations.** Users are resolved as siblings of the
  current user's home (`<home>/../<name>`), i.e. `/Users/*` on macOS and
  `/home/*` on Linux. A user whose home is elsewhere is not discovered. (See
  *Alternatives considered*.)
- **Windows cross-user.** The sibling-enumeration is structurally
  platform-neutral, but Windows ACLs and the `C:\Users` layout are untested; this
  feature targets macOS/Linux.

## Behavior

### Discovery

- `crap <id>` (no flag): search the **current user's** projects tree first
  (today's fast path, byte-for-byte unchanged). Only if the id is not found there,
  fall back to enumerating every sibling home that has a `.claude/projects`
  directory and search each. Self-first ordering guarantees a session the current
  user already owns always wins, so a UUID that happens to exist in two trees is
  never resolved to the foreign one.
- `crap <id> --user <name>`: search **only** `<name>`'s projects tree. Skips the
  current user's tree entirely, so `--user` is also how you disambiguate on
  purpose.

### Resume semantics

- **Same-user hit** → today's behavior, unchanged: `cd` to the recorded cwd and
  `claude --resume <id>` (same id, resumed in place).
- **Cross-user hit** → copy the transcript into the current user's tree and
  **fork** (`--fork-session`, fresh id). The original is only read, so this is
  safe even while the other user is live in that session. The copy is removed once
  Claude writes the forked transcript (same watcher/cleanup the current `--here`
  uses).
  - **Fork location = the original cwd.** `crap <id> --user X` lands you in the
    session's recorded working directory, forked. If that directory is missing
    **or** exists but the current user cannot enter it, `crap` **errors** —
    exactly as today's default mode errors with `DirectoryMissing` rather than
    silently substituting the current directory. The error hints:
    "use `crap --here <id>` to fork it in the current directory instead."
  - `crap --here <id> [--user X]` is the explicit current-directory escape hatch.
    It already ignores the original cwd, so it keeps working even when that cwd is
    gone or unreachable; it now also accepts a cross-user source (copying instead
    of symlinking).

### Permission handling — detect + guide, never escalate

While scanning a user's projects tree, any subdirectory that fails to open with
`PermissionDenied` is silently skipped, **but recorded** together with the owning
user. If the requested id is not found in any readable location and at least one
such dir was skipped, `crap` prints guidance naming the user and a copy-paste
remedy, e.g.:

```
Error: no readable Claude session found with id '1593d0cf-…'
       12 project dirs under user 'scyloswork' are owner-only and were skipped.
       If the session is one of those, resume it as that user, or make it
       readable first, for example:
         sudo -u scyloswork cat <path/to/id.jsonl> \
           > ~/.claude/projects/<encoded-cwd>/<id>.jsonl
```

`crap` itself never runs `sudo`.

## Architecture

### Search roots — `UserProjects`

A small newtype-flavored struct describes one search root:

```rust
/// One user's `~/.claude/projects` directory, tagged with who owns it.
struct UserProjects {
    /// The account name (the home directory's file name).
    user: String,
    /// That user's `.../.claude/projects` directory.
    projects_dir: PathBuf,
    /// Whether this is the invoking user's own tree.
    is_self: bool,
}
```

Constructors, all taking explicit inputs so they are tempdir-testable (no direct
`home_dir()` reads inside the pure logic):

- `self_projects(home: &Path) -> UserProjects` — `home/.claude/projects`,
  `is_self = true`.
- `user_projects(users_parent: &Path, name: &str, self_name: &str) -> UserProjects`
  — `users_parent/name/.claude/projects`; `is_self = name == self_name`.
- `enumerate_user_projects(users_parent: &Path, self_name: &str) -> Vec<UserProjects>`
  — one entry per sibling directory that actually contains a `.claude/projects`
  dir; the current user's own entry is included and marked `is_self`. Ordered
  self-first, then the rest deterministically (by user name) so output is stable.

The env-coupled glue lives in one thin place: `dirs::home_dir()` gives `home`;
`home.file_name()` gives `self_name`; `home.parent()` gives `users_parent`. That
glue is the only part not covered by pure tests.

### Finding across roots

```rust
/// Outcome of searching an ordered list of roots for a session id.
enum FoundSession {
    /// The transcript, and which root (hence user / is_self) it came from.
    Found { path: PathBuf, root: UserProjects },
    /// Not found in any readable location; carries the owner-only dirs skipped
    /// during the scan, so the caller can print actionable guidance.
    NotFound { skipped: Vec<SkippedDir> },
}

/// A project dir the scan could not enter.
struct SkippedDir { user: String, dir: PathBuf }
```

`find_session_across(roots: &[UserProjects], id: &str) -> FoundSession` walks the
roots in order. Within a root it lists `projects_dir`, and for each subdirectory
probes for `<id>.jsonl`; a subdir that returns `PermissionDenied` is pushed to
`skipped` (tagged with the root's user) rather than ignored. First match wins and
short-circuits.

This keeps the module deep: callers ask one question ("find this id, possibly
across users") and get back either a located file tagged with its owner, or a
structured miss that already contains everything needed for guidance. The
copy-vs-cd-vs-fork policy lives above it, in the run-mode functions.

`find_session_file` (single-dir) is retained and reused as the per-root inner
loop; `resolve_session_dir`, `resolve_here_link`, and `resolve_status_report` are
refactored to take the resolved *root* (or list of roots) instead of a bare
`projects_dir`.

### Import: copy vs symlink

`prepare_here_link` generalizes to `prepare_import`:

```rust
enum ImportMode { Symlink, Copy }

fn prepare_import(
    dest_projects_dir: &Path,   // always the current user's tree
    source_jsonl: &Path,        // may be under another user's home
    target_dir: &Path,          // current dir (--here) or original cwd (default)
    session_id: &str,
    mode: ImportMode,
) -> std::io::Result<Option<PathBuf>>
```

- Same-user `--here` → `Symlink` (today's behavior, unchanged).
- Cross-user (either `--here` or default) → `Copy`: a self-contained snapshot in
  the current user's tree, owned by the current user, cleaned up like the symlink.
- The existing "anything already at the target name means it's already resolvable,
  return `Ok(None)`" guard is preserved for both modes, so a repeat import is a
  no-op.

Because the fork writes a fresh-id file and we remove the imported `<id>.jsonl`
afterward, a cross-user copy is transient — it exists only for the moment Claude
reads it at startup.

### Shell protocol

The binary → shell-function contract gains one variant. Existing shapes are
untouched:

- **default (same-user)** — `<id>\n<dir>`: `cd <dir>`, `claude --resume <id>`.
- **`--here`** — `__CRAP_HERE__\n<id>\n<newid|sentinel>\n<link|sentinel>`: stay
  put, fork, clean up the link.

New:

- **cross-user default** — `__CRAP_FORK_AT__\n<id>\n<newid|sentinel>\n<link|sentinel>\n<dir>`:
  `cd <dir>` (the original cwd), then run the same fork + cleanup sequence as
  `--here`. `<dir>` is emitted **last** so a working directory containing an
  embedded newline still survives as "everything after the final field
  separator"; `<link>` is a path under `~/.claude/projects/<encoded>` whose
  encoding maps every non-alphanumeric character (including newline) to `-`, so it
  is newline-free and unambiguous as a middle field — the same invariant the
  existing `--here` output already relies on.

`--user <name>` is two ordinary tokens; it is **not** added to the shell
function's straight-through `case` (which only matches `--status`, `--help`, `-h`,
`--version`, `-V`, `--shell-setup`), so it flows through the normal resolve path
and is parsed by the binary. No shell-side handling of `--user` is needed beyond
passing `"$@"` through, which already happens.

### Errors and exit codes

Add to `mod exit_codes`:

- `DIRECTORY_UNREADABLE` — the original cwd of a cross-user session exists but the
  current user cannot enter it (distinct from `DIRECTORY_MISSING`, which stays for
  "does not exist"). Both print the `--here` hint.
- `INVALID_USER` — `--user` names a sibling that does not exist or has no
  `.claude/projects`.

The existing `SESSION_NOT_FOUND` path is extended: when the miss carries skipped
owner-only dirs, it prints the detect-and-guide message above (same exit code, a
richer message). Malformed-id and other existing codes are unchanged.

## Testing plan (TDD, red → green per behavior)

All tests use process-unique tempdirs (per the repo's parallel-safe rule); none
touch a real home. `is_alive` stays injected. New behaviors, each its own
red/green pair:

1. `user_projects` builds `<parent>/<name>/.claude/projects` and sets `is_self`
   correctly for self vs other.
2. `enumerate_user_projects` includes only siblings that have `.claude/projects`,
   marks the self entry, and orders self-first then by name.
3. `find_session_across` finds a foreign id when absent from self; prefers the
   self copy when the id exists in both (self-first).
4. `find_session_across` records a `PermissionDenied` subdir as a `SkippedDir`
   tagged with the owning user. **Unix-gated, and skipped when running as root**
   (root bypasses `700`, so the case is unobservable) — the test `chmod`s a
   subdir to `0o000`/`0o700` and asserts it is skipped, not matched.
5. `prepare_import` in `Copy` mode snapshots the source into the target folder;
   a second call is a no-op (`Ok(None)`).
6. `prepare_import` in `Symlink` mode is byte-identical to today's
   `prepare_here_link` behavior (regression guard).
7. Cross-user default output formatter emits the `__CRAP_FORK_AT__` shape with
   `<dir>` last and the sentinels in the right slots.
8. Original-cwd unreachable → the decision function selects the
   `DIRECTORY_UNREADABLE` error with the `--here` hint, rather than falling back
   to the current dir.
9. Guidance message names the owning user and includes a remedy, only when the
   miss carries skipped dirs.
10. CLI: `--user` parses and carries a value; composes with `--here` and
    `--status`; is rejected with `--shell-setup`; a bare `--user` with no value is
    a parse error.

Then the standard feedback loop: `cargo test -p crap`, `cargo clippy -p crap`,
`cargo fmt`.

## Edge cases

- **Same id in both trees** — self-first resolves to the current user's copy; the
  foreign one is never reached. No special handling needed.
- **Original cwd exists but is owner-only** — `DIRECTORY_UNREADABLE`, with the
  `--here` hint (the driving session's cwd is a shared `/Volumes` path, so this is
  the uncommon branch, but it must not silently fall back).
- **`--user` names a nonexistent account** — `INVALID_USER`, listing the users
  that *do* have a `.claude/projects` (best-effort, from the sibling scan).
- **Foreign session is live in the other user's process** — irrelevant: cross-user
  always forks (reads only), and `should_block_for_live` is already false for
  forks, so liveness never blocks it. (The other user's `~/.claude/sessions`
  registry may itself be unreadable; the fork path never consults it.)
- **Auto-fallback finds nothing anywhere, no skipped dirs** — plain
  `SESSION_NOT_FOUND`, as today.

## Alternatives considered

- **Adopt the same id at the original cwd (no fork).** Copying the foreign
  transcript to `~/.claude/projects/<encode(original_cwd)>/<id>.jsonl` and
  resuming the *same* id in place would reuse the existing `<id>\n<dir>` protocol
  with no new sentinel. Rejected in favor of a fresh forked id (the chosen resume
  mode): a fork gives the current user a clean, unambiguous new session and avoids
  two transcripts sharing one id across the two homes.
- **Symlink the foreign transcript instead of copying.** Would avoid the transient
  copy, but points a link from the current user's tree into another user's home
  (fragile if the source moves, and requires a live cross-home read at fork
  startup). A copy is self-contained and clean; the extra disk is one transcript
  for a few seconds.
- **Resolve homes via the system user database (`getpwnam`/`dscl`).** More correct
  for non-`/Users` homes, but adds a dependency/complexity for a case that does
  not occur on the target machines. Sibling enumeration off `home.parent()` is
  simple, portable across macOS/Linux, and fully tempdir-testable. Documented as a
  known limitation.
- **Scan other users' homes by default with no opt-out.** More convenient but
  scans everyone's homes unconditionally. The self-first-then-fallback model keeps
  the common path unchanged and only reaches into other homes when the current
  user's own tree misses, which is both faster and less surprising.

## Documentation

- `README.md` and `TLDR.md`: document `--user`, the auto-fallback, the copy+fork
  cross-user semantics, and the owner-only-dir guidance.
- The tool's module doc comment (top of `main.rs`) gains a paragraph on cross-user
  discovery alongside the existing `--here`/`--status` descriptions.

## Immediate unblock for the driving session

The feature does not help the specific `1593d0cf-…` session until it is readable,
and its project dir is `700`. Until then it can be resumed manually (run in the
directory you want it in; `sudo` will prompt):

```bash
SID=1593d0cf-3146-46dc-8209-d4bb06d25c11
SRC=$(sudo find /Users/scyloswork/.claude/projects -maxdepth 2 -name "$SID.jsonl")
DEST=~/.claude/projects/$(pwd | sed 's/[^A-Za-z0-9]/-/g')   # matches crap's encode_project_dir
mkdir -p "$DEST"
sudo cat "$SRC" > "$DEST/$SID.jsonl"   # redirect runs as you → file is yours; scyloswork's untouched
crap --here "$SID"                      # forks it here with a fresh id
```

Once the feature ships and the dir is readable, this becomes
`crap "$SID" --user scyloswork`.
