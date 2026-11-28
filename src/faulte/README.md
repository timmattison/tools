# faulte (fault rate)

- Ranks the processes of a Mac by page faults per second over an interval, and gives a total
  line for the Claude Code sessions. The header adds the swap traffic, the size of the
  compressor, and the swap in use over the same sample. `faulte check` says in seconds if the
  system time is high, and if memory churn or process churn occurs at the same time.
  `faulte kill` stops the old idle Claude Code sessions after a question. `faulte tui` is a live
  full-screen view with a history. It supports macOS only. See below for the full entry.
- To install: `cargo install --git https://github.com/timmattison/tools faulte`

Rank every process of this Mac by the page faults that it makes per second, and say what the memory of the Mac did over the same sample. `faulte kill` stops the old idle Claude Code sessions, after it shows the plan and asks.

On 2026-09-17 and 2026-09-18, this Mac spent 78% of its time in the kernel. 34 MB of memory was unused, the compressor held 32 GB, and 20 seconds of that state counted 46,564 swap-ins. The kernel did almost nothing but move pages between memory, the compressor and the swap file.

213 Claude Code processes made 90% of all page faults in that sample. No single one of them was the cause: the median session made 23,000 faults in the 20 seconds, and the `node` and `vitest` processes of a running pre-commit hook made 0.4% together. The answer was the sum of 213 small rates, and no row of a table says a sum. That is why `faulte` prints one total line for the Claude Code sessions, above the table. 86 of the 213 sessions were older than 7 days, and that is why `faulte kill` exists.

## Is this the problem now?

`faulte check` gives one verdict in 5 seconds. It reads the CPU counters and the compressor counters of the kernel, and it starts one short process to learn the newest process number. It runs no `top` and no `ps`, thus it works when one `ps` takes 13 seconds.

| Verdict | Meaning | Exit code |
| --- | --- | --- |
| `OK` | System time is below 30% of all CPU time. | 0 |
| `MEMORY CHURN` | System time is at least 30%, and the compressor compresses and decompresses at least 50,000 pages each second. | 1 |
| `PROCESS CHURN` | System time is at least 30%, the compressor does not reach its limit in both directions, and at least 150 processes start each second. | 1 |
| `OTHER SYSTEM TIME` | System time is at least 30%, the compressor does not reach its limit in both directions, and fewer than 150 processes start each second. | 1 |

An error gives the exit code 2. `faulte check --watch` prints one line for each interval until you stop it.

On 2026-11-08, at 0% idle, this Mac showed two states. With 32 GB in the compressor, system time was 52% to 66%, and it followed the rate of compressions (correlation 0.88, over 12 samples of 5 seconds). It did not follow the load average (0.32). Each second, 193 to 444 new processes started. With 16 GB in the compressor and at most 51,323 compressions each second, system time was still 42% to 56%, with 257 to 490 new processes each second. The limits come from the bad samples of that day. No sample of a quiet period set them.

## Basic Usage

```bash
faulte
faulte --interval 20s --limit 40
faulte check
faulte check --watch --interval 10s
faulte kill
faulte kill --older-than 3d --idle-for 1h --max 5
faulte tui
faulte tui --interval 2s
```

## Options

- `--interval <DURATION>`: The length of the sample, for example `5s`, `10m`, `2h`, or `7d`. A bare number is a number of seconds. The ranking, `faulte kill`, and each sample of `faulte tui` count the page faults of each process over this time. `faulte check` reads the kernel counters at the start and at the end of this time. Defaults to `5s`. Zero is refused, because a sample of no time measures nothing.
- `--limit <N>`: The number of rows that the table shows. Defaults to 25. One line under the table counts the rows that the limit hid, and gives their share of all faults, because the limit hides the small rates and the small rates were the load. The flag belongs to the ranking alone: `faulte kill` shows every candidate of its plan, and refuses `--limit` after its name. Before the name, as in `faulte --limit 3 kill`, the flag is a flag of `faulte` itself, and `faulte kill` takes it and ignores it. That spelling keeps the exit code that it had before `faulte tui`.
- `-V`, `--version`: Print the version, the git hash, and whether the build was clean.
- `-h`, `--help`: Print the usage.

`faulte kill` samples the page faults as well, so it takes `--interval`, and three flags of its own:

- `--older-than <DURATION>`: Select a session only when it is older than this time. Defaults to `7d`.
- `--idle-for <DURATION>`: Select a session only when it became idle more than this time ago. Defaults to `10m`.
- `--max <N>`: Stop no more than N sessions. `faulte` keeps the N oldest, and the plan counts the rest.

