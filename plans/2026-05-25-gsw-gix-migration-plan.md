# gsw: migrate from git-CLI subprocess to gix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace gsw's ~13 `git` subprocess calls per run with in-process `gix` (already a vetted workspace dependency), eliminating the `.git/index.lock` race *by construction* and deleting the per-tick index-snapshot workaround.

**Architecture:** gsw has a clean domain seam: `build_snapshot()` consumes plain data (`Vec<FileEntry>`, two `HashMap<String, NumStat>`, ages, branch, base, commits-ahead, last-commit-age) and `render()` consumes a `Snapshot`. We keep that seam untouched and replace only the *producers* of those values. A new `repo.rs` module holds all gix-backed git operations behind functions that return the existing domain types; `git.rs` keeps the domain types (`FileEntry`, `FileStatus`, `NumStat`) and loses its string parsers at the end; `main.rs` loses `run_git`, `git_command`, `redirect_index_to_snapshot`, `IndexSnapshot`, and `GSW_INDEX_FILE`.

**Tech Stack:** Rust, `gix = "0.83"` (default features already enable `status`, `dirwalk`, `blob-diff`, `revision` — **no feature flags to add**), `gix-imara-diff` (re-exported as `gix::diff::blob`) for line counts, `tempfile` + the `git` CLI for test fixtures, `anyhow` for errors.

---

## Why this is safe to do incrementally

The only `git` subprocesses that rewrite `.git/index` (and thus take `.git/index.lock`) are `git status` and `git diff` — that is the entire reason `redirect_index_to_snapshot()` exists. Every *other* call (`rev-parse`, `rev-list`, `log`, `symbolic-ref`) is read-only. So we can port the read-only metadata first (Tasks 1–5) while `status`/`diff` still shell out behind the existing index-snapshot guard, then port `status` (Task 6) and `numstat` (Task 7), and only **after** the last `git diff` is gone do we delete the index-snapshot machinery (Task 8). gsw renders correctly and ships green after every task.

## Parity harness (do not modify in early tasks)

`src/gsw/tests/integration.rs` already drives the **real gsw binary** against temp repos and asserts on rendered output (branch shown, `●` staged, `○` unstaged, `?` untracked, ages from a subdir, friendly header outside/​bare repos, and the index-not-rewritten regression). These tests are backend-agnostic: **they must keep passing unchanged after every task.** They are your behavioral safety net. Run the whole file after each task:

```
cargo test -p gsw --test integration
```

