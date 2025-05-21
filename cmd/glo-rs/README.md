# glo - Git Large Objects (Rust Implementation)

A tool to find large objects in Git repositories.

## Description

`glo` helps you identify large objects in your Git repository, which can be useful for:
- Reducing repository size
- Identifying files that should be stored using Git LFS
- Troubleshooting slow Git operations

## Installation

```
cargo install --git https://github.com/timmattison/tools glo
```

## Usage

```
glo [--repo <path>] [--top <n>]
```

### Options

- `--repo <path>`: Path to the Git repository (optional, defaults to automatically finding the repository from the current directory)
- `--top <n>`: Number of largest objects to display (default: 20)

## Example

```
$ glo
Using Git repository at: /path/to/repo
f7a5de3a2612 1.2 MiB images/large-image.png
a8b4c6d9e1f2 850.5 KiB documents/report.pdf
c7d8e9f1a2b3 425.3 KiB assets/video.mp4
...
(showing top 20 largest objects by default)
```

To show more or fewer objects:

```
$ glo --top 5    # Show only the 5 largest objects
$ glo --top 100  # Show the 100 largest objects
```

## How It Works

`glo` is equivalent to the following Git command pipeline:

```
git rev-list --objects --all --missing=print |
  git cat-file --batch-check='%(objecttype) %(objectname) %(objectsize) %(rest)' |
  sed -n 's/^blob //p' |
  sort --numeric-sort --key=2 |
  cut -c 1-12,41- |
  $(command -v gnumfmt || echo numfmt) --field=2 --to=iec-i --suffix=B --padding=7 --round=nearest
```

But implemented in Rust for better performance, portability, and integration with other tools.