## What it prints

```
$ COLUMNS=150 faulte --interval 2s --limit 10
1,744 processes over a 2.0 s window (interval 2s) — 9,158 faults, 4,579/s
swap: 4 in, 0 out in 3.6 s · compressor 15.0 GB · swap in use 8.4 GB of 10.0 GB
Claude: 37 sessions made 0.2% of all faults (9 of another account)
skipped: 1 exited before faulte read it (19.6% of the faults), 77 zombies, 2 were not in the top sample

┌───────┬───────────────┬──────────┬───────┬────────┬─────────┬────────────────────────────────────────────────────────┬─────────┬───────┬───────────┐
│ PID   ┆ OWNER         ┆ FAULTS/S ┆ SHARE ┆ RSS    ┆ AGE     ┆ COMMAND                                                ┆ SESSION ┆ STATE ┆ DIRECTORY │
╞═══════╪═══════════════╪══════════╪═══════╪════════╪═════════╪════════════════════════════════════════════════════════╪═════════╪═══════╪═══════════╡
│ 33899 ┆ root          ┆ 1,271    ┆ 27.8% ┆ 50 MB  ┆ 7h 35m  ┆ /System/Library/PrivateFrameworks/XprotectFramework.fr ┆         ┆       ┆           │
│       ┆               ┆          ┆       ┆        ┆         ┆ amework/Versions/A/XPCServices/XprotectService.xpc/Con ┆         ┆       ┆           │
│       ┆               ┆          ┆       ┆        ┆         ┆ tents/MacOS/…                                          ┆         ┆       ┆           │
├╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ 39450 ┆ timmattison   ┆ 689      ┆ 15.0% ┆ 19 MB  ┆ 3s      ┆ /Volumes/SamsungSSDs/code/tools-worktrees/issue-510/ta ┆         ┆       ┆           │
│       ┆               ┆          ┆       ┆        ┆         ┆ rget/debug/deps/repository-e0e5ec02b6d5fc96            ┆         ┆       ┆           │
├╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ 40242 ┆ scyloswork    ┆ 573      ┆ 12.5% ┆ 29 MB  ┆ 5h 3m   ┆ gsw                                                    ┆         ┆       ┆           │
├╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ 66335 ┆ timmattison   ┆ 296      ┆ 6.5%  ┆ 8 MB   ┆ 5h 52m  ┆ mosh-server new -c 256 -s -l LANG=en_US.UTF-8 -l       ┆         ┆       ┆           │
│       ┆               ┆          ┆       ┆        ┆         ┆ MOSH_CLIENT_IMAGES=kitty,sixel,iterm2                  ┆         ┆       ┆           │
├╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ 659   ┆ _windowserver ┆ 129      ┆ 2.8%  ┆ 191 MB ┆ 24d 0h  ┆ /System/Library/PrivateFrameworks/SkyLight.framework/R ┆         ┆       ┆           │
│       ┆               ┆          ┆       ┆        ┆         ┆ esources/WindowServer -daemon                          ┆         ┆       ┆           │
├╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ 908   ┆ root          ┆ 88       ┆ 1.9%  ┆ 42 MB  ┆ 24d 0h  ┆ /opt/homebrew/opt/tailscale/bin/tailscaled             ┆         ┆       ┆           │
├╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ 26222 ┆ scyloswork    ┆ 54       ┆ 1.2%  ┆ 58 MB  ┆ 4h 12m  ┆ /Applications/Stats.app/Contents/MacOS/Stats           ┆         ┆       ┆           │
├╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ 68603 ┆ scyloswork    ┆ 47       ┆ 1.0%  ┆ 43 MB  ┆ 8h 3m   ┆ /Applications/Wispr Flow.app/Contents/Resources/swift- ┆         ┆       ┆           │
│       ┆               ┆          ┆       ┆        ┆         ┆ helper-app-dist/Wispr Flow.app/Contents/MacOS/Wispr    ┆         ┆       ┆           │
│       ┆               ┆          ┆       ┆        ┆         ┆ Flow                                                   ┆         ┆       ┆           │
├╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ 88885 ┆ _coreaudiod   ┆ 44       ┆ 1.0%  ┆ 43 MB  ┆ 10d 19h ┆ /usr/sbin/coreaudiod                                   ┆         ┆       ┆           │
├╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ 7205  ┆ timmattison   ┆ 34       ┆ 0.8%  ┆ 11 MB  ┆ 3d 0h   ┆ mosh-server new -c 256 -s -l LANG=en_US.UTF-8 -l       ┆         ┆       ┆           │
│       ┆               ┆          ┆       ┆        ┆         ┆ MOSH_CLIENT_IMAGES=kitty,sixel,iterm2                  ┆         ┆       ┆           │
└───────┴───────────────┴──────────┴───────┴────────┴─────────┴────────────────────────────────────────────────────────┴─────────┴───────┴───────────┘
… 1,734 more processes made 10.0% of all faults
```

