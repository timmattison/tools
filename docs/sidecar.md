# sidecar

`sidecar` keeps the data of a project out of its code repository. Each set of data has a name and its own private repository. `sidecar` clones that repository once and links it into each worktree of the project.

## Terms

- **Host**: the repository that holds the code, for example `megarepo`.
- **Sidecar**: the private repository `<host>-data-<name>` that holds the data of one name.
- **Main clone**: the one clone of a sidecar. It is at `<parent of the main checkout>/<host>-data-<name>`, next to the main checkout of the host. All worktrees of the host share it.
- **Link**: `<link_dir>/<name>` in a worktree of the host. It is a symbolic link to the main clone, or to a sidecar worktree.
- **Sidecar worktree**: a git worktree of the main clone. It is at `<parent of the main checkout>/<host>-data-<name>-worktrees/<branch>`.

## The registry

The registry is the file `sidecar.toml` at the root of the host.

```toml
# The directory of the links in each worktree. Git must ignore it.
link_dir = "data"
# The environment variable that holds the CI token.
token_env = "SIDECAR_TOKEN"

sidecars = ["testdata", "remotesensing.fyi"]
```

- `link_dir` defaults to `data`. `token_env` defaults to `SIDECAR_TOKEN`.
- An unknown key is an error, because the file uses `deny_unknown_fields`. A typing error thus never gets a default.
- A name holds lowercase letters, digits, hyphens, and dots. A letter or a digit is at each end.
- `pleasantville.cc` is a name. `..` is not a name.
- The name rule has a purpose. A link never leaves `link_dir`, because no name can hold `..` or a slash.
- A name that occurs two times is an error.
- `sidecar add` changes the file and keeps its comments and its order.

## Names, URLs, and places

- `<host>` is the last path segment of the `origin` URL of the host, without `.git`.
- The URL of a sidecar is the `origin` URL of the host with only its last segment changed to `<host>-data-<name>`. An SSH host thus gets SSH sidecars, and an HTTPS host gets HTTPS sidecars.
- `sidecar` accepts a main clone whose `origin` names the same repository in the other form (SSH or HTTPS). It does not change that `origin`.
- `sidecar` finds the main checkout of the host through `git rev-parse --git-common-dir`. A command thus works in any worktree of the host.
- The branch of each sidecar is `main`.

## `sidecar setup`

`sidecar setup` clones each sidecar and links it. For each name of the registry, in order, it applies these eight rules:

1. If the main clone is missing, clone it on `main`.
2. If a directory at the place of the main clone is not a clone of the sidecar, do not touch it. Write a warning and make no link. If its `origin` is correct but its clone did not end (`HEAD` names no commit, or the index is missing), do the same, and name `sidecar setup` in the warning.
3. If the link is missing, make a link to the main clone.
4. If the link points at the main clone or at a sidecar worktree, keep it.
5. If the link is broken, point it at the main clone.
6. If the link is a real directory, a file, or a link to a different directory, do not touch it. Write a warning.
7. If the clone fails and a directory is now at the place of the main clone, examine it as in rule 2. Another setup can make the clone first at the same time. If that clone is usable, use it.
8. If the clone fails and nothing usable is at the place of the main clone, write a warning that names the name and `sidecar setup`. Continue with the next name.

An empty sidecar has no `main` branch. The setup reports that case as a warning too.

When each main clone and each link is correct, the setup makes no network call and writes nothing. The hook runs on each checkout, thus this case is fast.

### The ignore check

Before it starts, the setup makes sure that git ignores `link_dir` (`git check-ignore`). If git does not ignore it, the setup refuses and prints the `.gitignore` line to add:

```text
/data/
```

Nobody can then commit a link.

### The CI token

If the variable that `token_env` names is set and not empty, `sidecar` gives the token to each git command it starts. It uses `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_0`, and `GIT_CONFIG_VALUE_0`, as the `http.https://github.com/.extraheader` setting. The token is never in a remote URL, in a git configuration file, on a command line, or in the output.

### The two modes

| Mode | For | Exit code |
| --- | --- | --- |
| `sidecar setup` | A person and CI | `1` when it cannot start, or when a sidecar is not linked at the end. Else `0`. |
| `sidecar setup --hook [<old> <new> <flag>]` | A post-checkout hook and an install script | Always `0`. Warnings go to standard error. |

`--hook` does nothing in two cases:

- It gets three arguments and the flag is `0`. That checkout changed files, not a branch.
- `CI` is set to a value other than empty, `0`, or `false`.

## The commands

### `sidecar add <name> [--yes]`

`sidecar add` makes a sidecar and registers it. It does these seven steps:

1. Refuse a name that the name rule refuses.
2. Show the steps that remain, and ask for a yes. `--yes` skips the question. Without a terminal and without `--yes`, refuse. When no step remains, ask nothing.
3. Read `repos/<owner>/<host>-data-<name>` through `gh api`. If it is private, use it. If it is public, refuse and change nothing. If it is missing, make it private.
4. If the sidecar has no `main` branch, push a first commit with `README.md` and `.gitattributes`.
5. Add the name to `sidecar.toml`. If git does not ignore `link_dir`, add its line to `.gitignore`.
6. Clone the sidecar and link it, as `sidecar setup` does.
7. Print the steps that the tool cannot do.

Each step does nothing when its work is already done. A second `sidecar add` of the same name thus finishes a partial one, and exits with `0` when nothing remains. For an existing private repository, `sidecar add` only registers, clones, and links it.

