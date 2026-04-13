# portplz: Hash repo root + branch name to avoid port collisions

**Issue:** #197
**Date:** 2026-04-13
**Status:** Approved

## Problem

When multiple projects share the same branch name (typically `main`), `portplz` generates identical ports because it hashes only the branch name. This causes port collisions across unrelated projects.

Current behavior:
- `/code/project-a` on `main` -> hashes `"main"` -> port X
- `/code/project-b` on `main` -> hashes `"main"` -> port X (collision)

Worktree behavior is already correct -- worktrees of the same repo on the same branch produce the same port because they share the branch name.

## Solution

Always include the git repo root basename in the hash input: `repo-root-basename@branch`.

### Hash input examples

| Path | Branch | Hash input | Notes |
|------|--------|------------|-------|
| `/code/project-a` | `main` | `project-a@main` | Main repo |
| `/code/project-b` | `main` | `project-b@main` | Different port |
| `/code/project-a-worktrees/issue-42` | `main` | `project-a@main` | Same as main repo |
| `/code/project-a` | `feature-x` | `project-a@feature-x` | Different branch |

### Repo root discovery

New function `get_repo_root_name(repo) -> Option<String>`:
1. `repo.common_dir()` returns the shared `.git` path (consistent across worktrees)
2. `.parent()` gives the repo root directory
3. `.file_name()` extracts the basename
4. Returns `None` on failure (triggers CWD basename fallback)

### CLI changes

- **Remove** the `--with-dir` flag entirely. It is now redundant since the repo root is always included.
- Remove the `conflicts_with = "no_git"` constraint that was on `--with-dir`.
- No new flags.

### Hash input logic by scenario

| Scenario | Hash input |
|----------|------------|
| Git repo found, branch available | `repo-root-basename@branch` |
| Git repo found, detached HEAD (no branch) | repo-root-basename (no branch suffix) |
| `--no-git` flag | CWD basename |
| No git repo found | CWD basename |

### Verbose output

Update format to: `Port {port} for repo '{repo_root}' on branch '{branch}'`

Fallback format (no git): `Port {port} for directory '{dirname}' (no git repo)`

### Breaking changes

- All git-aware port assignments change because the hash input changes from `branch` to `root@branch`.
- `--with-dir` flag is removed. Scripts using it will error.
- Acceptable for a personal tools repo with no external consumers.

## Test plan

1. Same branch + same repo root -> same port
2. Same branch + different repo root -> different port
3. Worktrees of the same repo on the same branch -> same port (via common_dir)
4. Fallback to CWD basename when git repo unavailable
5. `--no-git` uses CWD basename
6. Detached HEAD falls back to CWD basename
