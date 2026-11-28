# bibibi (bye bye bye)

Find the commit that caused a regression, with a test from a later commit. The name is a reference to the NSYNC song "Bye Bye Bye": `bibibi` finds the bad commit, and then you can say goodbye to it.

TDD gives a red commit that proves a bug. The next question is often "which commit caused this bug?" `git bisect` answers that question, but it cannot use the red test. At each step, bisect puts the tree of an older commit in the working directory, and the red test is part of a newer tree. `git help bisect` gives a workaround ("Automatically bisect with temporary modifications"), and each person writes that script again. A script written by hand blames an unrelated commit when the build fails there, stops when a signal kills the test, never tests the two ends of the range, and can leave the tree dirty for the next checkout.

`bibibi` makes that procedure one command:

1. It takes the change of the test commit one time, as a patch (`git diff-tree -p --binary --full-index`).
2. It refuses a test commit that is a merge or a root commit or that changes no file, and a `--good` commit that is not an ancestor of the bad commit.
3. It makes a scratch worktree in a temporary directory. Your checkout does not change.
4. At each commit that it tests, it checks out the commit, applies the patch (`git apply`, then `git apply --3way`), runs the build, runs the test, and restores the tree (`git reset --hard`, then `git clean -fd`).
5. Before the search, it runs the test at each `--good` commit and at the bad commit (the pre-flight). If a good commit does not pass or the bad commit does not fail, it refuses, and the output of the build and the test is already on standard error.
6. It chooses each next commit from `git rev-list --bisect-all`, and keeps the search state in memory. It never runs `git bisect`, so a Ctrl-C leaves no `refs/bisect/*` and no `BISECT_LOG`.

## Basic Usage

```bash
# HEAD is the red commit that adds the test, and v1.4.0 is a release where the test passes
bibibi --good v1.4.0 -- cargo test -q --test regression_add

# Build first. A commit that does not build is a skip, not a bad commit
bibibi --good v1.4.0 --build 'cargo test --no-run -q' -- cargo test -q --test regression_add

# The test commit is not HEAD, the bad commit is not its parent, and two branches merged
bibibi 3f2a1bc --good a1b2c3d --good d4e5f6a --bad 9e8d7c6 -- sh tests/add.sh

# A test that can hang
bibibi --good v1.4.0 -- timeout 60 cargo test -q --test regression_add
```

## Options

```text
bibibi [<test-commit>] --good <commit>... [--bad <commit>] [--build <command>] -- <test-command>...
```

- `<test-commit>`: The commit that adds the test. Defaults to `HEAD`, which is usually the red commit that you just made. It must have one parent and change at least one file. If the test is in more than one commit, squash it into one red commit first.
- `--good <commit>`: A commit where the test passes. Necessary. Give the flag more than once for more than one good commit, for example one on each branch of a merge.
- `--bad <commit>`: A commit where the test fails. Defaults to the parent of the test commit, which has the bug but not the test.
- `--build <command>`: A shell command that runs before the test, through `sh -c`. If it exits non-zero or a signal stops it, `bibibi` skips the commit.
- `-- <test-command>...`: The test. `bibibi` runs it as an argument vector, without a shell. Write `-- sh -c '...'` when you need a shell.
- `-V`, `--version`: Print the version, the git hash, and whether the build was clean.
- `-h`, `--help`: Print the usage.

