# Plan: portplz repo-root hashing

> Source PRD: docs/superpowers/specs/2026-04-13-portplz-repo-root-hashing-design.md (Issue #197)

## Architectural decisions

Durable decisions that apply across all phases:

- **Hash input format**: `repo-root-basename@branch` (separator: `@`)
- **Repo root discovery**: `gix` `common_dir()` -> `parent()` -> `file_name()`
- **CLI surface**: remove `--with-dir`; keep `--no-git`, `--verbose`, positional `path`
- **Port generation**: unchanged (SHA-256 -> unprivileged port range 1024-65534)
- **Fallback**: CWD basename when git unavailable or `--no-git`

---

## Phase 1: Repo root hashing

**User stories**: As a developer running multiple projects on `main`, I get unique ports per project without any flags.

### What to build

Add a `get_repo_root_name` function that extracts the repo root basename from `gix`'s `common_dir()`. Change the default hash input from `branch` to `repo-root-basename@branch`. When the repo root can't be determined but a branch is available, fall back to CWD basename as the repo identifier.

### Acceptance criteria

- [x] Same branch + same repo root -> same port
- [x] Same branch + different repo root -> different port
- [x] `get_repo_root_name` returns consistent name across worktrees (uses common_dir)
- [x] Fallback to CWD basename when repo root discovery fails

---

## Phase 2: Remove --with-dir flag

**User stories**: As a user, the CLI is simpler because repo identity is always included automatically.

### What to build

Remove the `--with-dir` CLI flag, its `conflicts_with` constraint, and all conditional logic that checked it. Update verbose output to show the repo root name: `Port {port} for repo '{root}' on branch '{branch}'`. Replace old `--with-dir` tests with tests for the new verbose format.

### Acceptance criteria

- [x] `--with-dir` flag no longer exists in CLI struct
- [x] No conditional branching on `with_dir` in main logic
- [x] Verbose output shows repo name and branch
- [x] Old `--with-dir` tests removed, new verbose format tests added

---

## Phase 3: Edge case handling

**User stories**: As a developer in unusual git states, I still get a reasonable port without errors.

### What to build

Handle detached HEAD by using repo root basename without a branch suffix. Verify that `--no-git` and non-git directories use CWD basename. Add tests for each edge case.

### Acceptance criteria

- [x] Detached HEAD uses repo-root-basename (no branch suffix) as hash input
- [x] `--no-git` flag uses CWD basename
- [x] Non-git directory uses CWD basename
- [x] All edge cases have tests
