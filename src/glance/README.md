# glance (CPU and memory at a glance)

Show two small charts in the macOS menu bar: the share of the CPU time that the system processes and the user processes use, and the memory pressure. In local mode, the status item reads each value from the kernel in one call, starts no child process, and draws its charts into a pixel buffer itself. Thus the monitor adds almost nothing to the load that it shows.

`glance --host <destination>` shows the same charts for a second Mac, through one SSH connection that stays open. See [Show a second Mac](#show-a-second-mac).

`glance` starts the status item in the background and returns to the shell. A second run of the same command stops it. `glance -f` keeps the status item in the foreground. See [Start and stop](#start-and-stop).

## What it shows

- **The left chart is the CPU.** Each column is one sample. The red part at the bottom is the system share, and the blue part above it is the user share. A nice process counts as a user process. These are the colors of the CPU chart in Activity Monitor.
- **The right chart is the memory pressure.** The height of each column is the memory in use as a percentage, and its color is the pressure level of that sample: green for normal, orange for warn, and red for critical. A level that `glance` does not know is gray, never green.
- **Each chart is as high as the menu bar**, inside a thin frame, and the two charts share one border. Thus a low load is easy to see: on a menu bar of 22 points, each chart has 20 rows, and a CPU share of 10% fills 2 of them.
- **The charts show the last 32 samples.** At the default interval, that is the last 32 seconds. The newest column is at the right edge, so a short spike stays visible after it ends.
- **The name of the Mac is in small text at the top of the charts.** In local mode, it is the short name of the Mac: the name before the first `.`, as `hostname -s` gives it (`M4-Pro-Mac-Mini`, not `M4-Pro-Mac-Mini.local`). In remote mode, it is the destination of `--host` as you typed it, or, with `--resolve`, the name that the second Mac reports after the first line arrives (see [Show a second Mac](#show-a-second-mac)). The name goes across both charts, over the columns. Each pixel of the name is black or white, whichever contrasts more with the pixel behind it. Thus the name is black where it crosses a column, and it has the text color of the menu bar where the chart is empty. The charts are wide enough for a name such as `M4-Pro-Mac-Mini`. A longer name ends in an ellipsis, and the status item does not grow. If the menu bar is too thin for the name, the name goes away.
- **A click on the charts opens a menu** with the host (`Host: <name>`, the full name, or `Host: <destination> (ssh)` in remote mode, or `Host: <name> (ssh <destination>)` with `--resolve` when the reported name differs from the destination), the current values (user %, system %, idle %, the pressure level, and the percentage in use), and a **Quit glance** item.
- `glance` has no Dock icon and no window.

## Basic Usage

```bash
glance                         # start the status item in the background; a second run stops it
glance -f                      # start it in the foreground, where it keeps the terminal
glance --interval 2            # one sample every 2 seconds
glance --sample                # print one reading as JSON, then stop
glance --stream --count 3      # print 3 JSON lines, one each interval, then stop
glance --host m1-ts            # show the charts of the Mac that ssh calls m1-ts; a second run stops only m1-ts
glance --host m1-ts --resolve  # the same, with the name that the second Mac reports
```

## Options

- `--interval <SECONDS>`: The time between two samples, in seconds. A decimal number is permitted. Defaults to `1`. A value below `0.25` is a usage error. A longer interval uses less CPU time and shows a longer history.
- `--sample`: Take two readings one interval apart, print one JSON object, and stop. This mode needs no window server.
- `--stream`: Print one JSON line for each interval until the standard output closes. A closed pipe stops the stream with status 0. This mode needs no window server, thus it runs in an SSH session. It cannot go with `--sample`.
- `--count <N>`: Stop the stream after `N` lines. It needs `--stream`. With no `--count`, the stream has no limit.
- `--host <DESTINATION>`: Show the charts of a second Mac. `DESTINATION` is anything that `ssh` takes: an alias of `~/.ssh/config`, `user@host`, or an address. A destination that is empty or that starts with `-` is a usage error. It cannot go with `--stream` or `--sample`.
- `--remote-binary <PATH>`: The path of `glance` on the second Mac. Defaults to `~/.cargo/bin/glance`. It needs `--host`.
- `--resolve`: Show the name that the second Mac reports (`hostname -s` on that Mac), not the destination as you typed it. Before the first line arrives, the status item shows the destination. It needs `--host`.
- `-f`, `--foreground`: Run the status item in the process of the run, which keeps the terminal until the status item stops. Give it to a LaunchAgent (see [Start at login](#start-at-login)). With no `-f`, `glance` starts the status item in a copy that has no terminal, and returns to the shell when the status item is up. It cannot go with `--stream` or `--sample`.
- `-V`, `--version`: Print the version, the git hash, and whether the build was clean.
- `-h`, `--help`: Print the usage.

## Start and stop

Each Mac that `glance` shows has one status item: this Mac, and each destination of `--host`. A run of `glance` first looks for the status item of its own Mac. When that status item runs, the run stops it, starts nothing, and exits with status 0. Thus a second run of the same command closes the status item that the first run opened.

```bash
glance                    # glance: the status item runs as PID 4242. Run glance again to stop it.
glance --host m1-ts       # glance: the status item of the host m1-ts runs as PID 4250. Run glance again with the same --host to stop it.
glance --host m1-ts       # glance: stopped the status item of the host m1-ts that ran as PID 4250.
glance                    # glance: stopped the status item that ran as PID 4242.
```

- **A run stops only the status item of its own Mac.** A plain run stops the local status item, never a status item of `--host`. A run with `--host m1-ts` stops only the status item of `m1-ts`.
- **The Mac is the destination as you typed it.** `m1-ts` and `tim@m1-ts` are two different status items, even when they log in to the same Mac.
- **A new status item runs in the background.** The start returns to the shell when the status item is up. `-f` keeps a new status item in the foreground.
- **The default changed.** Before, a plain run kept the terminal, and `-b` started the status item in the background. Now a plain run starts it in the background. `-b` and `--background` are gone, and each one is a usage error. A LaunchAgent that runs a plain `glance` must get `--foreground` (see [Start at login](#start-at-login)).
- **`--foreground` does not change which status item a run stops.** A run with or without `-f` stops the status item of its Mac. `-f` only decides where a new status item runs.
- **`--resolve` does not change which status item a run stops.** `glance --host m1-ts` stops the status item that `glance --host m1-ts --resolve` started, and the reverse is also true.
- **A background status item has no terminal.** It runs in a new session, thus the close of the terminal or Control-C does not stop it. Until the status item is up, the start shows what the copy writes to its standard error. After that, the standard output and the standard error of the status item go to `/dev/null`. The start returns when the status item is up, or it prints why the status item did not start and exits with status 1.
- **A foreground status item (`-f`) keeps the terminal** until **Quit glance**, Control-C, or a second run. It prints the same line as a background start when it is up.
- **`--sample` and `--stream` never stop a status item.**
- **A stop is a quit.** The run sends `SIGTERM`, and the status item exits with status 0 at once, as **Quit glance** does. Thus a LaunchAgent does not start it again (see [Start at login](#start-at-login)). In remote mode, the `ssh` child then stops on its next write, as after `kill -9` (see [Show a second Mac](#show-a-second-mac)).
- **A lock file marks each status item** in `~/Library/Application Support/glance/`: `local.lock`, and `host-<destination>.lock` for each destination. The name writes each byte of the destination as `%` and two hex digits, unless the byte is a lowercase letter, a digit, `.`, `-`, `_`, or `@`. The default file system of macOS folds case, so `Alpha` and `alpha` get two files. The kernel releases the lock when its status item ends, also after a crash or `kill -9`. Thus a lock file that stays behind stops nothing and blocks nothing. The files are not in `$TMPDIR`, because macOS deletes old files there, and the lock of a status item that runs for days must stay.

## The `--sample` and `--stream` output

`--sample` prints one JSON object, and `--stream` prints one for each line. The keys are `host` (the short host name of the Mac that read the kernel), `user`, `system`, and `idle` (fractions whose sum is 1), `pressure_level` (`normal`, `warn`, `critical`, or `unknown`), `pressure_level_raw` (the value of the kernel), and `pressure_percent` (the memory in use).

```json
{"host":"M4-Pro-Mac-Mini","user":0.08,"system":0.05,"idle":0.87,"pressure_level":"normal","pressure_level_raw":1,"pressure_percent":53}
```

In `--stream`, a failed read gives the line `{"host":"<name>","error":"<reason>"}`, and the stream continues.

## Show a second Mac

`glance --host <destination>` runs on the Mac at your desk. It starts one `ssh` child, and that child runs `glance --stream` on the second Mac. The connection stays open while the status item runs, thus each sample costs one line of text, not one SSH handshake. The charts, the colors, and the menu lines are the same as in local mode.

1. Install `glance` on the second Mac: `cargo install --git https://github.com/timmattison/tools glance`.
2. Make sure that `ssh <destination>` logs in with a key and asks for nothing. `glance` starts `ssh` with `BatchMode=yes`, thus a refused key gives an error line in the menu, never a password prompt.
3. Start `glance --host <destination>` on the Mac at your desk. Run a second, local `glance` next to it to see both Macs.

**The label shows the destination as you typed it**, for the full life of the status item. That is the name that you know the Mac by. `tim@m1-ts` stays `tim@m1-ts`, and `studio.example.com` is not cut at the first `.`. A destination that is too wide for the label ends in an ellipsis. The first line of the menu is `Host: <destination> (ssh)`.

**This default changed.** Before, the label showed the name that the second Mac reports. To get that label, add `--resolve`. A wrong alias then cannot show a false name. Before the first line arrives, the label shows the destination. After that, it shows the name that the newest line reports, which is `hostname -s` on the second Mac. When that name is different from the destination, the first line of the menu is `Host: <name> (ssh <destination>)`.

**The default remote binary is the full path `~/.cargo/bin/glance`**, where `cargo install` puts it. `sshd` runs the command through the login shell of the remote user with `-c`. zsh then reads `~/.zshenv`, but not `~/.zprofile` or `~/.zshrc`, thus a bare `glance` is often not on `PATH` there. For a different place, give `--remote-binary`:

```bash
glance --host m1-ts --remote-binary '~/bin/glance'      # ~/ is the home directory of the remote user
glance --host m1-ts --remote-binary /opt/tools/glance   # an absolute path
glance --host m1-ts --remote-binary glance              # a bare name: the remote PATH finds it
```

Put quotes around a value that starts with `~/`. Without the quotes, your local shell expands `~` to your local home directory before `glance` sees it. `glance` keeps a leading `~/` outside the quotes of the remote command, so that the remote shell expands it. It quotes the rest of the path, so a space or a quote in the path stays in one word.

**When the connection stops**, the menu shows why, for example `Cannot read m1-ts: zsh:1: no such file or directory: /Users/tim/.cargo/bin/glance` or `Cannot read m1-ts: ssh: connect to host m1-ts port 22: Connection refused`. The reason is the last line that `ssh` wrote to its standard error, or its exit status. `glance` then connects again on its own: 5 seconds after the first stop, and twice as long after each failure, up to 60 seconds. The first sample of a connection sets the delay back to 5 seconds. The charts keep their history across a new connection.

**No process stays behind.** **Quit glance** stops the `ssh` child before `glance` stops. When the connection closes, the remote `glance --stream` fails on its next write and stops. If the local `glance` stops with no clean-up (for example, `kill -9`), its end of the pipe closes, and `ssh` and then the remote `glance --stream` stop on their next write. If the network goes down, `ssh` finds the loss within about 45 seconds (`ServerAliveInterval=15`, `ServerAliveCountMax=2`), and the menu shows the error line.

A remote `glance` that does not know `--stream` gives a usage error, and that error shows in the menu. Install the same version on both Macs.

## Start at login

`glance` has no "start at login" item. A LaunchAgent starts it instead. Write this file to `~/Library/LaunchAgents/com.timmattison.glance.plist`, and replace `YOU` with your account name. launchd does not expand `~`, thus the path must be complete.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.timmattison.glance</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/YOU/.cargo/bin/glance</string>
        <string>--foreground</string>
    </array>
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
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.timmattison.glance.plist
```

`RunAtLoad` starts `glance` at each login. `KeepAlive` with `SuccessfulExit` set to `false` starts it again after a crash, but not after **Quit glance**, which exits with status 0. `--foreground` is necessary. Without it, `glance` starts a copy in a new session and exits with status 0. launchd then watches no process, and it does not start the status item again after a crash. A run of `glance` in a terminal stops it the same way, with status 0. To stop it and keep it stopped until the next login, run `launchctl bootout gui/$(id -u)/com.timmattison.glance`.

For the status item of a second Mac, write a second file with its own label, for example `~/Library/LaunchAgents/com.timmattison.glance.m1-ts.plist`. Replace `m1-ts` with your SSH destination:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.timmattison.glance.m1-ts</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/YOU/.cargo/bin/glance</string>
        <string>--foreground</string>
        <string>--host</string>
        <string>m1-ts</string>
    </array>
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

Load it with `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.timmattison.glance.m1-ts.plist`. launchd starts `glance` with its own `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`), thus `glance` runs `/usr/bin/ssh`, which reads `~/.ssh/config` as in a terminal. launchd does not give `glance` the variables that your shell sets. If `ssh` needs one of them, the menu shows the error of `ssh`.

## Notes

- **The sources.** The CPU ticks come from `host_statistics(HOST_CPU_LOAD_INFO)`. The pressure level is `kern.memorystatus_vm_pressure_level` (1 is normal, 2 is warn, 4 is critical). The percentage in use is `100 - kern.memorystatus_level`, and `kern.memorystatus_level` is the free percentage that `memory_pressure` prints.
- **The height of the memory chart can be different from Activity Monitor.** Activity Monitor computes its graph with a formula that Apple does not document. The color is the level of the kernel, which is the same value that Activity Monitor uses.
- **Activity Monitor calls the warn level yellow.** `glance` shows it orange.
- **Weeks of uptime are safe.** The tick counters are 32 bits wide and wrap after about 31 days on a Mac with 16 cores. `glance` subtracts them with wrapping arithmetic, so a wrap does not show a false spike.
- **Child processes and threads.** In local mode, the status item starts no child process and no thread. A start with no `--foreground` starts one copy of `glance`, which runs the status item, and one thread that reads the standard error of the copy. The start then ends. In remote mode, it starts one `ssh` child at a time and two threads that read its pipes. The remote Mac sends samples, not raw tick counters, thus one sample line is the same object that `--sample` prints.
- **macOS only.** On a different platform, `glance` prints `glance supports macOS only.` and exits with status 1. The second Mac of `--host` must be a Mac too.

## Install

```bash
cargo install --git https://github.com/timmattison/tools glance
```