The first line gives the window that `faulte` measured, beside the interval that the command line asked for. The second line gives the swap traffic, the size of the compressor, and the swap in use. The third line is the total for Claude Code. This Mac was not short of memory at that moment, so the Claude Code share is 0.2%. On 2026-09-18 the same line said 90%.

The last header line counts the processes that got no row, and it is there only when there is one to count. Each process of the Mac is a row of the table or a member of one of those counts: a process that exited between the two reads, a zombie, or a process that `top` did not sample. A tool that drops a process without a word reports a clean Mac that is not clean.

Each row gives the PID, the owner, the faults per second, the share of all faults, the resident memory, the age, and the command. A row of a Claude Code session gives the session ID, the state (`busy`, `waiting`, or `idle` with the time since it became idle), and the directory as well.

## Why a rate, and not a counter

`top -o faults` sorts by the counter since each process started. The oldest process of the Mac ranks first on that counter, whatever it does now. The first analysis of the incident read the resident memory of each process, and then those counters, and neither number named the cause.

So `faulte` runs `top -l 2 -s <interval> -c d` and reads the **second** sample only. The first sample holds the counter since each process started. The second holds the change over the interval, which is the live rate.

`faulte` divides that change by the window that it measured, and not by the interval that the person asked for. Under `-s 2`, the two timestamps of a loaded Mac were 4 seconds apart. An interval is what `top` waits for, and a window is what it delivered, so the header prints both.

## Why the swap traffic comes from the system, and not from the processes

The per-process page-in counter does not count swap. In the 20 seconds that measured 46,564 swap-ins, the per-process page-in counters added up to about 1,000. A tool that ranks by that counter reports a quiet Mac while the swap file takes all of the time.

`faulte` reads `host_statistics64(HOST_VM_INFO64)` before the sample and after it. The swap-ins, the swap-outs and the size of the compressor come from those two reads, and the swap in use comes from `vm.swapusage`. The ranking of the processes comes from the fault counters of `top`. The two questions have two sources, and each source answers the question that it can answer.

## Why it runs `/usr/bin/top` and `/bin/ps`

Two accounts share this Mac, and each one ran about 100 Claude Code processes. A third-party binary reads almost nothing about the processes of another account: `proc_pidinfo(PROC_PIDTASKINFO)` and `proc_pid_rusage` return `EPERM`, so a crate such as `sysinfo` gives no owner, no parent, no start time, and no memory for them.

`/usr/bin/top` and `/bin/ps` both carry the entitlement `com.apple.system-task-ports.read`, and `/bin/ps` is setuid root as well. Each one therefore reads the processes of every account, for an account that holds no privileges at all. No third-party binary can get that entitlement. So `faulte` runs the two commands of the system and parses what they print: the fault counters come from `top`, and the owner, the parent, the memory, the start time and the command line come from `ps`.

Both parsers fail closed. A header row that is not the one `faulte` asked for, a sample with no row, and a row that is not a number all print an error and exit 2. An empty ranking looks the same as a Mac that does nothing, so `faulte` never prints one.

The registry of Claude Code is the one fact that stays out of reach. The `~/.claude/sessions` directory of the other account has the mode `0700`. A Claude Code process of another account therefore gives its fault rate, its memory and its age, and `other account — run with sudo` in place of the session, the state and the directory. Under `sudo`, `faulte` reads the registry in the home directory of each owner, and `faulte kill` can stop the sessions of every account. `faulte` never runs `sudo` itself, the same as `crap`.

## `faulte tui`

`faulte tui` opens a full-screen view that keeps a history. `faulte` takes one sample and exits. The view takes a new sample all the time, and each new ranking replaces the old one. The view keeps every ranking, so a person can go back in time.

```text
faulte tui
faulte tui --interval 2s
```

