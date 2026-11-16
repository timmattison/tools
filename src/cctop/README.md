# cctop (Claude Code top)

cctop is a full-screen terminal view of your Claude Code sessions. It shows one row for each live session. It tells you when a session needs you. It brings that session to the front when you press Enter.

## What it shows

Each row has these parts:

- A color block with the state of the session.
- The state: `working`, `needs-you`, `idle`, `agents`, or `shells`. A handled session adds ` - handled`, for example `idle - handled`.
- The time in that state.
- The Zellij session, or `—` outside Zellij.
- The name of the directory of the session.
- The registry name of the session, if it has one.

The rows are in this order: `needs-you`, `idle`, `agents`, `shells`, `working`. Rows in the same state show the oldest first, then the handled rows in the order of their marks.

### The handled mark

Press `h` to mark the selected session as handled. The row goes to the bottom of the rows in its state, and its state shows ` - handled`. The latest mark goes to the bottom of its state. The cursor then selects the next row that has no mark, thus you can press `h` again to mark that session. When no such row is below it, the cursor stays on the marked row.

The mark stays until the session changes state. A new write of the same state keeps the mark. When the state changes, the mark goes away, and it does not come back when the session returns to the old state.

Press `h` on a handled session to remove the mark. The row goes back to its place, and the cursor goes with it. `n` skips a handled session. The change log does not show a mark.

The colors are the colors of the `claude-tab-bar` Zellij plugin (`Palette::DEFAULT` in its `src/paint.rs`), thus cctop and the tab bars agree:

| State | Color |
| --- | --- |
| `working` | green, `#2f9e44` |
| `needs-you` | amber, `#fab005` |
| `idle` | red, `#e03131` |
| `agents` | white, `#ffffff` |
| `shells` | cyan, `#00cccc` |

Under the rows, cctop shows the change log. It lists the changes since cctop started, newest first. A change is a new session, a new state, or the end of a session. The first read at start logs nothing.

The last line is the status line. It shows the result of the last focus request or the last notification error. Without a message, it shows the keys.

## The state files

cctop reads one file for each Claude Code process:

```text
$CLAUDE_CONFIG_DIR/state/pane-state/<pid>.json
```

Without `CLAUDE_CONFIG_DIR`, the directory is `$HOME/.claude/state/pane-state/`. An empty variable counts as unset. Each file holds one line of JSON with these fields: `version` (1), `pid`, `sessionId`, `startedAt` (ms), `state`, `stamp` (ms), `cwd`, `zellijSession`, and `zellijPane`. cctop ignores `*.tmp` files.

The `zellij-pane-state` mod of timmattison/dotfiles issue #44 writes these files. Without that mod, the directory stays empty, and cctop shows no session.

### The stale rule

A session is stale when its pid has no live process. It is also stale when the process with that pid started after `startedAt`, because a new process reused the pid. A zombie counts as gone.

cctop removes the file of a stale session. Before it removes the file, it reads the file again and checks the rule on the new content. A new process can write a new file in the meantime.

## The notification

When a session changes to `needs-you`, cctop sends a macOS notification. It runs `osascript` and passes the text as arguments, never inside the AppleScript source. Thus no character in a directory name needs quoting. A session that is already in `needs-you` when cctop starts does not notify.

Use `--no-notify` to turn the notification off.

## The keys

| Key | Action |
| --- | --- |
| `j`, Down | Select the next row. |
| `k`, Up | Select the previous row. |
| Enter | Focus the selected session. |
| `n` | Select the next `needs-you` session that has no handled mark, and focus it. |
| `h`, `H` | Mark the selected session as handled, or remove its mark. |
| `q`, Ctrl-C | Quit. |

## The focus

A focus has two steps. Both steps always run. An error of one step shows in the status line, and cctop continues.

### Step 1: the Zellij pane

This step runs only when the state file has a Zellij session and a pane. cctop runs `zellij list-sessions --no-formatting` and checks that the session is live. It does not use `--short`, because `--short` lists EXITED sessions with no mark. Then it runs:

```text
zellij --session <session> action focus-pane-id terminal_<n>
```

The answer "already focused" counts as success. Each command has a limit of 5 s.

### Step 2: the WezTerm window

1. cctop finds the tty. Inside Zellij, it finds the `zellij --server` ancestor of claude. It then runs `lsof -a -U -c zellij -F pdn` to find the Zellij clients of that server. It never uses the session name to find a client, because `zellij a <prefix>` attaches by prefix. Outside Zellij, it uses the tty of claude.
2. It finds the WezTerm pane with that `tty_name` in the output of `wezterm cli list --format json`.
3. It writes this request to the tty of that pane:

   ```text
   ESC ] 1337 ; SetUserVar=claude-focus-pane=<base64 pane id> BEL
   ```

cctop writes to the tty of the target pane, not to its own terminal. When cctop runs in a Zellij pane, Zellij keeps the OSC, and WezTerm never gets it. The handler reads only the value, thus any WezTerm pane can send the request.

The `user-var-changed` handler in `~/.wezterm.lua` reads the value and brings the window to the front.

## Limits

- With two clients on one Zellij session, Zellij moves the client that typed last. cctop uses the first client, in pid order, that is a WezTerm pane.
- A renamed Zellij session makes step 1 fail. The status line shows the error.
- With a terminal that is not WezTerm, only step 1 works.
- cctop shows only the sessions of its own macOS user.
- The request bytes go to a tty that the Zellij client also writes to. A rare redraw glitch is possible.
- The notification and the focus steps use macOS tools. cctop was not tested on a different OS.

## Install

```bash
cargo install --git https://github.com/timmattison/tools cctop
```
