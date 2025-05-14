# dirc - Directory Copypasta

A versatile command-line tool that can:
1. Copy the current working directory to the clipboard
2. Read a directory path from the clipboard and output a command to change to that directory (`paste` mode)

## Usage

```
dirc [options]
```

## Options

- `-help`: Show help message
- `-paste`: Paste cd command for directory in clipboard

## Important Note

Since a program cannot directly change the current directory of the parent shell, `dirc` outputs a `cd` command that must be evaluated by your shell.

### Recommended Setup

Add an alias to your shell configuration file:

**Bash/Zsh** (add to `~/.bashrc` or `~/.zshrc`):
```bash
alias dirp='eval $(dirc -paste)'
```

**Fish** (add to `~/.config/fish/config.fish`):
```fish
alias dirp='eval (dirc -paste)'
```

## Examples

```bash
# Copy current directory
dirc

# Evaluate the output to actually change directories
eval $(dirc -paste)

# Using the recommended alias
dirp
```

## Installation

```bash
go install github.com/timmattison/tools/cmd/dirc@latest
```

## How it works

In the default mode , `dirc` gets the current working directory and copies it to the system clipboard, making it available for pasting elsewhere.

In paste mode (`-paste`), `dirc` validates that the clipboard content is a valid directory, and outputs a properly escaped `cd` command. When this command is evaluated by the shell, it changes the current directory to the path from the clipboard.