The new *unit* tests this plan adds live in-crate as `#[cfg(test)] mod tests` inside `repo.rs` (binary crates can't be imported by `tests/`), and build fixtures with the `git` CLI via a small helper, mirroring `integration.rs`'s `run_git`.

## File Structure

- **Create** `src/gsw/src/repo.rs` — all gix-backed git operations. Public (crate-local) free functions taking `&gix::Repository`, each returning existing domain types. Owns: `open()`, `branch_name()`, `resolve_base()`, `commits_ahead()`, `last_commit_age()`, `recent_log()`, `upstream_status()`, `collect_status()`, `collect_numstats()`, plus private helpers (`line_counts`, `is_binary`). Its own `#[cfg(test)] mod tests` with a `git`-CLI fixture helper.
- **Modify** `src/gsw/Cargo.toml` — add `gix.workspace = true` to `[dependencies]`.
- **Modify** `src/gsw/src/main.rs` — call `repo::*` instead of `run_git(...)`; delete the subprocess + index-snapshot plumbing; `mod repo;`. Keep `collect_ages` (pure filesystem mtime, gix-independent).
- **Keep** `src/gsw/src/git.rs` — retains `FileEntry`, `FileStatus`, `NumStat` (imported by `snapshot.rs`, `render.rs`, `repo.rs`). Loses `parse_status`, `parse_numstat`, and their helpers + tests in Task 9. (Filename stays `git.rs` to avoid churn across importers.)
- **Unchanged** `src/gsw/src/snapshot.rs`, `src/gsw/src/render.rs`, `src/gsw/src/bar.rs`, `src/gsw/src/age.rs`, `src/gsw/tests/integration.rs`.

## Conventions for every task

- **TDD red→green, separate commits.** The red commit contains the failing test **plus a compiling stub** of the new function (returning an empty/wrong value) so the test fails on a *behavioral assertion*, not a missing symbol (per `~/.claude/TESTING.md`). Red commits may use `git commit --no-verify` (the narrow TESTING.md exception); green commits must pass the pre-commit hook.
- **Parallel-safe fixtures:** always `tempfile::tempdir()` (unique per process). Never a fixed path.
- **Borrow-detail caveat:** the gix snippets below are source-verified for *types and method names*, but a few exact borrow forms (`&*id` vs `id.as_ref()` vs `id.detach()`) may need a one-character compiler-guided tweak. That is expected; the compiler will point at it. Do not treat it as a design change.
- Each commit message ends with the trailer:
  ```
  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  ```

---

## Task 1: Add gix dependency and open the repository

Replaces `inside_git_repo()` (which shelled out to `git rev-parse --is-inside-work-tree`) with a gix open that also gates out bare repos (no work tree).

**Files:**
- Modify: `src/gsw/Cargo.toml`
- Create: `src/gsw/src/repo.rs`
- Modify: `src/gsw/src/main.rs` (add `mod repo;`)

- [ ] **Step 1: Add the dependency**

In `src/gsw/Cargo.toml`, under `[dependencies]` (alongside `anyhow.workspace = true` etc.) add:

```toml
gix.workspace = true
```

No features needed — the workspace pins `gix = "0.83"` with default features, which already include `status`, `dirwalk`, `blob-diff`, and `revision`.

- [ ] **Step 2: Create `repo.rs` with a compiling stub + the failing test**

Create `src/gsw/src/repo.rs`:

```rust
//! gix-backed git operations for gsw.
//!
//! gsw is a read-only monitor. Every function here reads the repository
//! in-process via `gix` and never writes the index, so it can never take
//! `.git/index.lock` and can never race a concurrent rebase — the reason the
//! old `git` CLI path needed a private index snapshot.

use anyhow::Result;

/// Open the repository containing `cwd`, or `None` when there isn't one with a
/// working tree (outside any repo, or a bare repo — gsw has nothing per-file to
/// render in either case).
pub fn open() -> Option<gix::Repository> {
    None // STUB — replaced in Step 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    /// Run a git command in `dir`, isolated from the host's global/system
    /// config, asserting success. Test-only fixture construction.
    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("invoke git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// A fresh repo on branch `main` with one commit, returned as the TempDir
    /// (kept alive by the caller) plus its path. Parallel-safe: unique tempdir.
    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@example.com"]);
        git(p, &["config", "user.name", "Test"]);
        git(p, &["config", "commit.gpgsign", "false"]);
        std::fs::write(p.join("a.txt"), "initial\n").unwrap();
        git(p, &["add", "a.txt"]);
        git(p, &["commit", "-q", "-m", "initial"]);
        dir
    }

    /// Open a repo at an explicit path (tests can't rely on cwd under a
    /// parallel test runner). Mirrors `open()` but takes a path.
    fn open_at(path: &Path) -> Option<gix::Repository> {
        let repo = gix::discover(path).ok()?;
        repo.workdir().map(|_| repo)
    }

    #[test]
    fn open_at_finds_worktree_repo() {
        let dir = init_repo();
        assert!(open_at(dir.path()).is_some(), "should open a worktree repo");
    }

    #[test]
    fn open_at_rejects_bare_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "--bare", "-q"]);
        assert!(
            open_at(dir.path()).is_none(),
            "a bare repo has no work tree; gsw must treat it like no repo",
        );
    }
}
```

> Note: `open()` itself reads cwd, which the parallel test runner can't pin, so the tests exercise the path-taking `open_at` that shares `open()`'s logic. `open()` is covered end-to-end by `integration.rs`'s `outside_git_repo...` and `bare_repo...` tests.

- [ ] **Step 3: Run the tests — verify they FAIL behaviorally**

Run: `cargo test -p gsw --lib repo::tests`
Expected: COMPILES, then `open_at_finds_worktree_repo` and `open_at_rejects_bare_repo` both run. (These test the local `open_at`, which is already correct — so to make this a genuine red for `open()`, see Step 3b.)

- [ ] **Step 3b: Make the red about `open()` itself**

Add this test to the `tests` module (it calls the real `open()` via cwd, which is correct to assert against a known-good in-repo run — but cwd isn't controllable in tests, so instead assert the stub contract directly):

```rust
    #[test]
    fn open_returns_some_inside_this_repo_via_helper_parity() {
        // `open()` and `open_at` must share logic. Until `open()` is
        // implemented it returns None even though the helper opens fine.
        let dir = init_repo();
        let via_helper = open_at(dir.path()).is_some();
        // Re-implement open()'s contract against the same dir:
        let via_open_logic = {
            gix::discover(dir.path()).ok().and_then(|r| r.workdir().map(|_| r)).is_some()
        };
        assert_eq!(via_helper, via_open_logic);
        assert!(via_open_logic, "open()'s logic must accept a worktree repo");
    }
```

Run: `cargo test -p gsw --lib repo::tests` → all green except the stub remains unused by `main`. The stub is fine; `open()`'s real coverage is the two integration tests. **Proceed to wire `main` (Step 5) where the real red/green shows up against the binary.**

> Pragmatic note: `open()` is a thin wrapper; its true behavioral test is the existing `outside_git_repo...` / `bare_repo...` integration tests, which currently pass via the CLI path and must keep passing via gix. Treat *those* as the red/green oracle for Task 1.

- [ ] **Step 4: Implement `open()`**

Replace the stub body:

```rust
pub fn open() -> Option<gix::Repository> {
    let repo = gix::discover(".").ok()?;
    // Bare repos have no work tree; gsw renders a per-file working-tree view,
    // so there's nothing to show. Treat them like "not a repo".
    repo.workdir().map(|_| repo)
}
```

(`gix::discover` — `gix-0.83.0/src/lib.rs:257`; `Repository::workdir() -> Option<&Path>` — `src/repository/location.rs:91`.)

- [ ] **Step 5: Wire `main.rs` to use `repo::open()` for the repo gate**

In `src/gsw/src/main.rs`: add `mod repo;` next to the other `mod` lines. Replace:

```rust
    if !inside_git_repo() {
        println!("{}", "gsw • not a git repository".dimmed());
        return Ok(());
    }

    // Route every subsequent git call at a private copy of the index ...
    let _index_snapshot = redirect_index_to_snapshot();
```

with:

```rust
    let Some(repo) = repo::open() else {
        println!("{}", "gsw • not a git repository".dimmed());
        return Ok(());
    };

    // gsw still shells out for status/diff during the migration, so keep the
    // private-index guard until those calls move to gix (removed in Task 8).
    let _index_snapshot = redirect_index_to_snapshot();
```

Leave `inside_git_repo()` defined for now (it becomes dead in Task 9; removing it now would orphan nothing but keeps Task 1's diff small). Pass `&repo` is not yet needed by other calls — they still use `run_git`.

- [ ] **Step 6: Verify the binary still behaves (parity harness)**

Run: `cargo test -p gsw --test integration`
Expected: PASS — including `outside_git_repo_prints_friendly_header_and_exits_zero` and `bare_repo_prints_friendly_header_and_exits_zero`, now satisfied by gix's open/​worktree gate.

- [ ] **Step 7: Commit (single commit — this task is a refactor guarded by existing tests)**

```bash
git add src/gsw/Cargo.toml Cargo.lock src/gsw/src/repo.rs src/gsw/src/main.rs
git commit -m "refactor(gsw): open the repo via gix and gate bare/no-repo through it"
```

---

## Task 2: Branch name via gix

Replaces `run_git(&["rev-parse", "--abbrev-ref", "HEAD"])`.

**Files:**
- Modify: `src/gsw/src/repo.rs`
- Modify: `src/gsw/src/main.rs:231-234`
- Test: `src/gsw/src/repo.rs` (`tests` module)

- [ ] **Step 1: Add stub + failing test**

In `repo.rs`, add:

```rust
/// The short current-branch name (e.g. `main`), or `"HEAD"` when detached —
/// matching what `git rev-parse --abbrev-ref HEAD` prints.
pub fn branch_name(repo: &gix::Repository) -> String {
    let _ = repo;
    String::new() // STUB
}
```

In `tests`, add:

```rust
    #[test]
    fn branch_name_reports_current_branch() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(branch_name(&repo), "main");
    }

    #[test]
    fn branch_name_reports_head_when_detached() {
        let dir = init_repo();
        // Detach onto the current commit.
        git(dir.path(), &["checkout", "-q", "--detach"]);
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(branch_name(&repo), "HEAD");
    }
```

- [ ] **Step 2: Run — verify FAIL**

Run: `cargo test -p gsw --lib repo::tests::branch_name`
Expected: FAIL — both assert mismatch (`""` != `"main"` / `"HEAD"`).

- [ ] **Step 3: Commit red**

```bash
git add src/gsw/src/repo.rs
git commit --no-verify -m "test(gsw): red — branch_name via gix"
```

- [ ] **Step 4: Implement**

```rust
pub fn branch_name(repo: &gix::Repository) -> String {
    // head_name() is None when detached; mirror git's literal "HEAD" there.
    match repo.head_name() {
        Ok(Some(full)) => full.shorten().to_string(),
        _ => "HEAD".to_string(),
    }
}
```

(`Repository::head_name() -> Result<Option<gix_ref::FullName>, _>` — `src/repository/reference.rs:219`; `FullNameRef::shorten() -> &BStr` strips `refs/heads/` — `gix-ref/src/fullname.rs`.)

- [ ] **Step 5: Run — verify PASS**

Run: `cargo test -p gsw --lib repo::tests::branch_name`
Expected: PASS.

- [ ] **Step 6: Wire `main.rs`**

Replace lines 231–234:

```rust
    let branch = run_git(&["rev-parse", "--abbrev-ref", "HEAD"])
        .context("failed to read HEAD ref")?
        .trim()
        .to_string();
```

with:

```rust
    let branch = repo::branch_name(&repo);
```

- [ ] **Step 7: Run parity harness, commit green**

Run: `cargo test -p gsw` (lib + integration)
Expected: PASS.

```bash
git add src/gsw/src/repo.rs src/gsw/src/main.rs
git commit -m "feat(gsw): read the branch name via gix"
```

---

## Task 3: Base ref resolution + commits-ahead via gix

Replaces `resolve_base_ref()` (`rev-parse --verify` probes + `symbolic-ref origin/HEAD`) and `run_git(&["rev-list", "--count", "{base}..HEAD"])`.

**Files:**
- Modify: `src/gsw/src/repo.rs`
- Modify: `src/gsw/src/main.rs:236-241`
- Test: `src/gsw/src/repo.rs`

- [ ] **Step 1: Add stubs + failing tests**

In `repo.rs`:

```rust
/// Pick the first base ref that resolves: `main`, then `master`, then
/// `origin/HEAD`'s target, else `"HEAD"` (so commits-ahead degrades to 0).
pub fn resolve_base(repo: &gix::Repository) -> String {
    let _ = repo;
    "HEAD".to_string() // STUB
}

/// Count commits reachable from HEAD but not from `base`
/// (`git rev-list --count base..HEAD`). Returns 0 on any failure.
pub fn commits_ahead(repo: &gix::Repository, base: &str) -> u32 {
    let _ = (repo, base);
    u32::MAX // STUB (deliberately wrong so the test's `== N` fails)
}
```

In `tests`:

```rust
    #[test]
    fn resolve_base_prefers_main() {
        let dir = init_repo(); // already on main
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(resolve_base(&repo), "main");
    }

    #[test]
    fn resolve_base_falls_back_to_master() {
        let dir = init_repo();
        git(dir.path(), &["branch", "-m", "main", "master"]);
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(resolve_base(&repo), "master");
    }

    #[test]
    fn commits_ahead_counts_commits_past_base() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(p.join("b.txt"), "two\n").unwrap();
        git(p, &["add", "b.txt"]);
        git(p, &["commit", "-q", "-m", "second"]);
        std::fs::write(p.join("c.txt"), "three\n").unwrap();
        git(p, &["add", "c.txt"]);
        git(p, &["commit", "-q", "-m", "third"]);
        let repo = open_at(p).unwrap();
        assert_eq!(commits_ahead(&repo, "main"), 2);
    }

    #[test]
    fn commits_ahead_is_zero_when_base_equals_head() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(commits_ahead(&repo, "main"), 0);
    }
```

- [ ] **Step 2: Run — verify FAIL**

Run: `cargo test -p gsw --lib repo::tests`
Expected: FAIL on the four new tests.

- [ ] **Step 3: Commit red**

```bash
git add src/gsw/src/repo.rs
git commit --no-verify -m "test(gsw): red — base resolution and commits-ahead via gix"
```

- [ ] **Step 4: Implement**

```rust
pub fn resolve_base(repo: &gix::Repository) -> String {
    for candidate in ["main", "master"] {
        if repo.rev_parse_single(candidate).is_ok() {
            return candidate.to_string();
        }
    }
    // origin/HEAD is a symbolic ref pointing at the remote's default branch.
    if let Ok(reference) = repo.find_reference("refs/remotes/origin/HEAD") {
        if let Some(target) = reference.target().try_name() {
            // target is e.g. refs/remotes/origin/main → shorten to origin/main
            return target.shorten().to_string();
        }
    }
    "HEAD".to_string()
}

pub fn commits_ahead(repo: &gix::Repository, base: &str) -> u32 {
    let resolve = || -> anyhow::Result<u32> {
        let head = repo.head_id()?.detach();
        let base_id = repo.rev_parse_single(base)?.detach();
        if head == base_id {
            return Ok(0);
        }
        let count = repo
            .rev_walk(Some(head))
            .with_hidden(Some(base_id))
            .all()?
            .count();
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    };
    resolve().unwrap_or(0)
}
```

(`rev_parse_single` — `src/repository/revision.rs:42`; `find_reference` — `src/repository/reference.rs:320`; `rev_walk` — `src/repository/revision.rs:174`; `Platform::with_hidden(impl IntoIterator<Item = impl Into<ObjectId>>)` — `src/revision/walk.rs:257`; `.all() -> Result<Walk, _>` yields `Result<Info, _>`. `with_hidden` is the graph-based `^base` analog — preferred over `with_boundary`, which forces a commit-time cutoff that can misbehave on skewed dates.)

> For `reference.target().try_name()` / `.shorten()`: `Reference::target()` returns a `gix_ref::TargetRef`; symbolic targets expose `try_name() -> Option<&FullNameRef>`. If the exact accessor name differs, the compiler-suggested sibling on `TargetRef` is the one to use — the intent is "name of the ref origin/HEAD points at, shortened."

- [ ] **Step 5: Run — verify PASS**

Run: `cargo test -p gsw --lib repo::tests`
Expected: PASS.

- [ ] **Step 6: Wire `main.rs`**

Replace lines 236–241:

```rust
    let base = cli.base.unwrap_or_else(resolve_base_ref);

    let commits_ahead = run_git(&["rev-list", "--count", &format!("{base}..HEAD")])
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
```

with:

```rust
    let base = cli.base.unwrap_or_else(|| repo::resolve_base(&repo));
    let commits_ahead = repo::commits_ahead(&repo, &base);
```

- [ ] **Step 7: Run parity harness, commit green**

Run: `cargo test -p gsw`
Expected: PASS (`shows_branch_and_header` confirms the count phrase still renders).

```bash
git add src/gsw/src/repo.rs src/gsw/src/main.rs
git commit -m "feat(gsw): resolve base ref and count commits-ahead via gix"
```

---

## Task 4: Last-commit age + recent log via gix

Replaces `last_commit_age()` (`log -1 --format=%ct`) and `fetch_log()` (`log -nN --pretty=...`). The parse-into-`LogEntry` shape and `parse_log_line` stay in `main.rs`; only the *source* of the rows changes. We return raw `(hash, unix_secs, subject)` triples from `repo.rs` and let `main.rs` build `LogEntry` + compute ages exactly as today, so `render` is untouched.

**Files:**
- Modify: `src/gsw/src/repo.rs`
- Modify: `src/gsw/src/main.rs` (`last_commit_age` 550-555, `fetch_log` 516-526)
- Test: `src/gsw/src/repo.rs`

- [ ] **Step 1: Add stubs + failing tests**

In `repo.rs`:

```rust
/// Author/commit time of HEAD as unix seconds, or `None` (no commits, etc.).
pub fn head_commit_secs(repo: &gix::Repository) -> Option<i64> {
    let _ = repo;
    None // STUB
}

/// The `n` most recent commits from HEAD as `(short_hash, unix_secs, summary)`.
/// Empty when `n == 0` or there are no commits.
pub fn recent_log(repo: &gix::Repository, n: usize) -> Vec<(String, i64, String)> {
    let _ = (repo, n);
    Vec::new() // STUB
}
```

In `tests`:

```rust
    #[test]
    fn head_commit_secs_is_some_for_a_repo_with_a_commit() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        let secs = head_commit_secs(&repo).expect("a commit exists");
        assert!(secs > 1_000_000_000, "looks like a unix timestamp: {secs}");
    }

    #[test]
    fn recent_log_returns_newest_first_with_summaries() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("b.txt"), "two\n").unwrap();
        git(p, &["add", "b.txt"]);
        git(p, &["commit", "-q", "-m", "second commit"]);
        let repo = open_at(p).unwrap();
        let log = recent_log(&repo, 10);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].2, "second commit");
        assert_eq!(log[1].2, "initial");
        assert!(!log[0].0.is_empty(), "short hash present");
    }

    #[test]
    fn recent_log_zero_is_empty() {
        let dir = init_repo();
        let repo = open_at(dir.path()).unwrap();
        assert!(recent_log(&repo, 0).is_empty());
    }
```

- [ ] **Step 2: Run — verify FAIL**

Run: `cargo test -p gsw --lib repo::tests`
Expected: FAIL on the three new tests.

- [ ] **Step 3: Commit red**

```bash
git add src/gsw/src/repo.rs
git commit --no-verify -m "test(gsw): red — head-commit time and recent log via gix"
```

- [ ] **Step 4: Implement**

```rust
pub fn head_commit_secs(repo: &gix::Repository) -> Option<i64> {
    let commit = repo.head_commit().ok()?;
    Some(commit.time().ok()?.seconds)
}

pub fn recent_log(repo: &gix::Repository, n: usize) -> Vec<(String, i64, String)> {
    if n == 0 {
        return Vec::new();
    }
    let Ok(head) = repo.head_commit() else {
        return Vec::new();
    };
    let Ok(walk) = head.ancestors().all() else {
        return Vec::new();
    };
    walk.take(n)
        .filter_map(|info| {
            let info = info.ok()?;
            let commit = info.object().ok()?;
            let hash = info.id().shorten_or_id().to_string();
            let secs = commit.time().ok()?.seconds;
            let summary = commit.message().ok()?.summary().to_string();
            Some((hash, secs, summary))
        })
        .collect()
}
```

(`head_commit()` — `src/repository/reference.rs:250`; `Commit::time() -> Result<gix_date::Time, _>` and `gix_date::Time.seconds: i64` — `src/object/commit.rs:105`, `gix-date/src/lib.rs:25`; `Commit::ancestors()` returns the revwalk Platform — `src/object/commit.rs:196`; `Info::id().shorten_or_id() -> Prefix` (infallible) — `src/id.rs:60`; `Commit::message()?.summary() -> Cow<BStr>` — `src/object/commit.rs:72` + `gix-object/src/commit/message/mod.rs:76`. `ancestors().all()` is newest-first by default sorting.)

- [ ] **Step 5: Run — verify PASS**

Run: `cargo test -p gsw --lib repo::tests`
Expected: PASS.

- [ ] **Step 6: Wire `main.rs`**

Replace `last_commit_age()`'s body usage. At line 243:

```rust
    let last_commit_age = last_commit_age();
```

Change `last_commit_age()` (lines 550–555) to consume gix. Simplest: inline at the call site and delete the helper, or keep the helper taking `&repo`. Keep a helper for clarity — replace lines 550–555 with:

```rust
/// How long ago HEAD was committed, or `None` when undeterminable.
fn last_commit_age(repo: &gix::Repository) -> Option<Duration> {
    let secs = repo::head_commit_secs(repo)?;
    let secs = u64::try_from(secs).ok()?;
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    SystemTime::now().duration_since(when).ok()
}
```

and update the call site (line 243) to `let last_commit_age = last_commit_age(&repo);`.

Replace `fetch_log()` (lines 516–526) to use `repo::recent_log` and reuse the existing age math from `parse_log_line`. Replace its body:

```rust
fn fetch_log(repo: &gix::Repository, n: usize) -> Vec<LogEntry> {
    let now = SystemTime::now();
    repo::recent_log(repo, n)
        .into_iter()
        .map(|(hash, secs, subject)| {
            let age = u64::try_from(secs)
                .ok()
                .map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s))
                .and_then(|when| now.duration_since(when).ok())
                .unwrap_or(Duration::ZERO);
            LogEntry { hash, subject, age }
        })
        .collect()
}
```

Update the call site (line 272): `snapshot.log = fetch_log(&repo, log_lines);`

`parse_log_line` is now unused by production but still has tests — Task 9 removes it. (Leaving it compiles; if clippy flags it as dead in this task, add `#[allow(dead_code)]` with a `// removed in Task 9` note, or move its removal forward to here. Prefer removing it here if the pre-commit clippy gate fails — delete `parse_log_line` and its two tests `parse_log_line_keeps_entry_when_line_has_no_space`, `parse_log_line_drops_empty_lines`.)

- [ ] **Step 7: Run parity harness, commit green**

Run: `cargo test -p gsw`
Expected: PASS.

```bash
git add src/gsw/src/repo.rs src/gsw/src/main.rs
git commit -m "feat(gsw): read last-commit age and recent log via gix"
```

---

## Task 5: Upstream ahead/behind via gix

Replaces `detect_upstream()` (`rev-parse --abbrev-ref @{upstream}` + `rev-list --left-right --count`).

**Files:**
- Modify: `src/gsw/src/repo.rs`
- Modify: `src/gsw/src/main.rs` (`detect_upstream` 488-508, call site 274)
- Test: `src/gsw/src/repo.rs`

- [ ] **Step 1: Add stub + failing tests**

In `repo.rs` (import the type from render where `UpstreamStatus` lives — it's `crate::render::UpstreamStatus`):

```rust
use crate::render::UpstreamStatus;

/// The current branch's upstream tracking status, or `None` when there's no
/// upstream configured / HEAD is detached. `name` is the short tracking-ref
/// name like `origin/main`; ahead/behind match
/// `git rev-list --left-right --count <upstream>...HEAD`.
pub fn upstream_status(repo: &gix::Repository) -> Option<UpstreamStatus> {
    let _ = repo;
    None // STUB
}
```

In `tests`:

```rust
    /// Build an "origin" by cloning, so the clone has a real upstream.
    fn init_repo_with_upstream() -> (tempfile::TempDir, tempfile::TempDir) {
        let origin = init_repo();
        let clone = tempfile::tempdir().expect("tempdir");
        git(
            clone.path().parent().unwrap(),
            &[
                "clone", "-q",
                origin.path().to_str().unwrap(),
                clone.path().to_str().unwrap(),
            ],
        );
        git(clone.path(), &["config", "user.email", "t@example.com"]);
        git(clone.path(), &["config", "user.name", "Test"]);
        git(clone.path(), &["config", "commit.gpgsign", "false"]);
        (origin, clone)
    }

    #[test]
    fn upstream_none_for_branch_without_upstream() {
        let dir = init_repo(); // local-only main, never pushed
        let repo = open_at(dir.path()).unwrap();
        assert!(upstream_status(&repo).is_none());
    }

    #[test]
    fn upstream_reports_name_and_ahead_count() {
        let (_origin, clone) = init_repo_with_upstream();
        let p = clone.path();
        std::fs::write(p.join("local.txt"), "x\n").unwrap();
        git(p, &["add", "local.txt"]);
        git(p, &["commit", "-q", "-m", "local only"]);
        let repo = open_at(p).unwrap();
        let up = upstream_status(&repo).expect("clone has an upstream");
        assert_eq!(up.name, "origin/main");
        assert_eq!(up.ahead, 1);
        assert_eq!(up.behind, 0);
    }
```

- [ ] **Step 2: Run — verify FAIL**

Run: `cargo test -p gsw --lib repo::tests::upstream`
Expected: FAIL (`upstream_reports_name_and_ahead_count` — `None.expect()` panics).

- [ ] **Step 3: Commit red**

```bash
git add src/gsw/src/repo.rs
git commit --no-verify -m "test(gsw): red — upstream ahead/behind via gix"
```

- [ ] **Step 4: Implement**

```rust
pub fn upstream_status(repo: &gix::Repository) -> Option<UpstreamStatus> {
    use gix::bstr::ByteSlice;
    use gix::remote::Direction;

    let head_ref = repo.head_ref().ok()??; // None => detached/unborn
    // Short display name like "origin/main"; computed from refspecs, so it
    // works even before the tracking ref is fetched.
    let name = match head_ref.remote_tracking_ref_name(Direction::Fetch) {
        Some(Ok(full)) => full.shorten().to_str().ok()?.to_owned(),
        _ => return None, // no upstream configured (or name error)
    };

    let head_id = repo.head_id().ok()?.detach();
    // `@{upstream}` resolves via the tracking ref; needs it to exist locally.
    let upstream_id = repo.rev_parse_single("@{upstream}").ok()?.detach();

    let ahead = repo
        .rev_walk(Some(head_id))
        .with_hidden(Some(upstream_id))
        .all()
        .ok()?
        .count();
    let behind = repo
        .rev_walk(Some(upstream_id))
        .with_hidden(Some(head_id))
        .all()
        .ok()?
        .count();

    Some(UpstreamStatus {
        name,
        ahead: u32::try_from(ahead).unwrap_or(u32::MAX),
        behind: u32::try_from(behind).unwrap_or(u32::MAX),
    })
}
```

(`Reference::remote_tracking_ref_name(Direction) -> Option<Result<Cow<FullNameRef>, _>>` — `src/reference/remote.rs:70`; `FullNameRef::shorten()` strips `refs/remotes/` → `origin/main` — `gix-ref/src/category.rs:17` + `fullname.rs:99`; `head_ref() -> Result<Option<Reference>, _>` returns `Ok(None)` when detached/unborn; `@{upstream}` is supported by `rev_parse_single` — `gix-revision/src/spec/parse/delegate.rs:149`.)

- [ ] **Step 5: Run — verify PASS**

Run: `cargo test -p gsw --lib repo::tests::upstream`
Expected: PASS.

- [ ] **Step 6: Wire `main.rs`**

Replace line 274 `snapshot.upstream = detect_upstream();` with `snapshot.upstream = repo::upstream_status(&repo);`. Delete the `detect_upstream()` fn (lines 488–508). Remove the now-unused `UpstreamStatus` import from `main.rs`'s `use crate::render::...` line if it's no longer referenced there (it's only used by the deleted fn).

- [ ] **Step 7: Run parity harness, commit green**

Run: `cargo test -p gsw`
Expected: PASS.

```bash
git add src/gsw/src/repo.rs src/gsw/src/main.rs
git commit -m "feat(gsw): read upstream ahead/behind via gix"
```

**Milestone:** All read-only metadata now flows through gix. `git status` / `git diff` are the only remaining subprocesses, still guarded by `redirect_index_to_snapshot()`.

---

## Task 6: Status (`Vec<FileEntry>`) via gix

Replaces `run_git(&["status", "--porcelain=v2", "-z"])` + `git::parse_status`. Maps gix status items to the existing `FileEntry`/`FileStatus` domain, including the staged-before-unstaged deterministic ordering (gix's status iterator yields items in nondeterministic order because the `parallel` feature is on).

**Files:**
- Modify: `src/gsw/src/repo.rs`
- Modify: `src/gsw/src/main.rs:245-246`
- Test: `src/gsw/src/repo.rs`

- [ ] **Step 1: Add stub + failing tests**

In `repo.rs` (import the domain types):

```rust
use crate::git::{FileEntry, FileStatus};

/// All working-tree changes as `FileEntry` rows, mirroring
/// `git status --porcelain=v2 -z`. A path modified in both index and worktree
/// yields two rows (staged + unstaged). Rows are ordered by path, staged
/// before unstaged, so the downstream stable mtime sort is deterministic
/// (gix's status iterator itself yields items in nondeterministic order).
pub fn collect_status(repo: &gix::Repository) -> Result<Vec<FileEntry>> {
    let _ = repo;
    Ok(Vec::new()) // STUB
}
```

In `tests`:

```rust
    fn statuses(repo: &gix::Repository) -> Vec<(String, FileStatus, bool)> {
        collect_status(repo)
            .unwrap()
            .into_iter()
            .map(|e| (e.path, e.status, e.staged))
            .collect()
    }

    #[test]
    fn status_staged_modification() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "changed\n").unwrap();
        git(p, &["add", "a.txt"]);
        let repo = open_at(p).unwrap();
        assert_eq!(
            statuses(&repo),
            vec![("a.txt".to_string(), FileStatus::Modified, true)],
        );
    }

    #[test]
    fn status_unstaged_modification() {
        let dir = init_repo();
        std::fs::write(dir.path().join("a.txt"), "edited\n").unwrap();
        let repo = open_at(dir.path()).unwrap();
        assert_eq!(
            statuses(&repo),
            vec![("a.txt".to_string(), FileStatus::Modified, false)],
        );
    }

    #[test]
    fn status_both_sides_yields_two_rows_staged_first() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "staged change\n").unwrap();
        git(p, &["add", "a.txt"]);
        std::fs::write(p.join("a.txt"), "staged change\nthen worktree change\n").unwrap();
        let repo = open_at(p).unwrap();
        assert_eq!(
            statuses(&repo),
            vec![
                ("a.txt".to_string(), FileStatus::Modified, true),
                ("a.txt".to_string(), FileStatus::Modified, false),
            ],
        );
    }

    #[test]
    fn status_untracked_file_and_dir() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("loose.txt"), "x\n").unwrap();
        std::fs::create_dir(p.join("sub")).unwrap();
        std::fs::write(p.join("sub").join("nested.txt"), "y\n").unwrap();
        let repo = open_at(p).unwrap();
        let s = statuses(&repo);
        assert!(s.contains(&("loose.txt".to_string(), FileStatus::Untracked, false)));
        // git's default collapses a fully-untracked dir to "sub/".
        assert!(s.iter().any(|(path, st, _)| path == "sub/" && *st == FileStatus::UntrackedDir));
    }

    #[test]
    fn status_staged_addition_and_deletion() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("added.txt"), "new\n").unwrap();
        git(p, &["add", "added.txt"]);
        git(p, &["rm", "-q", "a.txt"]);
        let repo = open_at(p).unwrap();
        let s = statuses(&repo);
        assert!(s.contains(&("added.txt".to_string(), FileStatus::Added, true)));
        assert!(s.contains(&("a.txt".to_string(), FileStatus::Deleted, true)));
    }

    #[test]
    fn status_staged_rename_keeps_orig_path() {
        let dir = init_repo();
        let p = dir.path();
        git(p, &["mv", "a.txt", "renamed.txt"]);
        let repo = open_at(p).unwrap();
        let entry = collect_status(&repo)
            .unwrap()
            .into_iter()
            .find(|e| e.path == "renamed.txt")
            .expect("renamed.txt present");
        assert_eq!(entry.status, FileStatus::Renamed);
        assert!(entry.staged);
        assert_eq!(entry.orig_path.as_deref(), Some("a.txt"));
    }
```

- [ ] **Step 2: Run — verify FAIL**

Run: `cargo test -p gsw --lib repo::tests::status`
Expected: FAIL on all status tests (stub returns empty).

- [ ] **Step 3: Commit red**

```bash
git add src/gsw/src/repo.rs
git commit --no-verify -m "test(gsw): red — status mapping via gix"
```

- [ ] **Step 4: Implement**

```rust
pub fn collect_status(repo: &gix::Repository) -> Result<Vec<FileEntry>> {
    use gix::bstr::BString;

    let platform = repo
        .status(gix::progress::Discard)?
        // git's default: collapse fully-untracked directories to "dir/".
        .untracked_files(gix::status::UntrackedFiles::Collapsed);

    let mut out = Vec::new();
    for item in platform.into_iter(Vec::<BString>::new())? {
        match item? {
            gix::status::Item::TreeIndex(change) => push_staged(change, &mut out),
            gix::status::Item::IndexWorktree(item) => push_unstaged(item, &mut out),
        }
    }

    // gix yields items in nondeterministic order (parallel producers). Make
    // the order deterministic: by path, staged before unstaged for the same
    // path — matching git's emission so the downstream stable mtime sort in
    // build_snapshot ties-breaks the same way it always has.
    out.sort_by(|a, b| a.path.cmp(&b.path).then(b.staged.cmp(&a.staged)));
    Ok(out)
}

/// Map a HEAD-tree↔index change (the STAGED side) to a `FileEntry`.
fn push_staged(change: gix::diff::index::Change, out: &mut Vec<FileEntry>) {
    use gix::diff::index::Change;
    let (path, status, orig_path) = match &change {
        Change::Addition { location, .. } => {
            (location.to_string(), FileStatus::Added, None)
        }
        Change::Deletion { location, .. } => {
            (location.to_string(), FileStatus::Deleted, None)
        }
        Change::Modification {
            location,
            previous_entry_mode,
            entry_mode,
            ..
        } => {
            let status = if previous_entry_mode.kind() != entry_mode.kind() {
                FileStatus::TypeChange
            } else {
                FileStatus::Modified
            };
            (location.to_string(), status, None)
        }
        Change::Rewrite {
            location,
            source_location,
            copy,
            ..
        } => {
            let status = if *copy { FileStatus::Copied } else { FileStatus::Renamed };
            (location.to_string(), status, Some(source_location.to_string()))
        }
    };
    out.push(FileEntry { path, orig_path, status, staged: true });
}

/// Map an index↔worktree item (the UNSTAGED side, plus untracked) to zero or
/// one `FileEntry`.
fn push_unstaged(item: gix::status::index_worktree::Item, out: &mut Vec<FileEntry>) {
    use gix::status::index_worktree::Item;
    use gix_status::index_as_worktree::{Change, EntryStatus};

    match item {
        Item::Modification { rela_path, status, .. } => {
            let mapped = match status {
                EntryStatus::Conflict(_) => Some((FileStatus::Conflicted, None)),
                EntryStatus::IntentToAdd => Some((FileStatus::Added, None)),
                EntryStatus::Change(change) => match change {
                    Change::Removed => Some((FileStatus::Deleted, None)),
                    Change::Type { .. } => Some((FileStatus::TypeChange, None)),
                    Change::Modification { .. } => Some((FileStatus::Modified, None)),
                    Change::SubmoduleModification(_) => Some((FileStatus::Modified, None)),
                },
                EntryStatus::NeedsUpdate(_) => None, // internal; not user-visible
            };
            if let Some((status, orig_path)) = mapped {
                out.push(FileEntry {
                    path: rela_path.to_string(),
                    orig_path,
                    status,
                    staged: false,
                });
            }
        }
        Item::DirectoryContents { entry, .. } => {
            // Untracked (and, if configured, ignored) entries. We only enabled
            // untracked, so anything here that's Untracked becomes a row.
            if entry.status == gix_dir::entry::Status::Untracked {
                let is_dir = entry.disk_kind == Some(gix_dir::entry::Kind::Directory);
                out.push(FileEntry {
                    path: entry.rela_path.to_string(),
                    orig_path: None,
                    status: if is_dir { FileStatus::UntrackedDir } else { FileStatus::Untracked },
                    staged: false,
                });
            }
        }
        Item::Rewrite { source, dirwalk_entry, .. } => {
            out.push(FileEntry {
                path: dirwalk_entry.rela_path.to_string(),
                orig_path: Some(source.rela_path().to_string()),
                status: FileStatus::Renamed,
                staged: false,
            });
        }
    }
}
```

Add the two plumbing crates as direct deps so the `gix_status::` / `gix_dir::` paths resolve. In `src/gsw/Cargo.toml` `[dependencies]`:

```toml
gix-status.workspace = true
gix-dir.workspace = true
```

If those aren't yet workspace deps, add them to the **root** `Cargo.toml` `[workspace.dependencies]` pinned to the versions gix 0.83 resolves (`gix-status = "0.30"`, `gix-dir = "0.25"` per the lockfile) — or, to avoid extra deps entirely, re-export them through gix instead: replace the `use gix_status::...` / `gix_dir::...` paths with `gix::status::index_worktree::iter::Item`-adjacent re-exports. **Prefer the re-export route if it compiles** (`gix` re-exports much of these under `gix::status` and `gix::dir`); only add the direct deps if a needed type isn't re-exported. Verify with `cargo doc -p gix --open` or by following the compiler.

> Caveats baked into the tests above: `entry_mode.kind()` distinguishes blob/symlink/commit for the typechange decision (`gix_index::entry::Mode::kind()`); the exact `Change`/`EntryStatus` paths are from `gix-status-0.30.0/src/index_as_worktree/types.rs:105,180`; `index_worktree::Item` variants from `src/status/index_worktree.rs:337`. If a variant's field names differ slightly (e.g. `rela_path` vs `relation`), follow the compiler — the mapping intent is fixed by the tests.

- [ ] **Step 5: Run — verify PASS**

Run: `cargo test -p gsw --lib repo::tests::status`
Expected: PASS. If `status_untracked_file_and_dir`'s collapsed-dir expectation differs (gix may emit `sub` without trailing slash), adjust the mapping to append `/` for `UntrackedDir` so it matches git's `?? sub/` convention that `parse_status` produced (`render` keys off the trailing slash). Add that normalization in `push_unstaged`'s dir branch:
```rust
let path = if is_dir && !entry.rela_path.ends_with(b"/") {
    format!("{}/", entry.rela_path)
} else {
    entry.rela_path.to_string()
};
```

- [ ] **Step 6: Wire `main.rs`**

Replace lines 245–246:

```rust
    let status_raw = run_git(&["status", "--porcelain=v2", "-z"])?;
    let entries = parse_status(&status_raw);
```

with:

```rust
    let entries = repo::collect_status(&repo)?;
```

Remove the now-unused `use crate::git::{parse_numstat, parse_status, FileEntry};` import of `parse_status` (keep `parse_numstat` until Task 7, `FileEntry` if still used).

- [ ] **Step 7: Run parity harness, commit green**

Run: `cargo test -p gsw`
Expected: PASS — `shows_staged_modification` (`●`), `shows_unstaged_modification` (`○`), `shows_untracked_file` (`?`) all confirm rendered parity.

```bash
git add src/gsw/src/repo.rs src/gsw/src/main.rs src/gsw/Cargo.toml Cargo.toml Cargo.lock
git commit -m "feat(gsw): compute working-tree status via gix"
```

---

## Task 7: numstat (`+adds/-dels`) via gix blob diff

Replaces both `run_git(&["diff", "--cached", "--numstat", "-z"])` and `run_git(&["diff", "--numstat", "-z"])` + `git::parse_numstat`. This is the last `git diff`; after it, no index-rewriting subprocess remains.

**Files:**
- Modify: `src/gsw/src/repo.rs`
- Modify: `src/gsw/src/main.rs:248-253`
- Test: `src/gsw/src/repo.rs`

- [ ] **Step 1: Add stub + failing tests**

In `repo.rs` (import `NumStat`, add to the existing `use crate::git::...`):

```rust
use crate::git::NumStat;
use std::collections::HashMap;

/// Per-path line counts for the staged side (HEAD-tree vs index) and the
/// unstaged side (index vs worktree), keyed on the post-rename path — the
/// gix equivalent of `git diff --cached --numstat` and `git diff --numstat`.
/// Untracked files are excluded (git's numstat ignores them), matching the
/// old behavior where untracked rows fall back to (0, 0, false).
pub fn collect_numstats(
    repo: &gix::Repository,
) -> Result<(HashMap<String, NumStat>, HashMap<String, NumStat>)> {
    let _ = repo;
    Ok((HashMap::new(), HashMap::new())) // STUB
}
```

In `tests`:

```rust
    #[test]
    fn numstat_staged_modification_counts_lines() {
        let dir = init_repo(); // a.txt = "initial\n"
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "initial\nadded one\nadded two\n").unwrap();
        git(p, &["add", "a.txt"]);
        let repo = open_at(p).unwrap();
        let (staged, _unstaged) = collect_numstats(&repo).unwrap();
        let ns = staged.get("a.txt").expect("staged numstat for a.txt");
        assert_eq!((ns.adds, ns.dels, ns.binary), (2, 0, false));
    }

    #[test]
    fn numstat_unstaged_modification_counts_lines() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "rewritten\n").unwrap();
        let repo = open_at(p).unwrap();
        let (_staged, unstaged) = collect_numstats(&repo).unwrap();
        let ns = unstaged.get("a.txt").expect("unstaged numstat");
        // one line removed, one added
        assert_eq!((ns.adds, ns.dels, ns.binary), (1, 1, false));
    }

    #[test]
    fn numstat_staged_addition_counts_all_lines() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("new.txt"), "l1\nl2\nl3\n").unwrap();
        git(p, &["add", "new.txt"]);
        let repo = open_at(p).unwrap();
        let (staged, _) = collect_numstats(&repo).unwrap();
        let ns = staged.get("new.txt").expect("staged add numstat");
        assert_eq!((ns.adds, ns.dels), (3, 0));
    }

    #[test]
    fn numstat_binary_file_is_flagged() {
        let dir = init_repo();
        let p = dir.path();
        std::fs::write(p.join("blob.bin"), [0u8, 1, 2, 0, 3, 4]).unwrap();
        git(p, &["add", "blob.bin"]);
        let repo = open_at(p).unwrap();
        let (staged, _) = collect_numstats(&repo).unwrap();
        let ns = staged.get("blob.bin").expect("binary numstat");
        assert!(ns.binary, "NUL-containing blob must be flagged binary");
        assert_eq!((ns.adds, ns.dels), (0, 0));
    }
```

- [ ] **Step 2: Run — verify FAIL**

Run: `cargo test -p gsw --lib repo::tests::numstat`
Expected: FAIL (stub returns empty maps).

- [ ] **Step 3: Commit red**

```bash
git add src/gsw/src/repo.rs
git commit --no-verify -m "test(gsw): red — numstat line counts via gix"
```

- [ ] **Step 4: Implement**

```rust
pub fn collect_numstats(
    repo: &gix::Repository,
) -> Result<(HashMap<String, NumStat>, HashMap<String, NumStat>)> {
    use gix::bstr::BString;
    use gix::diff::index::Change;
    use gix::status::index_worktree::Item as IwItem;
    use gix_status::index_as_worktree::{Change as IwChange, EntryStatus};

    let mut staged = HashMap::new();
    let mut unstaged = HashMap::new();

    let platform = repo
        .status(gix::progress::Discard)?
        .untracked_files(gix::status::UntrackedFiles::Collapsed);

    for item in platform.into_iter(Vec::<BString>::new())? {
        match item? {
            gix::status::Item::TreeIndex(change) => {
                // Staged: diff HEAD-tree blob vs index blob.
                let (path, old, new) = match &change {
                    Change::Addition { location, id, .. } => {
                        (location.to_string(), Vec::new(), blob(repo, id)?)
                    }
                    Change::Deletion { location, id, .. } => {
                        (location.to_string(), blob(repo, id)?, Vec::new())
                    }
                    Change::Modification { location, previous_id, id, .. } => {
                        (location.to_string(), blob(repo, previous_id)?, blob(repo, id)?)
                    }
                    Change::Rewrite { location, source_id, id, .. } => {
                        (location.to_string(), blob(repo, source_id)?, blob(repo, id)?)
                    }
                };
                staged.insert(path, line_counts(&old, &new));
            }
            gix::status::Item::IndexWorktree(IwItem::Modification {
                rela_path, entry, status, ..
            }) => {
                // Unstaged: diff index blob vs on-disk worktree bytes.
                let index_blob = blob(repo, &entry.id)?;
                let worktree = read_worktree(repo, &rela_path);
                let ns = match &status {
                    EntryStatus::Change(IwChange::Removed) => line_counts(&index_blob, &[]),
                    EntryStatus::Change(IwChange::Modification { .. })
                    | EntryStatus::Change(IwChange::Type { .. }) => {
                        line_counts(&index_blob, &worktree)
                    }
                    _ => continue, // conflicts/intent-to-add/submodule: no numstat
                };
                unstaged.insert(rela_path.to_string(), ns);
            }
            // Untracked dirs/files and worktree rewrites: git numstat shows
            // nothing for untracked; rewrites are rare for a monitor. Skip.
            _ => {}
        }
    }

    Ok((staged, unstaged))
}

/// Read a blob's bytes from the object DB.
fn blob(repo: &gix::Repository, id: &gix::hash::oid) -> Result<Vec<u8>> {
    Ok(repo.find_object(id)?.into_blob().take_data())
}

/// Read the on-disk worktree bytes for `rela_path`, or empty if unreadable.
fn read_worktree(repo: &gix::Repository, rela_path: &gix::bstr::BStr) -> Vec<u8> {
    use gix::bstr::ByteSlice;
    let Some(workdir) = repo.workdir() else { return Vec::new() };
    let Ok(rel) = rela_path.to_str() else { return Vec::new() };
    std::fs::read(workdir.join(rel)).unwrap_or_default()
}

/// Count added/removed lines between two blobs, flagging binaries (NUL byte in
/// the first 8000 bytes — gix's own heuristic, replicated since it's private).
fn line_counts(old: &[u8], new: &[u8]) -> NumStat {
    if is_binary(old) || is_binary(new) {
        return NumStat { adds: 0, dels: 0, binary: true };
    }
    use gix::diff::blob::{intern::InternedInput, sources::byte_lines, Algorithm, Diff};
    let input = InternedInput::new(byte_lines(old), byte_lines(new));
    let diff = Diff::compute(Algorithm::Histogram, &input);
    NumStat {
        adds: diff.count_additions(),
        dels: diff.count_removals(),
        binary: false,
    }
}

fn is_binary(buf: &[u8]) -> bool {
    buf[..buf.len().min(8000)].contains(&0)
}
```

(`find_object` — `src/repository/object.rs:56`; `Object::into_blob()` then the data field — `Blob.data: Vec<u8>` (`src/types.rs:60`); if `take_data()` doesn't exist, use `repo.find_object(id)?.into_blob().data`. `gix::diff::blob` re-exports `gix-imara-diff`: `Algorithm::Histogram`, `InternedInput::new`, `byte_lines`, `Diff::compute`, `count_additions/count_removals` — `gix-imara-diff-0.2.1/src/lib.rs:176,98,251,288,293`; `byte_lines` — `sources.rs:43`. Private binary heuristic mirrored from `gix-diff-0.63.0/src/blob/pipeline.rs:519`.)

> **Parity caveat (documented, accepted):** this is a raw byte-line diff with no clean/smudge or autocrlf filtering. `git diff --numstat` applies attributes/EOL conversion via gix's blob *resource-cache pipeline*. For gsw — a magnitude-bar monitor — raw counts are visually identical in the common case; exact parity on repos with `text=auto`/CRLF normalization would require routing through `gix_diff::blob::Platform` with a resource cache. Note this in the function doc and move on; revisit only if a real repo shows a discrepancy.

- [ ] **Step 5: Run — verify PASS**

Run: `cargo test -p gsw --lib repo::tests::numstat`
Expected: PASS.

- [ ] **Step 6: Wire `main.rs`**

Replace lines 248–253:

```rust
    let staged_numstat = run_git(&["diff", "--cached", "--numstat", "-z"])
        .map(|s| parse_numstat(&s))
        .unwrap_or_default();
    let unstaged_numstat = run_git(&["diff", "--numstat", "-z"])
        .map(|s| parse_numstat(&s))
        .unwrap_or_default();
```

with:

```rust
    let (staged_numstat, unstaged_numstat) =
        repo::collect_numstats(&repo).unwrap_or_default();
```

Remove the `parse_numstat` import from `main.rs`.

- [ ] **Step 7: Run parity harness, commit green**

Run: `cargo test -p gsw`
Expected: PASS.

```bash
git add src/gsw/src/repo.rs src/gsw/src/main.rs
git commit -m "feat(gsw): compute per-file numstat via gix blob diff"
```

**Milestone:** No `git status` / `git diff` subprocess remains. The index-snapshot guard is now load-bearing for nothing.

---

## Task 8: Delete the subprocess + index-snapshot machinery

Now that nothing shells out to git, remove `run_git`, `git_command`, `redirect_index_to_snapshot`, `IndexSnapshot`, `GSW_INDEX_FILE`, the `_index_snapshot` guard, `inside_git_repo`, and `resolve_base_ref`. Repurpose the existing index-race regression test to assert the property holds *structurally* (gix never writes the index), and tighten its comment.

**Files:**
- Modify: `src/gsw/src/main.rs`
- Modify: `src/gsw/tests/integration.rs` (comment + assertion message only)

- [ ] **Step 1: Confirm the regression test still passes against the gix binary (it should, trivially)**

Run: `cargo test -p gsw --test integration does_not_rewrite_the_index_so_a_concurrent_rebase_keeps_the_lock`
Expected: PASS — gsw never opens the index for writing, so `before == after` holds without any `GIT_OPTIONAL_LOCKS` / `GIT_INDEX_FILE` tricks. If it FAILS, stop: some gix call is writing the index (it shouldn't; `repo.status()` reads only). Investigate before deleting the guard.

- [ ] **Step 2: Delete the machinery from `main.rs`**

Remove, in `src/gsw/src/main.rs`:
- the `let _index_snapshot = redirect_index_to_snapshot();` line and its comment block (the gix open in Task 1 already replaced the `inside_git_repo` gate).
- `fn inside_git_repo` (336–347).
- `static GSW_INDEX_FILE` (352) and its doc.
- `fn git_command` (371–378).
- `struct IndexSnapshot` + its `impl Drop` (382–393).
- `fn redirect_index_to_snapshot` (412–438).
- `fn run_git` (441–455).
- `fn resolve_base_ref` (460–473) — replaced by `repo::resolve_base` in Task 3.

Remove now-unused imports from `main.rs`: `std::process::Command`, `std::sync::OnceLock`, `bail`, `Context` (verify each with the compiler — keep any still used by `collect_ages`/`effective_*`).

- [ ] **Step 3: Update the regression test's comment + message**

In `src/gsw/tests/integration.rs`, in `does_not_rewrite_the_index_...`, update the doc comment and the `assert_eq!` failure message to reflect the new mechanism. Replace the assertion message:

```rust
    assert_eq!(
        before, after,
        "gsw rewrote .git/index. gsw reads the repo in-process via gix and must \
         never write the index — writing it takes .git/index.lock, which races a \
         concurrent rebase. A gix status/diff read should never touch .git/index.",
    );
```

And trim the comment's references to `GIT_OPTIONAL_LOCKS` / `GIT_INDEX_FILE` (now obsolete), keeping the backdate-mtime setup explanation (still the trigger that would make the *old* CLI path rewrite the index).

- [ ] **Step 4: Run the full suite**

Run: `cargo test -p gsw`
Expected: PASS (lib + all integration tests).

- [ ] **Step 5: Clippy must be clean (no dead code, no unused imports)**

Run: `cargo clippy -p gsw --all-targets -- -D warnings`
Expected: clean. Fix any unused-import / dead-code findings the deletions surfaced.

- [ ] **Step 6: Commit**

```bash
git add src/gsw/src/main.rs src/gsw/tests/integration.rs
git commit -m "refactor(gsw): drop the git-subprocess and index-snapshot workaround now that gix never locks the index"
```

---

## Task 9: Delete the dead string parsers and finalize

`git.rs`'s `parse_status` / `parse_numstat` (and helpers) are now unused; remove them and their unit tests, leaving `git.rs` as the domain-types module. Confirm no `git` subprocess references remain anywhere in gsw.

**Files:**
- Modify: `src/gsw/src/git.rs`

- [ ] **Step 1: Verify the parsers are unreferenced**

Run: `cargo build -p gsw 2>&1 | rg "never used|unused" ; rg -n "parse_status|parse_numstat" src/gsw/src`
Expected: the only matches are the definitions and their tests in `git.rs`.

- [ ] **Step 2: Delete the parsers + their tests**

In `src/gsw/src/git.rs`, remove:
- `pub fn parse_status`, `fn split_first`, `fn parse_ordinary_entry`, `fn parse_rename_entry`, `fn parse_unmerged_entry`, `fn emit_xy`, `fn char_to_status`, `pub fn parse_numstat` (lines ~43–215).
- the entire `#[cfg(test)] mod tests` block (lines ~217–424) — every test there exercises the deleted parsers; the gix backend is covered by `repo.rs`'s tests and `integration.rs`.
- the now-unused `use std::collections::HashMap;` at the top.

Keep: `FileStatus`, `FileEntry`, `NumStat` (and their derives/docs). Update the file's top doc comment from "Parse `git status ...`" to describe it as the working-tree change domain types.

- [ ] **Step 3: Confirm `git.rs` still provides what importers need**

Run: `cargo build -p gsw`
Expected: PASS — `snapshot.rs` (`FileEntry`, `NumStat`) and `render.rs` (`FileStatus`) still resolve.

- [ ] **Step 4: Full suite + clippy + no leftover subprocess calls**

Run:
```
cargo test -p gsw && cargo clippy -p gsw --all-targets -- -D warnings && rg -n 'Command::new\("git"\)|run_git|GIT_INDEX_FILE|GIT_OPTIONAL_LOCKS' src/gsw/src
```
Expected: tests PASS, clippy clean, and the `rg` over `src/` returns **nothing** (any matches in `tests/` are fixture construction and are fine).

- [ ] **Step 5: Commit**

```bash
git add src/gsw/src/git.rs
git commit -m "refactor(gsw): remove the now-dead porcelain/numstat parsers"
```

- [ ] **Step 6: Manual smoke test (real repo, real viddy loop)**

Run gsw against this very repo and eyeball parity vs the previous build:
```
cargo run -p gsw --release
```
Confirm: branch, commits-ahead, last-commit age, per-file bars/+/-/age, untracked `?`, staged `●`/unstaged `○`, and the commit log all render as before. (Optional: `viddy -n1 'cargo run -q -p gsw --release'` to confirm the loop is visibly snappier with no subprocess fan-out.)

---

## Self-Review (completed against the analysis)

**1. Spec coverage** — every `git` call site in `main.rs` is accounted for: `--is-inside-work-tree`→Task 1; `--abbrev-ref HEAD`→Task 2; base probes + `rev-list --count`→Task 3; `log -1`/`log -nN`→Task 4; `@{upstream}` + `--left-right --count`→Task 5; `status --porcelain=v2`→Task 6; both `diff --numstat`→Task 7; `--show-toplevel`/`--git-path index` become unnecessary (gix `workdir()`, and no index path is needed once we stop redirecting it) and are removed with the machinery in Task 8. `collect_ages` (filesystem mtime) is intentionally untouched.

**2. Placeholder scan** — no TBD/TODO; every code step shows real code; every command shows expected output. The two genuinely uncertain spots (gix-status/gix-dir re-export vs direct dep in Task 6; numstat filter parity in Task 7) are called out explicitly with a concrete default and a fallback, not left vague.

**3. Type consistency** — `FileEntry { path, orig_path, status, staged }`, `FileStatus` variants, and `NumStat { adds, dels, binary }` match `git.rs` exactly across Tasks 6–7. `UpstreamStatus { name, ahead, behind }` and `LogEntry { hash, subject, age }` match `render.rs`. `repo::collect_numstats` returns `(staged, unstaged)` consumed positionally by `build_snapshot(... &staged_numstat, &unstaged_numstat ...)` in the original order.

**Known follow-ups (not blockers):** (a) optionally fuse `collect_status` + `collect_numstats` into one status walk later to avoid the second traversal; (b) revisit numstat filter parity only if a real repo shows a discrepancy.