The build and the test run in the root of the scratch worktree, with `/dev/null` as standard input. Their standard output and standard error go to the standard error of `bibibi`, so the standard output of `bibibi` holds only the result. They get your environment without the `GIT_` variables that a git hook exports (`GIT_DIR`, `GIT_INDEX_FILE`, and the others), so a `bibibi` that a hook starts does not aim the test at a different repository. The six variables that a person sets on purpose stay: `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, `GIT_SSH`, `GIT_SSH_COMMAND`, `GIT_ASKPASS`, and `GIT_TERMINAL_PROMPT`.

## The verdict at one commit

| Result | Verdict |
| --- | --- |
| The patch does not apply, also with `--3way` | skip |
| The build exits non-zero, or a signal stops it | skip |
| The test exits 0 | good |
| The test exits 125 | skip |
| The test exits with a different code | bad |
| A signal stops the test | bad |

A skip never blames a commit. If skips leave more than one commit that can be the first bad commit, `bibibi` names each of them and exits 1.

## What it prints

Standard error has one line for each commit that `bibibi` tests: the short id, the verdict, the subject, the reason for a skip, and the approximate number of steps left. The subject gets shorter to fit the terminal. Standard output has only the result: the full id and the subject of the first bad commit.

```
$ COLUMNS=80 bibibi --good 0ea2e3d -- sh tests/add.sh
test: add(2, 2) must be 4
bibibi: 0ea2e3d good add the add function - pre-flight
test: add(2, 2) must be 4
bibibi: 9e5dd09 bad  write note 10 - pre-flight
test: add(2, 2) must be 4
bibibi: 40b0a0f bad  simplify the arithmetic - about 1 step left
test: add(2, 2) must be 4
bibibi: 50ce507 good write note 3 - about 1 step left
test: add(2, 2) must be 4
bibibi: 5a88026 good write note 5 - about 0 steps left
40b0a0f0eb62646296305a44a2183e911ccfe825 simplify the arithmetic
```

The `test:` lines are the output of the test script. When the build fails at the culprit and at its parent, skips leave more than one candidate. Standard output then has one line for each candidate:

```
$ COLUMNS=80 bibibi --good 0ea2e3d --build 'test ! -f note-5.txt || test -f note-7.txt' -- sh tests/add.sh
...
bibibi: 40b0a0f skip simplify the ari… - the build exited 1 - about 2 steps left
bibibi: 5a88026 skip write note 5 - the build exited 1 - about 2 steps left
...
40b0a0f0eb62646296305a44a2183e911ccfe825 simplify the arithmetic - skipped: the build exited 1
5a88026130fe71a05742aca6276ffccf7f34c215 write note 5 - skipped: the build exited 1
d09208ec7eea6e59b1a2c6b2e4df85a21c7c4708 write note 7 - bad
bibibi: skips leave 3 candidates for the first bad commit
```

## Exit codes

| Exit | Meaning |
| --- | --- |
| 0 | The search found one first bad commit. |
| 1 | Skips leave more than one candidate. |
| 2 | Usage error, refusal, or git error. |
| 128 + signal | A signal ended the run: 129 for SIGHUP (the terminal closed), 130 for SIGINT (Ctrl-C), 143 for SIGTERM. |

## Notes

- **Your checkout does not change.** HEAD, the index, your uncommitted files, and the `refs/bisect/*` of a `git bisect` that you have in progress stay as they are. The scratch worktree and the patch file go away after each end of the run: a result, a refusal, an error, a panic, SIGHUP, SIGINT, or SIGTERM.
- **A Ctrl-C never counts as a verdict.** The build and the test run in a session of their own. On SIGHUP, SIGINT, or SIGTERM, `bibibi` stops the process group of the build or the test (SIGTERM, then SIGKILL after 2 seconds), removes the scratch worktree, and exits 128 + signal. A Ctrl-C or a hangup of the terminal also reaches a git command of `bibibi`. If that git command fails first, the run can exit 2 in place of 130 or 129, and the cleanup still occurs. A second Ctrl-C during the cleanup can stop the `git worktree remove` of `bibibi` and leave the scratch worktree behind. It is in your temporary directory, and `git worktree list` shows it. Remove it with `git worktree remove --force <path>`. If its directory is already gone, run `git worktree prune`.
- **The build and the test have no terminal.** They run with no controlling terminal, as under CI. A command that opens `/dev/tty` (a password prompt, a pager, a terminal interface) fails at once, so the commit counts as bad, or as a skip when the build fails. Wrap such a command, or give it its input in another way.
- **The first build is cold.** Each worktree has its own `./target`, so the first build in the scratch worktree compiles everything. The restore never runs `git clean -x`, so ignored build output stays, and each build after the first is incremental. To share a build cache with your checkout, set `CARGO_TARGET_DIR`.
- **There is no `--timeout` flag.** If a test can hang, wrap it: `-- timeout 60 cargo test ...`. `timeout` exits 124, which counts as bad.
- **Limits.** `bibibi` does not support a test that is in more than one commit (squash it first), repositories with submodules, or Windows (`--build` runs through `sh -c`).

## Install

```bash
cargo install --git https://github.com/timmattison/tools bibibi
```