The tool cannot do these three steps. It prints them at the end of each run:

1. Give the CI token read access to the new repository at `https://github.com/settings/personal-access-tokens`. GitHub has no API that adds a repository to a fine-grained personal access token.
2. Name the sidecar in the README of the project.
3. Commit `sidecar.toml` and `.gitignore`, and merge them through a pull request.

### `sidecar status`

`sidecar status` reads and makes no network call. It prints one row for each sidecar.

| Column | States |
| --- | --- |
| `CLONE` | `missing`, `not a clone of the sidecar`, `clone did not end`, `ready` |
| `LINK` | `missing`, `at the main clone`, `at a worktree on branch <branch>`, `at a worktree on a detached HEAD`, `broken`, `foreign` |

It also prints a row marked `not registered` for each `<host>-data-*` directory next to the main checkout that the registry does not name. That row shows a half-made sidecar. The command exits with `1` when a row is not ready.

### `sidecar path <name>`

If the link of `<name>` in the current worktree points at the main clone or at a sidecar worktree, `sidecar path` prints the absolute path of the link and exits with `0`. Else it exits with `1`, and its message on standard error names the fix:

- The registry does not name the sidecar: `sidecar add <name>`.
- The link is missing or broken: `sidecar setup`.
- Something else is at the place of the link: move it away, then `sidecar setup`.

A program in any language runs `sidecar path <name>`. It gets the path, or it stops with that message. The program never makes the directory. A shell program does it like this:

```sh
dir=$(sidecar path testdata) || exit 1
```

The message of `sidecar path` is already on standard error, thus the program adds none.

### `sidecar worktree <name> [<branch>]`

`sidecar worktree` points the link of a sidecar at a sidecar worktree. It has three forms:

- `sidecar worktree <name>` uses the current branch of the host. If the host has a detached `HEAD`, it refuses.
- `sidecar worktree <name> <branch>` uses `<branch>`.
- `sidecar worktree <name> main` points the link at the main clone again and makes no worktree.

The rules:

- A new sidecar worktree is at `<parent of the main checkout>/<host>-data-<name>-worktrees/<branch>`.
- If the sidecar already has a worktree on `<branch>`, `sidecar` uses it. Two worktrees of the host that ask for the same branch thus share one sidecar worktree.
- A new sidecar worktree takes its branch from the first of these sources: the local branch, then `origin/<branch>` after a fetch, then a new branch from `main`.
- If the fetch fails, `sidecar` writes a warning and continues with the local branches.
- The link changes atomically. `sidecar` makes a new link and renames it over the old link.
- Only the link of `<name>` in the current worktree changes.

## `sidecar` never deletes

No command deletes a repository, a main clone, a sidecar worktree, a branch, or a change. When a link moves away from a sidecar worktree, that worktree and its changes stay.

## What goes in a sidecar

- **`.gitattributes` with `* -text`.** Git then keeps each data file byte-identical under any `core.autocrlf` setting. Without it, `core.autocrlf=input` changed the CRLF line endings of the `.j2w` world files in `megarepo-data-remotesensing.fyi`. `sidecar add` writes this file in the first commit.
- **A README that names the host and the name.** `sidecar add` writes it in the first commit.
- **Files below 100 MB.** GitHub refuses a larger file.
- **No Git LFS.** A clone then needs no `git-lfs`, and no LFS bandwidth quota applies.

## The flow for a change to data

1. In the worktree of the issue, run `sidecar worktree <name>`.
2. Change the data, commit it in the sidecar worktree, and push the branch.
3. Open a pull request in the sidecar.
4. Name that pull request in the pull request of the host.
5. Merge the sidecar pull request first. Thus `main` of the host never reads data that `main` of the sidecar does not have.
6. Run `sidecar worktree <name> main`.

## The setup of a host

### The registry and the ignore line

Write `sidecar.toml` at the root of the host. Add this line to `.gitignore`:

```text
/data/
```

If you set another `link_dir`, use that name in the line. `sidecar add` writes both for you.

### The post-checkout hook

Put this hook in `.git/hooks/post-checkout`, or in the hooks directory of your hook tool. With husky, it goes in `.husky/post-checkout`.

```sh
command -v sidecar >/dev/null 2>&1 || exit 0
sidecar setup --hook "$@"
```

The first line makes the hook do nothing on a machine that has no `sidecar`.

### The install script

Run the setup from the install script, because a new worktree can start without a hook. In a `package.json`, a `prepare` script does it:

```json
{
  "scripts": {
    "prepare": "command -v sidecar >/dev/null 2>&1 && sidecar setup --hook || true"
  }
}
```

`sidecar setup --hook` always exits with `0`, thus it needs no `|| true` of its own. The `|| true` here only covers the `command -v` test, which exits with `1` when `sidecar` is missing.

### The CI step

1. Install `sidecar`: `cargo install --git https://github.com/timmattison/tools sidecar`.
2. Make a fine-grained personal access token with read access to the contents of each sidecar.
3. Store the token as a secret. Put the secret in the variable that `token_env` names.
4. Run `sidecar setup`. It exits with `1` when a sidecar is not linked.

A GitHub Actions step:

```yaml
- name: Link the sidecars
  run: |
    cargo install --git https://github.com/timmattison/tools sidecar
    sidecar setup
  env:
    SIDECAR_TOKEN: ${{ secrets.SIDECAR_TOKEN }}
```
