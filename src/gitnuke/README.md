# gitnuke (nuke a worktree)

- Removes a git worktree and deletes the branch it had checked out, resolved as one
  operation so the branch is never deleted while the worktree is left standing. Takes a
  path, a directory name, or a branch name (exact matches only). A worktree with
  submodules checked out - which plain `git worktree remove` refuses outright - or with
  uncommitted changes needs `--force`. `--dry-run` reports the plan as a preflight, and
  `--safe` keeps the branch unless it is fully merged. Before it deletes a branch that no
  message of the last 10 commits of the main branch or its upstream names, it asks. Thus a
  mistyped issue number does not delete unmerged work. `--yes` skips the question. Without
  a terminal, gitnuke cannot ask, and it refuses with exit code `10`.
- To install: `cargo install --git https://github.com/timmattison/tools gitnuke`

Removes a git worktree and deletes the branch it had checked out. The two halves are one
operation: the target is resolved against `git worktree list`, so the branch gitnuke deletes
is whatever that worktree actually had checked out, and it is only deleted once the removal
has succeeded. A refused removal never leaves the branch destroyed and the worktree standing.

## Basic Usage

```bash
gitnuke ../issue-42-wt        # by path
gitnuke issue-42-wt           # by directory name
gitnuke issue-42              # by branch name
gitnuke --force issue-42      # worktree has submodules or uncommitted changes
gitnuke --dry-run issue-42    # report the plan, change nothing
gitnuke --safe issue-42       # keep the branch unless it is fully merged
gitnuke --yes issue-42        # do not ask about a branch that no recent commit names
gitnuke wt-a wt-b wt-c        # nuke several; each is reported independently
```

## A branch that no recent commit names

The normal flow is: the PR merges, then you nuke its branch. gitnuke counts on a merge
convention that this repository obeys: each squash commit names its branch on a
`Branch: <branch>` line. Thus the recent commits of the main branch name each branch that is
safe to nuke, and a mistyped issue number gives a name that they do not name.

GitHub does not write that line. Its default squash message holds no branch name, and its
default merge commit names `owner/branch`, which is not a whole-token match for `branch`. In
a repository that does not obey the convention, no recent commit names a branch, so gitnuke
asks before each nuke. Use `--yes` there.

Before gitnuke removes a worktree that has a branch, it looks for the branch name in the
messages of the last 10 commits of the main worktree's branch. It also reads the last 10
commits of the upstream of that branch (`origin/main`), because `gh pr merge` in a worktree
does not move the local `main`. If no message names the branch, gitnuke asks:

```text
gitnuke: no commit in the last 10 of main or origin/main names branch 'issue-42'.
  If you typed the wrong name, answer n. If its PR merged moments ago, run git fetch and try again.
  Remove worktree /code/repo-worktrees/issue-42 and delete branch issue-42? [y/N]
```

- Only `y` or `yes`, in any case, continues. Each other answer, an empty line, and the end
  of the input keep the worktree and the branch, with exit code `10`.
- The name must be a whole token. The character before it and the character after it must
  not be a letter, a digit, `.`, `_`, `/` or `-`. Thus `issue-70` does not match
  `Branch: issue-704`.
- gitnuke does not fetch, and it uses no network. If the PR merged moments ago, run
  `git fetch` first.
- When stdin is not a terminal, gitnuke cannot ask. It refuses with exit code `10`, and it
  does not continue silently. An agent that runs gitnuke through a tool can type the wrong
  number too.
- `--yes` skips the question. `--force` does not skip it: `--force` overrides git's refusals
  about the content of a worktree, but this question is about which worktree you named.
- A detached-HEAD worktree has no branch, so gitnuke asks nothing about it.
- A target that an earlier gate refuses (locked, submodules, uncommitted changes) gets that
  refusal, with no question first.
- `--dry-run` asks nothing. It reports that a real run asks, and it exits `10`. With `--yes`,
  the dry run exits `0`.
- Each target gets its own question. A "no" keeps that target only, and the run continues
  with the next target.

## Worktrees with submodules

Plain `git worktree remove` refuses a worktree that has submodules checked out:

```text
fatal: working trees containing submodules cannot be moved or removed
```

gitnuke refuses too, but tells you what is in the way and how to override it:

```text
gitnuke: /code/repo-worktrees/issue-42 contains submodules (sub) — git refuses to remove a
worktree with submodules checked out.
  Nuking it deletes those checkouts along with any uncommitted or unpushed work inside them.
  Re-run with --force to nuke it anyway.
```

`--force` is required because that warning is literal: a submodule checkout can hold commits
and untracked files that exist nowhere else, and removing the worktree deletes them silently.
One `--force` covers both of the refusals gitnuke overrides — submodules checked out and
uncommitted changes in the worktree itself.

## Worktrees with uncommitted changes

git's other refusal, and gitnuke raises it the same way it raises the submodule one — on its
own terms, before `git worktree remove` is ever reached:

