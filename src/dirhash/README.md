# dirhash

- Gets a SHA256 hash of a directory tree. This is useful for comparing two directories to see if they are
  identical. This hash will only be the same if the directories have the same file names and the same file contents.
  However, we ignore the directory names and locations of files in the directories. Skips hidden files and the
  files that .gitignore and the other standard ignore files name, and reports on stderr how many it left out.
  See below for an example.
- To install: `cargo install --git https://github.com/timmattison/tools dirhash`

Calculate a SHA256 hash of a directory tree that's deterministic based on file contents. Skips hidden files and the files that .gitignore and the other standard ignore files name.

## Usage

```
dirhash [OPTIONS] <DIRECTORY>
```

## Options

- `--no-ignore`: Don't respect any ignore file — `.ignore`, `.gitignore`, the global gitignore, and `.git/info/exclude`
- `--no-ignore-vcs`: Don't respect the VCS ignore files — `.gitignore`, the global gitignore, and `.git/info/exclude`. `.ignore` files still apply
- `--hidden`: Include hidden files and directories

`--no-ignore` covers everything `--no-ignore-vcs` covers, so `--hidden --no-ignore` hashes every file on disk.
Hidden files are a separate group: no ignore flag brings them in, and `--hidden` is the only flag that does.

## Features

- **Skips hidden files**: Automatically excludes hidden files and hidden directories
- **Respects ignore files**: Automatically excludes the files that .gitignore, .ignore, and the other standard ignore files name
- **Clean output**: Outputs only the final hash to stdout for easy scripting
- **Informative messages**: Counts the excluded files on stderr, hidden ones apart from ignored ones, and names the
  flag that brings each group in:
  `Note: 88 file(s) excluded: 38 hidden, 50 ignored. Use --hidden and --no-ignore to include them.`
- **Fast**: Uses parallel processing for hashing multiple files

## How it works

If you have two directories with the following contents:

```
dir1/
  file1.txt
  file2.txt
  subdir1/
    file3.txt
    file4.txt
```

```
dir2/
  subdir1/
    file1.txt
    file2.txt
  subdir2/
    file3.txt
    file4.txt
```

As long as the contents of `file1.txt`, `file2.txt`, `file3.txt`, and `file4.txt` are the same in both directories, the
hashes will be the same. The subdirectory names and locations are ignored.

## Examples

Basic usage (skips hidden files and ignored files):
```bash
dirhash /path/to/directory
```

Ignore .gitignore but keep .ignore files:
```bash
dirhash --no-ignore-vcs /path/to/directory
```

Include hidden files and directories:
```bash
dirhash --hidden /path/to/directory
```

Hash every file on disk, leaving nothing out:
```bash
dirhash --hidden --no-ignore /path/to/directory
```

Compare two directories:
```bash
if [ "$(dirhash dir1)" = "$(dirhash dir2)" ]; then
  echo "Directories have identical contents"
fi
```
