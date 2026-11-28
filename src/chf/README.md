# chf (ch find)

Find a Claude Code session by the words in its messages, and resume it.

The `/resume` picker of Claude Code shows the sessions of one worktree by default. It does not open the expanded version of a compacted session. A person who remembers a word from a session, but not its folder, cannot find it. `chf` searches the sessions of every folder. It then resumes the session you pick with `ch` or `che`.

Run `chf` and type words. The list changes at each key press. A preview of the messages around the first match shows beside the list, or below it on a terminal narrower than 100 columns.

```bash
chf
```

## Keys

| Key | Action |
| --- | --- |
| Up, Down, `Ctrl-P`, `Ctrl-N` | Move the selection. |
| PageUp, PageDown | Move the selection by one page. |
| Shift-Up, Shift-Down | Scroll the preview. |
| `Ctrl-U` | Clear the query. |
| Enter | Resume the selected session. |
| Esc, `Ctrl-C` | Quit. |

## Matching rules

- Each word is a literal text. There is no fuzzy match.
- Each word must occur in the session. Words can occur in different messages.
- The case rule is "smart case" for each word. A word in lowercase matches text in any case. A word with a capital letter matches only that capital letter.
- The case folding follows Unicode.

## The row

Each row shows the time of the last message, a `⇊` mark with the count of compactions for a session that Claude Code compacted, the folder, and the title of the session. The preview header shows the session id, the folder, and the git branch.

## A compacted session

When you press Enter on a compacted session, `chf` asks which version to resume. Press `c` for `ch`, the compacted version. Press `e` for `che`, the expanded version. The question shows the token count of the conversation before the last compaction. The expanded version holds all of those tokens. Thus it can fill the context window of Claude at once. The question also tells you when the first match is in a message from before the last compaction. The compacted version holds that message only as part of a summary.

## Requirements

`chf` runs `crap --here` through `$SHELL -ic`. Thus:

- The `crap` shell function must exist. Run `crap --shell-setup` to install it.
- `$SHELL` must be `bash` or `zsh`. For other shells, `chf` prints the command to paste.
- `CRAP_LAUNCHER` applies, because the shell function starts Claude.

## The cache

`chf` keeps its index in `<cache dir>/chf`. It writes one file for each project folder. The directory has mode 0700 and each file has mode 0600. The first run reads every transcript and shows its progress. Later runs read only the files that changed. If `chf` cannot read a project folder or the store, it counts it as unreadable and keeps its cache files. If `chf` cannot stat a transcript, it counts it as unreadable and keeps its cache entry.

## What `chf` does not search

- Tool calls
- Tool results
- Thinking blocks
- Transcripts of subagents

## Exit codes

| Exit | Meaning |
| --- | --- |
| 0 | `chf --help` and `chf --version` print their text and exit with this code. |
| 1 | A failure, or a `$SHELL` that is not `bash` or `zsh`. |
| 2 | Standard input or standard output is not a terminal, or `chf` cannot find the home directory. |
| 130 | You pressed Esc or `Ctrl-C`. |

After Enter, `chf` replaces itself with the shell that runs `crap`. The exit status is then the status of that shell. This status can be any of the codes above or another code.

## Install

```bash
cargo install --git https://github.com/timmattison/tools chf
```