```text
gitnuke: /code/repo-worktrees/issue-42 contains modified or untracked files — nuking it
discards work that exists nowhere else.
  Re-run with --force to nuke it anyway.
```

Untracked files count, ignored files do not — the same question `git worktree remove` asks.
This is exit code `9`, and `--dry-run` reports it with the identical message: one gate serves
both runs, so they cannot drift. A worktree that has *both* submodules and uncommitted
changes reports the submodule refusal (`5`), because a submodule checkout is the more
consequential thing `--force` would take with it.

## Locked worktrees

git has a third refusal, and `--force` is not the answer to it. A worktree you locked with
`git worktree lock` is declined even by `git worktree remove --force`, which asks for
`remove -f -f` instead. A lock is a deliberate "leave this alone" marker you set by hand, so
gitnuke honours it rather than escalating, and says so before it touches anything:

```text
gitnuke: /code/repo-worktrees/issue-42 is locked (mid-bisect, do not touch) — git refuses to
remove a locked worktree even with --force.
  Unlock it first: git worktree unlock /code/repo-worktrees/issue-42
```

The reason in parentheses is whatever you passed to `git worktree lock --reason`; it is left
out when the lock has none. This is exit code `8`, and `--dry-run` reports it too.

## Safety rails

- **Exact matches only.** Unlike `cwt`, gitnuke never substring-matches: asking for
  `issue-42` will not resolve `issue-421`. A target that matches both one worktree's
  directory name and another's branch name is reported as ambiguous rather than guessed at.
- **Never your own directory.** gitnuke refuses to remove the worktree your shell is
  standing in (or any parent of it) and names somewhere else to `cd` to first. It is a
  binary, not a shell function, so it cannot move your shell out of a directory it deletes.
- **Never the main worktree.** It is the worktree the repository itself lives in, and git
  declines to remove it with or without `--force`. gitnuke refuses it up front and says what
  to do instead, rather than letting git's `fatal: '…' is a main working tree` be the answer —
  a `--dry-run` never runs that command, so leaving the rule to git meant the dry run cleared
  the one worktree nothing can ever remove. Both runs report exit `2`, the code git's own
  refusal produces.
- **Never a branch whose removal git refused.** The branch is deleted only once the worktree
  is actually gone.
- **Uncommitted work is gitnuke's own refusal.** Submodules and uncommitted changes are both
  diagnosed by gitnuke before `git worktree remove` is invoked, so each gets a message that
  names what is in the way and an exit code of its own (`5` and `9`) rather than git's
  locale-dependent `fatal:` under the generic `2`.
- **A lock is honoured, not overridden.** A worktree locked with `git worktree lock` is
  refused even with `--force`, quoting the lock reason if git recorded one and handing back
  the `git worktree unlock` command to run.
- **A mistyped branch gets a question.** gitnuke asks before it deletes a branch that no
  recent commit names, and it refuses with exit `10` when it cannot ask. See
  [A branch that no recent commit names](#a-branch-that-no-recent-commit-names).
- **`--dry-run` is a real preflight.** It runs every check a real run runs — submodules,
  uncommitted changes, whether a recent commit names the branch, and under `--safe` whether
  the branch is merged — and exits with the same status that run would, so `gitnuke -n x`
  failing means `gitnuke x` would fail too.

## Options

- `-f, --force`: Nuke the worktree despite the two refusals it overrides — submodules checked
  out, or uncommitted changes. It does not override a locked worktree; that is refused
  separately, with instructions to unlock it. It does not skip the question about a branch
  that no recent commit names
- `-s, --safe`: Keep the branch unless it is fully merged (`git branch -d` semantics instead
  of the default force-delete). Only affects the branch; the worktree is still removed
- `-n, --dry-run`: Report what would happen without removing or deleting anything
- `-y, --yes`: Do not ask about a branch that no recent commit names. Without a terminal,
  gitnuke needs this flag to delete such a branch
- `[TARGETS]...`: One or more worktrees, each given as a path, directory name, or branch name

## Exit Codes

- `0`: Success
- `1`: Not in a git repository
- `2`: A git command failed
- `3`: No worktree matched the target
- `4`: The target matched more than one worktree
- `5`: The worktree contains submodules and `--force` was not given
- `6`: The shell is standing inside the target worktree
- `7`: The worktree was removed but its branch could not be deleted
- `8`: The worktree is locked, which `--force` does not override
- `9`: The worktree contains modified or untracked files and `--force` was not given
- `10`: No recent commit names the branch, and nobody answered yes

## Replacing the shell functions

gitnuke supersedes the hand-rolled `gitnuke`/`gitclean` shell functions this pattern usually
starts as:

```bash
# Before: deletes the branch even when the worktree removal failed, and only
# works when the worktree's directory name happens to equal its branch name.
gitnuke() { git worktree remove "$@"; git branch -D "$@"; }
```

If you keep a `gitclean` alias for the safe variant, point it at the binary so it inherits
the same submodule handling and ordering guarantees:

```bash
gitclean() { gitnuke --safe "$@"; }
```
