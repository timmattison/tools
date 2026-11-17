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

### The title line

The top border of the table shows the number of sessions, then one count for each state. You can see how many sessions need you, and you do not have to read the rows:

```text
┌ cctop · 14 sessions · 1/2 ❗ · 3/7 💤 · 1/0 🤖 · 0/0 🐚 · 0/0 🏃 ───────┐
```

Each count has the shape `<no mark>/<handled> <marker>`. The left number counts the sessions in that state that have no [handled mark](#the-handled-mark). The right number counts the sessions in that state that have a handled mark. In the example, 10 sessions are in `idle`: 3 with no mark and 7 with a mark.

- The counts are in the order of the rows. The `needs-you` count stands first, thus a cut at the right edge removes it last.
- Each count shows, also when both numbers are 0. The empty board shows the five counts too, all `0/0`.
- Both numbers show, also when the right number is 0: `2/0 💤`, not `2 💤`. Each count has one shape, thus a new mark does not move the counts after it.
- A handled session counts on the right side of its state. A change of state removes the mark, thus the session then counts on the left side of its new state.
- Only a `needs-you` or `idle` session takes a mark. Thus the right number of `agents`, `shells`, and `working` is always 0, but it shows, because each count has one shape.
- At 70 columns, the whole title line shows when each number has 1 digit.

| State | Marker | Code point |
| --- | --- | --- |
| `needs-you` | ❗ | U+2757 HEAVY EXCLAMATION MARK SYMBOL |
| `idle` | 💤 | U+1F4A4 SLEEPING SYMBOL |
| `agents` | 🤖 | U+1F916 ROBOT FACE |
| `shells` | 🐚 | U+1F41A SPIRAL SHELL |
| `working` | 🏃 | U+1F3C3 RUNNER |

Each marker is one code point of East Asian Width `W`, with no variation selector and no joiner. Thus all terminals agree that each marker is 2 columns wide. `ha` uses the same rule, and it also uses 🏃 for a running job and 💤 for a stale repository. A marker has no foreground color, because a terminal draws an emoji in its own colors.

`needs-you` uses ❗ and not ‼️. `‼` (U+203C) has East Asian Width `N` and text presentation by default. It draws as an emoji only with the variation selector U+FE0F. Zellij removes each zero-width code point from its grid, and U+FE0F is one of them. In a Zellij pane, `‼️` thus draws as a text glyph 1 column wide, and the screen does not agree with the width that cctop uses for the title. ❗ is the nearest emoji that obeys the rule.

### The handled mark

Press `h` to mark the selected session as handled. The row goes to the bottom of the rows in its state, and its state shows ` - handled`. The latest mark goes to the bottom of its state. The cursor then selects the next `needs-you` or `idle` row that has no mark, thus you can press `h` again to mark that session. When no such row is below it, the cursor stays on the marked row.

Only a session in `needs-you` or `idle` takes a mark. A mark tells that you saw the session and have nothing to do for it now, and only these two states wait for you. When you press `h` on a session in `working`, `agents`, or `shells`, cctop puts no mark on it. The row and the cursor do not move, and the status line shows `Cannot mark <name> as handled: only needs-you and idle sessions take a mark`.

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

The last line is the status line. It shows the result of the last focus request or the last notification error. Without a message, it shows the keys. A move key (`j`, `k`, Up, or Down) clears the message.

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

## The jump to a session that needs you

When a session changes to `needs-you`, cctop moves the selection to its row at once. A count in the title line is easy to miss, but you see the selection move. The jump only moves the selection. It does not focus the session. Press Enter to focus it.

- A new session that starts in `needs-you` gets the jump too.
- `--no-notify` does not stop the jump.
- A session that is already in `needs-you` when cctop starts gets no jump. The first row is selected at start, and a `needs-you` row sorts first, thus that row is selected already.
- When more than one session changes to `needs-you` in one event, cctop selects the one that sorts first.

### The hold

A key press holds the jump for 2 seconds. When you pressed a key less than 2 seconds before the change, the selection does not move yet. Each key press starts the hold again. A key release does not. The first tick after the end of the hold moves the selection. cctop ticks each second, thus the jump comes at most 3 seconds after your last key press.

The hold prevents two errors:

- A jump between a move key and Enter focuses a different session.
- A jump just before `h` puts a handled mark on the new `needs-you` session, and that mark hides the session that the jump must show.

A held jump stops when its session leaves `needs-you`, ends, or gets a handled mark before the hold ends. When more than one session changes to `needs-you` during the hold, the end of the hold selects the one that sorts first of those that are still in `needs-you` with no mark.

## The keys

| Key | Action |
| --- | --- |
| `j`, Down | Select the next row. |
| `k`, Up | Select the previous row. |
| Enter | Focus the selected session. |
| `n` | Select the next `needs-you` session that has no handled mark, and focus it. |
| `h`, `H` | Mark the selected `needs-you` or `idle` session as handled, or remove its mark. A session in a different state takes no mark. |
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
