# cwt (change worktree)

- Change Worktree - Navigate between the git worktrees of a repository and of the
  repositories inside it. Shows a list of all worktrees with the current one highlighted
  and the number of each open pull request after its branch, or cycle through them with
  `-f` (forward) and `-p` (previous). Can also jump directly to a worktree by directory
  name or branch name. Use `--no-family` to stay inside one repository, and
  `--shell-setup` to automatically add shell integration to your config.
- To install: `cargo install --git https://github.com/timmattison/tools cwt`

Navigate between the git worktrees of a repository and of the repositories inside it. Lists them all, cycles through them, or jumps to a specific one by name.

## Basic Usage

```bash
cwt                           # Show list of worktrees with current highlighted, and open PR numbers
cwt -f                        # Go to next worktree (wraps around)
cwt -p                        # Go to previous worktree (wraps around)
cwt -m                        # Go to the main worktree, or up a level when you are at its root
cwt main                      # Go to worktree by branch name
cwt absurd-rock               # Go to worktree by directory name
cwt vial-qmk:vial             # Go to one repository's worktree by name
```

## Options

- `-f, --forward`: Go to the next worktree in the sorted list (wraps around)
- `-p, --prev`: Go to the previous worktree (wraps around)
- `-m, --main`: Go to the main worktree (see [The main worktree](#the-main-worktree))
- `[TARGET]`: Worktree to switch to (directory name, branch name, or `REPO:NAME`)
- `--no-family`: List only the repository you are standing in
- `--shell-setup`: Automatically add shell integration to your ~/.zshrc or ~/.bashrc
- `-q, --quiet`: Suppress error messages

## The main worktree

`cwt -m` (and the `wtm` alias) goes to the worktree on branch `main`. If no worktree is on
`main`, it goes to the worktree on branch `master`, so a repository that never renamed its
first branch gets the same shortcut.

In a family, `cwt -m` stays in the repository you are standing in until you are already at
the top of its main worktree. Every repository of a family has a main worktree of its own,
and the shortcut takes you to yours.

The branch name must match exactly. This is what separates `cwt -m` from `cwt main`, which
also substring-matches: in a `master` repository, `cwt main` lands on a branch such as
`wt-main-master`, and `cwt -m` does not. A detached worktree has no branch, so it is never
the main worktree.

If your repository has neither branch checked out, `cwt -m` lists the worktrees and exits
with code 3.

In a repository whose git directory is detached from its work tree — a `yadm` repository of
dotfiles, whose git directory is `~/.local/share/yadm/repo.git` and whose work tree is your
home directory — git names the git directory itself as the main worktree. Git builds that
name from the git directory with a trailing `/.git` removed, and this layout has no such
suffix. So the path `cwt -m` takes you to ends in `.git`, and that is also the path the
listing shows. Every `cwt` command works from there.

### Pressing it again climbs out

When the directory you are in **is** the main worktree, you have asked to go up. So the
next `wtm` takes you to the main worktree of the repository that **holds** yours, and the
one after that goes up again, for as deep as the repositories are nested:

```bash
cd keyboards/zmk-config-corne-worktrees/guard-the-commit-hashes-in-prose
wtm   # -> keyboards/zmk-config-corne        the repository's own main worktree
wtm   # -> keyboards                         the repository that holds it
wtm   # Error: No repository above /code/keyboards has a main worktree
```

It is the directory you stand in that decides, not the worktree that holds you. From a
subdirectory of the main worktree — `zmk-config-corne/config`, say — `wtm` takes you to the
top of it, exactly as it does from a subdirectory of any other worktree. Only the top of
the main worktree has anywhere left to go, so that is the only place the climb starts:

```bash
cd keyboards/zmk-config-corne/config
wtm   # -> keyboards/zmk-config-corne        the top of the worktree you are in
wtm   # -> keyboards                         now the climb starts
```

The climb is measured from where your repository **sits on disk** — its own main worktree —
never from the worktree you happen to stand in. That is what keeps the first `wtm` above
from skipping `zmk-config-corne` and landing on `keyboards`.

Only the directory directly above counts, the same one level the family scan looks down. A
repository on the way with neither `main` nor `master` cannot be a destination, so the
climb steps over it and asks the repository above it. When nothing above has a main
worktree, `cwt -m` says so and exits with code 3. `--no-family` does not change any of
this: it says which repositories the **listing** shows, and the climb is not a listing.

## Families of Repositories

Some repositories are containers. They track the map of a workspace, and the real
repositories sit one level below them, kept out of the parent's history by
`.gitignore`:

```text
keyboards/                    # the parent repository
  qmk_firmware/               # a repository of its own
  vial/                       # a repository of its own
  vial-qmk/                   # a repository of its own
  zmk-config-corne/           # a repository of its own
```

`cwt` treats the parent and its children as one **family**. One listing shows every
worktree of every repository, grouped by repository, and one name reaches any of them:

```bash
cd keyboards && cwt
# keyboards
# >   /code/keyboards                                          [main]
#     /code/keyboards-worktrees/keymap-parity-tool             [keymap-parity-tool]
#
# qmk_firmware
#     /code/keyboards/qmk_firmware                             [master]
#
# vial-qmk
#     /code/keyboards/vial-qmk                                 [vial]
#     /code/keyboards/vial-qmk-worktrees/split-handedness-build [split-handedness-build]
```

The family is the same from anywhere inside it, so `cwt` gets you out of a child
repository as easily as it gets you in. Only one level is scanned: a child of a child
is that child's business. `nwt` links the children into each new worktree of the parent
(see [Child Repository Links](../../README.md#child-repository-links)).

A directory that looks like a repository but has no worktree to offer — a bare
repository, which git lists without a HEAD — cannot join the family, because there is
nothing there to change into. `cwt` leaves it out of the listing and says so on
standard error, so a directory you can see is never missing without a reason:

```text
Warning: skipped /code/keyboards/archive: no worktrees to list
```

### Which repository answers a name

A name is offered to each repository in turn, nearest first:

1. The repository you are standing in
2. The parent repository the family is anchored at
3. Every other repository in the family

So `wtm` still means "my repository's main branch" wherever you are standing — and,
once you are standing in it, the repository above that. Within
each repository the order is the same as before: exact directory name, then exact
branch name, then a case-insensitive substring of a branch name. An exact name
anywhere in the family beats a substring anywhere.

### When two repositories share a name

`cwt` refuses to guess, and names the candidates the way you have to type them:

```bash
cwt master
# Error: Multiple worktrees match 'master'. Be more specific:
#   qmk_firmware:qmk_firmware [master]
#   zmk-config-corne:zmk-config-corne [master]
```

A `REPO:NAME` target searches one repository of the family. The repository can be
named in full or by any part of its name, and `REPO:` on its own selects that
repository's main worktree:

```bash
cwt zmk-config-corne:master    # that repository's master
cwt zmk:                       # that repository's main worktree
```

Every name `cwt` prints can be typed straight back, so two repositories of one family
cannot be left sharing a name — not even the parent and a child named after it. The
child is listed, and typed, as the path that leads to it:

```bash
cd keyboards && cwt
# keyboards
# >   /code/keyboards                              [main]
#
# keyboards/keyboards
#     /code/keyboards/keyboards                    [main]
#     /code/keyboards/keyboards-worktrees/inner    [inner]

cwt keyboards:                 # the parent
cwt keyboards/keyboards:       # the child checked out inside it
cwt keyboards/keyboards:inner  # a worktree of that child
```

### Staying in one repository

`--no-family` confines the listing, the cycling, and the name search to the repository
you are standing in. Set `CWT_NO_FAMILY` to any value other than `0` to make that the
default.

## Open pull requests

The list shows the number of each open pull request after the branch that is its head.
The numbers are in ascending order, and a row with no open pull request does not change:

```text
> /code/tools-worktrees/issue-546 [issue-546] #547
  /code/tools-worktrees/170 [170]
  /code/tools-worktrees/ic-xtermjs [ic-xtermjs] #512 #519
```

The current row paints the numbers green, as it paints its branch. Other rows paint them
dimmed. A draft pull request is open, so it shows. Closed and merged pull requests do not
show.

`cwt` asks GitHub through [`gh`](https://cli.github.com/), about the repository that
`origin` names. That is also true in a fork that has an `upstream` remote. A pull request
shows only when its head is a branch of the repository that `origin` names, so the `main` or
`patch-1` of a fork never attaches to your branch of the same name. `cwt` sends one query for
each repository of the family, and up to 32 queries run at a time.

- Only the list asks GitHub. `cwt NAME`, `-f`, `-p`, `-m`, and the error lists stay
  offline, because the `wt` function waits for the path that they print.
- A repository whose `origin` is not on `github.com`, or on the host that `GH_HOST` names,
  shows no numbers, and `cwt` does not run `gh` for it. `ssh.github.com`, the SSH host of
  GitHub on port 443, counts as `github.com`. An SSH host alias from `~/.ssh/config`, such
  as `git@github-work:owner/name.git`, is a different host to `cwt`, so it shows no numbers.
- A machine with no `gh` shows no numbers and prints nothing more.
- If `gh` fails or does not answer in time, that repository shows no numbers, and `cwt`
  prints one warning line that names the repository and the cause. `--quiet` removes the
  line.
- `CWT_PR_TIMEOUT` sets the time limit in seconds, for example `10` or `0.5`. The default
  is 3. The queries run in groups of up to 32, and the limit applies to each group, not to
  each repository. Thus a family of up to 32 repositories waits for the limit once at most.
  A larger family can wait once for each group of 32.
- `cwt` asks for up to 1000 open pull requests of each repository. A repository that has
  1000 or more gets a warning that some numbers can be missing.

## Shell Integration

The easiest way to set up shell integration is:

```bash
cwt --shell-setup
```

This automatically adds the `wt` function and aliases to your shell config. Run `source ~/.zshrc` (or `~/.bashrc`) to activate, or open a new terminal.

> **Note:** `--shell-setup` currently supports bash and zsh only. Fish users should use the manual setup below.

### Manual Setup

If you prefer to add it manually, since a program can't change the parent shell's directory, cwt outputs the target path to stdout. Add these shell functions to enable directory changing:

### Bash / Zsh (~/.bashrc or ~/.zshrc)

```bash
# Change to a git worktree
function wt() {
    if [ $# -eq 0 ]; then
        # No args: show list interactively
        cwt
    else
        local target=$(cwt "$@")
        if [ $? -eq 0 ] && [ -n "$target" ]; then
            cd "$target"
        fi
    fi
}

# Quick navigation aliases
alias wtf='wt -f'  # Next worktree
alias wtb='wt -p'  # Previous worktree (back)
alias wtm='wt --main'  # Main worktree, or a level up when you are at its root
```

### Fish (~/.config/fish/config.fish)

```fish
function wt
    if test (count $argv) -eq 0
        cwt
    else
        set -l target (cwt $argv)
        if test $status -eq 0 -a -n "$target"
            cd $target
        end
    end
end

# Quick navigation aliases
alias wtf 'wt -f'  # Next worktree
alias wtb 'wt -p'  # Previous worktree (back)
alias wtm 'wt --main'  # Main worktree, or a level up when you are at its root
```

## Examples

Show all worktrees with current highlighted:
```bash
cwt
#   /path/to/repo                        [main]
# > /path/to/repo-worktrees/absurd-rock  [feature-branch]
#   /path/to/repo-worktrees/zen          [fix-bug]
```

Cycle through worktrees:
```bash
wt -f    # Move to next worktree
wt -p    # Move to previous worktree
```

Jump to specific worktree:
```bash
wt main           # By branch name
wt absurd-rock    # By directory name
wt vial-qmk:vial  # By repository and branch name
wtm               # The main worktree, or a level up when you are at its root
```

## Exit Codes

- `0`: Success
- `1`: Not in a git repository
- `2`: Git command error
- `3`: Worktree not found
- `4`: Could not determine current worktree (for -f/-p)
- `5`: Shell setup failed
- `6`: Multiple worktrees matched the name (be more specific)
