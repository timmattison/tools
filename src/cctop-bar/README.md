# cctop-bar (Claude Code top in the menu bar)

cctop-bar puts the cctop board in the menu bar of macOS. It runs the same board as the terminal [`cctop`](../cctop/README.md), in one process, with no terminal. You see which sessions need you, and you do not look for the cctop terminal.

- **The status item** shows a mini-map: one tile for each Claude Code session, in the color of its state.
- **The menu** shows the counts, the sessions that need you, and a switch for the notifications.
- **A global hotkey** opens the normal cctop screen in a floating panel, over any app.

cctop-bar runs on macOS only.

## The status item

The status item shows one tile for each session on the board. The tiles are in the order of the cctop rows, thus the `needs-you` tiles are at the left. Each tile has the color of its state, as in the cctop rows and in the `claude-tab-bar` Zellij plugin:

| State | Color |
| --- | --- |
| `needs-you` | amber `#fab005` |
| `idle` | red `#e03131` |
| `agents` | white `#ffffff` |
| `shells` | cyan `#00cccc` |
| `working` | green `#2f9e44` |

- **A handled session** is an outline tile in the color of its state. See [the handled mark](../cctop/README.md#the-handled-mark).
- **A frozen session** is a filled blue tile (`#4dabf7`). A tile is too small to show the state color and the word `frozen` together.
- **The number of sessions that need you** shows in black on an amber field, at the left of the tiles. It counts the live `needs-you` sessions with no handled mark. It is the left number of the `needs-you` count of the cctop title line. When no session needs you, the field goes away.
- **At most 12 tiles** show. When the board holds more sessions, `+N` follows the last tile. The cut removes the least urgent tiles, because the tiles are in the order of the rows.
- **With no session**, the status item shows one empty outline tile, thus you can still click it.

In a light menu bar, a white tile gets a black edge, so that it stays visible. The `+N` mark is black in a light menu bar and white in a dark one. The status item draws again when the board changes, not on each tick.

## The menu

A click on the status item opens the menu. The menu builds its items when it opens.

1. The counts of the cctop title line, for example `14 sessions · 1/2 ❗ · 3/7 💤 · 1/0 🤖 · 0/0 🐚 · 0/0 🏃 · 0 🧊`.
2. One item for each session that needs you: its name or its directory, and its time in the state. A click focuses that session through the same two steps as Enter in cctop. See [the focus](../cctop/README.md#the-focus).
3. **Show cctop**, with the hotkey, for example `Show cctop  ⌃⌥⇧⌘⌫`. It opens the panel.
4. **Notifications**, a switch. It turns the notifications on and off while cctop-bar runs.
5. The last error, when there is one: a failed hotkey registration, a failed focus, or a failed notification.
6. **Quit cctop-bar**. Only this item stops the program.

## The panel

The hotkey or **Show cctop** opens a floating panel in the center of the active screen. The panel shows the normal cctop screen: the same rows, the same change log, the same status line, and the same keys. cctop-bar paints the screen itself, thus the panel needs no terminal.

- **The active screen** is the screen that holds the most of the front window of the front app. With no such window, it is the screen under the mouse pointer.
- **The size** is 2/3 of the screen, without the menu bar and the Dock, in whole cells. It is at least 80 × 24 cells when the screen has room for that.
- **The panel shows over a full-screen app** on the current Space.
- **Each show selects the first row.** Thus the hotkey and then Enter focus the most urgent session.
- **The panel is dark at all times**, because the cctop state colors are for a dark terminal.
- **The app that you worked in keeps the keyboard focus** when the panel hides. cctop-bar never comes to the front.

### The keys of the panel

The keys are the keys of the terminal cctop. See [the keys](../cctop/README.md#the-keys).

| Key | Action |
| --- | --- |
| `j`, Down | Move the selection down one row. |
| `k`, Up | Move the selection up one row. |
| Page Up, Page Down | Move the selection one page. |
| Home, End | Select the first or the last row. |
| Enter | Focus the selected session, and hide the panel. |
| `n` | Focus the next session that needs you. |
| `h` | Put a handled mark on the selected session, or remove it. |
| Esc, `q`, Control-C | Hide the panel. They do not stop cctop-bar. |
| The hotkey | Hide the panel. |

A click outside the panel also hides it. The panel takes no mouse clicks, as the terminal cctop does.

CAUTION: OPTION + SHIFT + COMMAND + DELETE, WITH NO CONTROL, IS THE FINDER SHORTCUT THAT EMPTIES THE TRASH WITH NO CONFIRMATION. IF YOUR PRESS OF THE DEFAULT HOTKEY MISSES CONTROL WHILE FINDER IS IN FRONT, FINDER EMPTIES THE TRASH. A HYPER KEY THAT GIVES ALL FOUR MODIFIERS WITH ONE KEY REMOVES THIS RISK. ELSE, SET A DIFFERENT HOTKEY WITH `--hotkey`.

## The hotkey

The default hotkey is Control + Option + Shift + Command + Delete: `ctrl+alt+shift+cmd+backspace`, which the menu shows as `⌃⌥⇧⌘⌫`. Delete is the key labeled "delete" on a Mac keyboard, and Backspace on other keyboards.

Give a different hotkey with `--hotkey`. Join the parts with `+`. Case and spaces do not matter.

- The modifiers: `ctrl` or `control`, `alt`, `opt`, or `option`, `shift`, and `cmd` or `command`. A hotkey needs at least one modifier, because a global hotkey with no modifier takes a key from every app.
- The keys: one letter `a`-`z`, one digit `0`-`9`, `f1` thru `f20`, `backspace` or `delete`, `space`, `escape`, `return`, `tab`, `up`, `down`, `left`, and `right`.

```bash
cctop-bar --hotkey ctrl+alt+cmd+k     # Control + Option + Command + K
cctop-bar --hotkey 'cmd + shift + f12'
cctop-bar --no-hotkey                 # no global hotkey; use Show cctop in the menu
```

cctop-bar registers the hotkey through Carbon `RegisterEventHotKey`. Thus it needs no Accessibility permission and no Input Monitoring permission. When a different app holds the same hotkey, the registration fails. The menu then shows the error, and **Show cctop** still opens the panel.

## Start and stop

Each user has one status item. A run of `cctop-bar` first looks for it. When it runs, the run stops it, starts nothing, and exits with status 0. Thus a second run of the same command closes the status item that the first run opened.

```bash
cctop-bar       # cctop-bar: the status item runs as PID 4242. Run cctop-bar again to stop it.
cctop-bar       # cctop-bar: stopped the status item that ran as PID 4242.
cctop-bar -f    # run the status item in this process, which keeps the terminal
```

- **A new status item runs in the background.** The command returns to the shell when the status item is up. `-f` keeps it in the foreground, for a LaunchAgent.
- **Two starts at the same moment give one status item.** The status item holds a lock on `~/Library/Application Support/cctop-bar/cctop-bar.lock`. The kernel releases the lock when the status item ends, also after a crash or `kill -9`.
- **A background status item has no terminal.** The close of the terminal or Control-C does not stop it.

The flags:

- `--no-notify`: Send no notification when a session changes to `needs-you`. The **Notifications** switch can turn them on again.
- `--hotkey <CHORD>`: Set the global hotkey. See [The hotkey](#the-hotkey).
- `--no-hotkey`: Register no global hotkey.
- `-f`, `--foreground`: Run the status item in the process of the command.
- `-V`, `--version`: Print the version, the git hash, and whether the build was clean.
- `-h`, `--help`: Print the usage.

## The notification

cctop-bar sends the same macOS notification as cctop when a session changes to `needs-you`. When the terminal cctop runs too, you get one notification, not two. Before each notification, the terminal cctop asks the kernel for the holder of the lock of cctop-bar. While cctop-bar holds it, cctop sends nothing. The jump of the selection in cctop does not change.

## Start at login

cctop-bar has no "start at login" item. A LaunchAgent starts it instead. Write this file to `~/Library/LaunchAgents/com.timmattison.cctop-bar.plist`, and replace `YOU` with your account name. launchd does not expand `~`, thus each path must be complete.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.timmattison.cctop-bar</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/YOU/.cargo/bin/cctop-bar</string>
        <string>--foreground</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
</dict>
</plist>
```

Then load it:

```bash
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.timmattison.cctop-bar.plist
```

- **`PATH` is necessary.** The focus runs `zellij`, `ps`, and `lsof` by name, and launchd gives a short `PATH`. Put the directory of `zellij` in it.
- **Add `CLAUDE_CONFIG_DIR`** to `EnvironmentVariables` when your shell sets it. Else cctop-bar reads `~/.claude`.
- **`--foreground` is necessary.** Without it, cctop-bar starts a copy and exits with status 0. launchd then watches no process, and it does not start the status item again after a crash.
- **Quit cctop-bar exits with status 0**, thus launchd does not start it again. A run of `cctop-bar` in a terminal stops it the same way. To stop it until the next login, run `launchctl bootout gui/$(id -u)/com.timmattison.cctop-bar`.

## Limits

- The panel takes no mouse clicks.
- The tiles do not pulse. The notification and the amber field give the alert, and an animation in the menu bar costs CPU time on each frame.
- cctop-bar runs on macOS only. On a different OS, it stops with the message `cctop-bar supports macOS only.`

## Install

```bash
cargo install --git https://github.com/timmattison/tools cctop-bar
```