The table has the same columns as `faulte`, and the header has the same lines. It shows every process that made at least one fault. The `--limit` flag does not apply, because a limit hides the small rates, and the small rates were the load. `faulte tui` refuses `--limit`. Above the table, a timeline shows the fault rate over the whole history. Each cell of the timeline shows the highest rate of its slice, so a spike stays visible on a narrow terminal. A second row shows the swap traffic. The two rows are separate, because faults and swap are two separate questions.

In this section, a **frame** is the ranking and the memory counters of one sample. A **gap** is a time that the view tried to measure and did not measure. A failed read makes a gap with its reason. A gap has its own glyph on the timeline, and it is never the lowest bar. The view continues after a failed read.

The view is one sample behind the clock. A sample is complete when the header of the next sample arrives. A banner states the time of the frame on the screen. When the frame is not the live frame, the banner says so in words, with the clock time and the age of the frame. The age and the session state of each row are those of that frame. The sampler continues while a person reads the past, so the history has no hole.

| Key | Action |
| --- | --- |
| `←` or `h`, `→` or `l` | Move one frame back, or one frame forward |
| `[`, `]` | Move one minute back, or one minute forward |
| `Home` | Move to the oldest frame |
| `End` | Return to the live view |
| `Esc` | Close the detail view. With no detail view open, return to the live view |
| `↑` or `k`, `↓` or `j` | Select the row above, or the row below. In the detail view, show the command of the new process from its first line |
| `PageUp`, `PageDown` | Move the selection by one screen. In the detail view, scroll the command line |
| `w` | Change the span of the table: one frame, 1 m, 5 m, 15 m, the whole history |
| `Enter` | Open the detail view of the selected process |
| `q`, `Ctrl-C` | Quit |

No key uses Shift or another modifier, because a modified arrow key depends on the terminal and on the multiplexer. The keys answer during a sample, because the sampler runs on its own thread. A slow `top` does not freeze the view.

The `w` key ranks the processes over a span that ends at the selected frame, so one table answers "which processes caused the last 5 minutes". A process that exited in the span keeps its row. The rate of a span is the sum of the faults of the process over the frames of the span, divided by the sum of the windows of those frames. A gap adds to neither sum, so the rate counts measured time only. The header of a span states the measured time and the count of gaps. Its skipped line gives the sum of the processes that exited over the frames of the span, and the zombies and the unsampled processes of the newest frame. Those two counts are snapshots, so a sum counts one zombie once for each frame that saw it. The detail view shows the fault rate of one process over the whole history, and its full command line. A reused PID is a different process, so two processes never share one history.

The view marks the rows of `faulte`, `top`, and `ps` that it started. Those rows stay in the totals. No key sends a signal. `faulte kill` keeps its plan and its question.

### The memory budget

The history lives in memory and ends with the process. A Mac that is short of memory must not lose more memory to its own diagnostic tool. The budget is 64 MiB, and it counts the frames and the command text. The count of frames also has a limit, so the list of the frames takes at most an eighth of the budget. At one frame each 5 s, the budget holds about 8 hours. The oldest frames leave first. If the selected frame leaves, the selection moves to the oldest frame, and the banner says so. The 8 hours is an estimate from one Mac. The budget test holds the limit.

A frame keeps one compact row for each process that made at least one fault. On 2026-11-08, one sample of this Mac had 1,415 rows, and 495 of them had at least one fault. The command lines took 238,203 bytes. A history of full values at one frame each 5 s needs about 340 MB of text each hour.

### One `top` child

The view runs one `top -l 0 -s <interval> -c d` child for its whole life. The command `faulte` runs `top -l 2` for each window, and each run pays the full start of `top`. That start made a hole in the record. These measurements are from 2026-11-08, at a load average of 125 to 166:

| Measurement | Result |
| --- | --- |
| One `top -l 2 -s 5` | 25.02 s of wall time for one window of 9 s |
| One `top -l 0 -s 2` through a pipe | First sample after 9.42 s, then one more sample each 4.2 s to 6.9 s |
| One `top -l 0 -s 2` through a pseudo-terminal | First sample after 22.17 s, then one more sample each 4.6 s to 7.0 s |

A pipe is sufficient. The first sample holds the counter since each process started, so the view discards it. Each later sample is one window, and the windows touch. The window of each frame comes from what the samples state, and not from the interval that the person asked for. If the child ends, the sampler starts one new child after one interval, and a gap holds the lost time. When the view ends, the `top` child stops with it.

### The terminal

`faulte tui` needs a terminal on its input and on its output. If one of them is not a terminal, `faulte tui` prints the reason and exits 1, and it starts no `top`. A quit, a panic, `SIGTERM`, and `SIGHUP` each restore the terminal. The view redraws at any size. On a platform other than macOS, it prints `faulte supports macOS only.` and exits 1.

