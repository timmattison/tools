# gitdiggin

A tool to recursively search Git repositories for commits containing a specific string.

## Usage

```
git-diggin [options] <search-term> [path...]
```

If no path is specified, the current directory is used.

## Options

- `--contents`: Search in commit contents (diffs) in addition to commit messages
- `--all`: Search all branches, not just the current branch
- `--root <dir>`: Specify the root directory to start scanning from (overrides positional arguments)
- `--ignore-failures`: Suppress output about directories that couldn't be accessed
- `--help` or `-h`: Show help message

## Examples

Search for "registration" in commit messages of all repositories under the current directory:
```
git-diggin registration
```

Search for "api" in both commit messages and contents of all repositories under a specific directory:
```
git-diggin --contents api /path/to/projects
```

Search for "fix" in all branches of repositories under the current directory:
```
git-diggin --all fix
```

## Output

The tool will display:
- The repository path
- The commit hash
- The commit message (first line)

For each matching commit.