## `faulte kill`

`faulte kill` stops Claude Code sessions, and it never selects another kind of process. `pk` does that job. A session is a candidate when all four of these rules are true:

1. The process is older than `--older-than` (7 days by default).
2. The registry status is `idle`, and the session became idle more than `--idle-for` ago (10 minutes by default). A session whose idle time is unknown fails this rule. A session with no registry record fails it too, because nothing proves that it is idle.
3. No descendant of the session is alive. A background shell or a running tool call makes a session active, whatever its status says.
4. The session is not the `faulte` process and not an ancestor of it. A session that runs `faulte kill` is never a candidate.

A `busy` session and a `waiting` session fail rule 2. `waiting` means that the session waits for an answer from its user, and a stop discards the question.

The plan comes before the question. It lists the candidates oldest first, in the columns of the ranking, and it counts every session that it refused under the first rule that the session failed:

```
$ COLUMNS=150 faulte kill --interval 2s
0 Claude sessions are older than 7d, idle for more than 10m, and have no live descendant

not selected: 1 runs faulte, 26 younger than 7d, 1 not idle
faulte stops nothing.
```

That run found nothing to stop, so it asked nothing. When the plan names a session, `faulte` prints the table of the candidates and then asks on the last line:

```
Stop 19 sessions? [y/N]
```

- Only `y` and `yes` confirm, in any case, after the spaces come off. An empty answer, any other answer, and the end of the input all stop nothing. **No flag skips the question.**
- A run whose input is not a terminal prints the plan, stops nothing, and exits 1. A script, a pipe and a hook answer nothing, and a stop is not reversible.
- `--max N` keeps the N oldest candidates. The plan counts the ones that it held back.

A person takes minutes to answer, so the plan is old by then. Immediately before it signals a session, `faulte` reads the process table and the registry again. The check drops a session whose status changed, a session that started a process since the plan, a PID that is gone, and a PID that another process took. A session that the person started to use again in those minutes survives, and the report names it and the reason.

`SIGTERM` goes first, so that Claude Code closes its transcript. `SIGKILL` goes 30 seconds later, and only to a target that is still the same process. The grace period is long because of the Mac that this tool is for: a Mac that is short of memory is slow to page a process in, and a process handles no signal until it is in memory. After `SIGKILL`, `faulte` waits up to 10 seconds for the session to go, and the report names a session that is still there then as one that did not stop.

For each session that it stopped, `faulte` prints one line:

```
crap 34ffff5a-3324-4038-89bb-d5cc5972cfd0
```

The transcript of a session stays on disk after the stop, so `crap` takes that session up again in the directory that it ran in. Nothing that `faulte kill` stops is lost.

## What it leaves out, and says so

- **A scheduled wake-up.** A session can be idle and hold a scheduled wake-up, for example a `/loop`. The registry records no wake-up, so `faulte` cannot see one. This is why the plan shows every candidate before the question: `faulte` gives the facts that it has, and the person decides.
- **The sessions of another account, without root.** They get a row and a fault rate, and `other account — run with sudo` in place of the session fields. In the plan, they get a block of their own with the `sudo faulte kill` command line that includes them, and `faulte kill` never signals them. A count of unreadable sessions is a better answer than silence about them.
- **A process that the two sources disagree about.** `top` lists no zombie, and `ps` lists no PID 0. A process can also start or stop between the two reads. `faulte` gives PID 0 a row that names `kernel_task`, and it gives every other such process a count in the header. Nothing falls out of the report.
- **A live display, and another platform.** The ranking and `faulte kill` take one sample and exit. `faulte check --watch` adds one line for each interval, and it redraws nothing. `faulte tui` is the live view, and it keeps a history. It supports macOS only, because the fault sampler and the counters of the memory system are those of macOS. On another platform it prints `faulte supports macOS only.` and exits 1.

## Exit codes

- `0`: Success. A question that the person declined is a success as well, because `faulte` did what the answer said. For `faulte check`, the verdict is `OK`, or the reader of a `--watch` left.
- `1`: The platform is not macOS, or `faulte check` found a problem, or `faulte kill` found candidates and its input is not a terminal, or `faulte tui` has no terminal on its input or its output.
- `2`: A source of this Mac failed, or its output failed a parser, or a stop did not do what the plan said that it would do, or `faulte check` could not write its output. `faulte` prints the reason, and it never prints an empty ranking.
