# crap (Claude, Resume Anywhere Please)

- Resume a Claude Code session from wherever you are. Given a session id, `crap` looks the
  session up under `~/.claude/projects`, recovers the directory it originally ran in, `cd`s
  there, and re-launches Claude with `--resume <id>` — through the launcher that you name in
  `CRAP_LAUNCHER` (for example `export CRAP_LAUNCHER=c`), otherwise through plain `claude`. If
  the original directory no longer exists — or exists but can't be entered from your account —
  it tells you and stops,
  pointing you at `crap --here <id>` to fork it where you stand instead; and it refuses to
  resume a session that's already open in another running process (pass `--force` to override)
  so two processes can't corrupt the same session log. With `--here` it brings the session into
  the *current* directory instead, resuming it as a forked (new-id) session so you can carry its
  context into a different working tree.
  After you gave the fork new input, the shell function prints
  `Resume this fork with: crap <new-id>` when Claude exits. Thus you know the id of the fork and
  not only the id of the original. `--expand` forks a session with each compaction undone, thus
  `/rewind` in the fork lists the prompts from before each `/compact`. The original transcript is
  only read. If
  the id belongs to another account on the machine, `crap` finds it automatically — searching your
  own sessions first, then other users' as a self-first fallback — and resumes a private fork of
  it (or target a specific account with `--user <name>`, which fails up front and lists the real
  accounts if you name one that never ran Claude). A project directory it isn't allowed to read is
  skipped rather than fatal, and if the session was hiding in one the miss names the account and
  prints the commands to copy the transcript over — `crap` never runs `sudo` itself.
  `--status <id>`
  reports where a session left off (`waiting-for-user`, `busy`, `awaiting-assistant`, …) without
  resuming — and finds a session under another account the same way a resume does, honoring
  `--user <name>` or falling back self-first, but only ever *reading* it (no copy, no fork); a
  cross-user miss that stepped over an owner-only directory prints the same copy-it-first
  guidance. `--status` with no id lists every session for the current directory (as a table, or
  JSON with `--json`) showing each one's state and start/last times; that per-directory listing is
  inherently your own, so it stays current-user-only. Run `crap --shell-setup` once to install the
  shell function.
- To install: `cargo install --git https://github.com/timmattison/tools crap`

Resume a Claude Code session from any directory. You give `crap` a session id; it finds that session under `~/.claude/projects`, recovers the directory the session originally ran in, changes into it, and re-launches Claude with `--resume <id>` from there.

## Basic Usage

```bash
crap 57570685-2d64-4431-8ab6-c021a12fa1af   # cd into that session's dir and resume it
```

The session id is the name of the `.jsonl` file under `~/.claude/projects/<project>/`. `crap` reads the directory from the session log itself (the sanitized project folder name is lossy), so it always lands in the real original path.

If the session's original directory no longer exists, `crap` prints an error and stops without launching anything.

## Choose the launcher: `CRAP_LAUNCHER`

The `crap` shell function starts Claude through a launcher. Each time it runs, it picks the launcher in this order:

1. `CRAP_LAUNCHER`, when you set it and it is not empty. The value is shell text, and the function evaluates it when it runs. Thus an alias, a shell function, or a command with flags of its own (for example `claude --model opus`) works. Claude gets its arguments after the value.
2. Plain `claude`.

```bash
export CRAP_LAUNCHER=c   # start Claude through your `c` alias
```

`crap` does not guess at a short name such as `c`. Many users give `c` to a command that is not Claude, for example `clear`, `code`, or a `cd` helper. Set `CRAP_LAUNCHER` to use such a name.

A fork always passes `--session-id <new-id>`. Thus a launcher that keeps the session id in its own argv lets a Zellij resurrect resume the fork, and not fork the original session again.

## Resume in the current directory: `--here`

Sometimes you don't want to go back to where a session started — you want to bring its context to where you *are* now (a different worktree, a fresh checkout, a scratch dir):

```bash
crap --here 57570685-2d64-4431-8ab6-c021a12fa1af   # resume it right here
```

Claude resolves `--resume <id>` only against the project folder that matches your current directory, so a plain `claude --resume <id>` from anywhere else fails with *"No conversation found with session ID"*. `crap --here` gets around that: it symlinks the session's transcript into the current directory's project folder so Claude can find it, then resumes it with `--fork-session --session-id <new-id>`. Forking means Claude continues with the **full prior context under a brand-new session id**, so the original transcript is never modified.

The symlink is only needed while Claude reads the transcript at startup. A background watcher removes it the moment the forked session file appears — typically within a second — so it doesn't linger for the whole session, and a final `rm` after the session ends serves as a safety net.

A couple of things to know:

- The replayed history still references the *original* directory's paths. Claude works in your current directory from here on, but the conversation it inherits talks about the old one.
- A live original never blocks `--here`, because the fork only reads the original transcript. Thus `--here` ignores `--force`.

`--here` also accepts a cross-user source. Combine it with [`--user`](#resume-another-users-session-automatic-or---user) — `crap --here <id> --user alice` — to fork another account's session right here in your current directory. Because a transcript in someone else's home can never be found by a `claude --resume` you run yourself, `crap` **copies** it into your own tree rather than symlinking (nothing is ever linked into another user's home), then forks and cleans the copy up the same way it removes the symlink. A same-user `--here` still symlinks exactly as before. This is the escape hatch when the session's original directory is gone or you can't enter it: `--here` ignores that directory entirely.

### The forked session's id

The fork gets a new id, and `crap` chooses that id before Claude starts. When Claude exits, the shell function tells you the id:

```text
Resume this session with:
claude --resume 9f8e7d6c-5b4a-3210-fedc-ba9876543210
Resume this fork with: crap 9f8e7d6c-5b4a-3210-fedc-ba9876543210
```

The first two lines come from Claude, and the last line comes from `crap`. `crap <new-id>` resumes the fork from any directory. Use it, not the id you gave to `crap --here`: that id is the original session, which does not contain the work you did in the fork.

`crap` prints its line only when Claude saved the fork. Claude writes the fork transcript only after your first new input, so if you exit the fork at once, there is no fork to resume, and neither Claude nor `crap` prints a resume line. The line from `crap` is also the one to trust when the session has a custom title (from `/rename`). Claude then prints `claude --resume "<title>"`, but the fork inherits the title, so that command opens a picker that lists both the fork and the original.

Without a second argument, `crap` generates a UUID v4 for the fork. Pass a second argument to choose the id yourself:

```bash
crap --here 57570685-2d64-4431-8ab6-c021a12fa1af 9f8e7d6c-5b4a-3210-fedc-ba9876543210
```

The new id must be a valid UUID, and `crap` refuses it if it already names a session, so the fork can never overwrite an unrelated transcript. A generated id gets the same checks. Choose the id yourself when a script must know the new id before Claude starts.

The cross-user resume (below) forks too, so it gets a generated id and the same line.

After you upgrade `crap`, run `crap --shell-setup` again. The resume line comes from the shell function, and an older function does not print it.

## Resume with the full history: `--expand`

`/compact` does not delete history. It adds a compact boundary and a summary to the end of the transcript, and the messages from before the compaction stay in the file. But a resume loads only the summary and the messages after it. Thus `/rewind` cannot go back to a prompt from before the compaction, and a plain resume cannot get the full history back.

`--expand` resumes a fork of the session with each compaction undone:

```bash
crap --here --expand 57570685-2d64-4431-8ab6-c021a12fa1af   # fork it in the current directory
crap --expand 57570685-2d64-4431-8ab6-c021a12fa1af          # fork it in its original directory
```

In the fork, `/rewind` lists every prompt of the session, also the prompts from before each compaction. A short alias helps: `alias che='crap --here --expand'`.

`crap` writes an **expanded import**: a new transcript under a new id, in the project folder where the fork runs. The import is the original transcript with these changes:

1. The boundary and the summary of each compaction are removed. Each message that came after them is linked to the last message before the compaction (the `logicalParentUuid` of the boundary).
2. The `sessionId` of each line is the id of the import.
3. The `bridge-session` lines are removed, so the fork does not attach to the Remote Control session of the original, which can be live.
4. The `queue-operation` lines are removed, so old queued input does not play again.

Each other line stays byte-identical. The shell function resumes the import with `--fork-session`, and it removes the import when the fork appears, the same as the symlink of `--here`. Thus the shell function needs no change.

- The original transcript is only read. Thus `--expand` works also when the original session is live, and it ignores `--force`.
- `crap --expand <id>` forks too, at the original directory. A plain resume writes to the original transcript, and `--expand` never changes the original.
- `--expand` works with `--user <name>`, with the automatic cross-user fallback, and with the second argument of `--here`. For a cross-user source, the expanded import replaces the plain copy, and `crap` writes nothing in the other home.
- If the session has no compaction, `crap` says so on stderr and does the same as the command without `--expand`.

The expanded history can be larger than the context window. `crap` prints its size on stderr:

```text
Note: crap expanded 1 compaction of session 57570685-2d64-4431-8ab6-c021a12fa1af.
      Before the last compaction, the context held 480231 tokens.
      The expanded history can be larger than the context window.
      If it is larger, use /rewind before you send the first message.
```

`crap` refuses a transcript that it cannot expand, and it writes no import (exit code `13`). It names the boundary or the summary that stopped it. The causes are:

- A boundary has no `logicalParentUuid`, or no line of the transcript has that uuid.
- A boundary has no summary, or a summary has no boundary. A boundary or a summary has no `uuid`.
- The transcript does not hold the messages from before a compaction. This occurs in a fork that Claude made after the compaction, because Claude copies only the summary and the messages after it into the fork. Expand the session that the fork came from. `crap --forks <id>` names that session when `crap` made the fork.

Two limits: "Restore code" in the fork does not find the file backups, because they are under the id of the original session (`--here` has the same limit). `crap` does not expand subagent transcripts.

## Resume another user's session: automatic, or `--user`

A session started under a different account is found automatically: when the id isn't in your own tree, plain `crap <id>` falls back to the other accounts on the machine (see below). `--user <name>` is the explicit form — point `crap` at one account by name and it searches only that user's `~/.claude/projects` tree instead:

```bash
crap 57570685-2d64-4431-8ab6-c021a12fa1af --user alice
```

The name resolves as a sibling of your home (`<home>/../<name>` — `/Users/alice` on macOS, `/home/alice` on Linux). Because a transcript that belongs to another user can never be found by a `claude --resume` you run yourself, `crap` copies it into your own tree and resumes it as a `--fork-session` (a fresh id) at its original recorded directory — the foreign transcript is only ever read, and every write lands under your home. The transient copy is removed once Claude writes the forked transcript, the same way `--here` cleans up its import (a symlink for a same-user source, a copy for a cross-user one). Because the fork only reads the original, `--user` is safe even while that session is still live in the other user's account. A `--user` naming your own account is a same-user hit and simply resumes in place. That includes another name for your own home: a different case on a case-insensitive filesystem (`--user Tim` for `/Users/tim` on the macOS default, or for `/home/tim` on an ext4 casefold, CIFS or ntfs3 home on Linux), or a symlinked alias. `crap` compares the trees themselves (the device and the inode of each), not the names or the resolved paths.

Most of the time you don't need the flag at all. With no `--user`, `crap <id>` searches **your own** tree first — byte-for-byte the same fast path as always — and only if the id isn't there does it automatically fall back to scanning every sibling home that has run Claude, resuming the first readable match exactly as `--user` would (copy into your own tree, fork at the original directory). The fallback is **self-first**, so an id that happens to exist in two accounts always resolves to *your* copy, never the foreign one. Reach for `--user <name>` only when you want to force a specific account — it skips your own tree entirely, which is also how you disambiguate on purpose.

## When a project directory is owner-only

Claude Code creates its project folders with whatever the owner's umask gives them, and on a shared machine that is often `0o700` — the owner may read, list, and enter; nobody else may do anything. Such a directory is *opaque*, not merely unreadable: `crap` can't list it, so it can't even tell whether the session you asked for is inside. Seeing inside would take `sudo`, and `crap` scans other homes with exactly the privileges you invoked it with.

So a directory it is refused is **skipped and remembered** — along with the account that owns it — instead of ending the search. (The same goes one level up, when an entire account's `~/.claude/projects` can't be listed.) Treating a permission error as fatal would let one locked folder in someone else's home hide a session sitting perfectly readable two folders later; this way an unreadable neighbour never makes a readable session unfindable.

If the id then turns up nowhere readable **and** at least one directory was skipped, `crap` doesn't flatly claim the session doesn't exist — a claim it isn't in a position to make. The headline hedges to "no *readable* Claude session", every account that refused it gets a count of what was stepped over, and you get the remedy as commands you can paste:

```text
Error: no readable Claude session found with id '99999999-8888-7777-6666-555555555555'
       looked under /Users/me/.claude/projects
       …and 2 other accounts on this machine
       1 project dir under user 'alice' is owner-only and was skipped.
       1 project dir under user 'scyloswork' is owner-only and was skipped.
       if the session is in one of those, crap cannot read it — and crap never
       runs sudo itself. copy it into your own tree first — for user 'alice',
       for example:
         SRC=$(sudo -u alice find /Users/alice/.claude/projects -name '99999999-8888-7777-6666-555555555555.jsonl')
         mkdir -p ~/.claude/projects/-Users-me-code-foo
         sudo -u alice cat "$SRC" > ~/.claude/projects/-Users-me-code-foo/99999999-8888-7777-6666-555555555555.jsonl
         crap --here 99999999-8888-7777-6666-555555555555
```

Those four lines do exactly what `crap` would have done for you if it could see: find the transcript anywhere in the owning account's tree, copy it into your own, and resume it. The copy lands in the project folder for **the directory you're standing in**, and the last line is therefore [`crap --here`](#resume-in-the-current-directory---here) rather than a plain resume — with the transcript unreadable, the directory recorded inside it is precisely the thing that can't be known, and your current directory is both knowable and almost certainly where you meant to work, since it's where you typed the id. Counts are printed for every account, but the worked example is keyed to the first one named; if the session turns out to belong to a different account, swap the name in.

That is where `crap` stops. It prints the escalation for you to read, weigh, and run yourself — it never runs one, not even to satisfy the request you just made. This isn't a convention a later change could quietly relax: a test scans the compiled source for every program the binary spawns and fails on anything outside a two-entry allowlist (`ps`, for the is-it-still-running check, and `bash`, which the shell-integration tests use), and it holds the shell function `--shell-setup` installs to the same rule. Escalating stays your decision, and the audit trail points at you rather than at a tool that reached for `sudo` on your behalf.

A miss with nothing skipped is unchanged: `crap` looked everywhere it can look, so it still says plainly that no session has that id.

## When the original directory is gone or sealed, or the account is wrong

A cross-user resume lands you at the session's **original** recorded directory (`--here` is the exception — it deliberately ignores that directory and forks where you stand). When the original directory can't be used, `crap` refuses rather than silently dropping you in the current directory — the same contract a same-user resume has always had — and points you straight at the escape hatch that works precisely then:

```text
Error: the directory for session '57570685-…' no longer exists:
       /Volumes/code/old-worktree
       use 'crap --here 57570685-…' to fork it in the current directory instead.
```

There are two distinct versions of this, because "gone" and "sealed" are different facts. If the directory simply isn't there any more, that's the `no longer exists` message above (exit code `3`). If it *is* there but your account can't enter it — a parent folder you lack permission on, or the directory missing its search bit — you get `cannot be entered from this account:` instead (exit code `11`), never a silent fall-back to wherever you happened to be. Either way, `crap --here <id>` is the fix: it never touches the original directory, so it succeeds exactly when the original directory is the problem.

Separately, if `--user <name>` names an account that isn't there — a typo, or a real account that has simply never run Claude — `crap` says so up front instead of searching a phantom tree and reporting a misleading "no session found". It lists the accounts you *can* resume from (exit code `12`):

```text
Error: --user 'alcie' does not name an account with a Claude projects tree.
       accounts you can resume from with --user:
         me
         alice
```

An account whose projects tree merely *exists but is unreadable* to you is **not** treated as invalid — it's a real account, so `crap` resolves it and the [owner-only guidance](#when-a-project-directory-is-owner-only) above takes over (which, unlike this message, does show you the `sudo` remedy). `--user` reports a wrong *name*; owner-only reports sealed *data*.

## Don't attach twice

Claude Code records every live CLI session under `~/.claude/sessions/<pid>.json` and removes it on clean exit. Before resuming, `crap` checks that registry: if the session you asked for is already open in another running `claude` process, it refuses and tells you where:

```text
Error: session '4d1637ec-…' is already running (pid 62043, idle)
       in /Volumes/code/muxiavelli
       resuming it again can corrupt the session log.
       re-run with --force to resume anyway.
```

This prevents two processes from appending to the same session log at once. The check verifies the recorded pid is still a live `claude` process (so a stale file left by a crash — or a pid since reused by something else — won't trigger a false alarm). Pass `--force` to resume anyway.

## Check a session's state: `--status`

Before resuming — or when scripting over many sessions — you can ask where a session left off without launching anything:

```bash
crap --status 57570685-2d64-4431-8ab6-c021a12fa1af
```

It prints exactly one of these tokens on stdout:

- `waiting-for-user` — Claude finished its turn and is waiting for your input.
- `busy` — work is in flight: the assistant has a pending tool call, or a tool result was just delivered and the reply hasn't landed yet.
- `awaiting-assistant` — you sent the last message and Claude hasn't replied (an active turn, or a session abandoned mid-reply).
- `empty` — the transcript has no conversational turns yet.

Claude Code never writes an explicit "waiting for input" marker, so `crap` infers the state from the last real turn in the transcript — skipping subagent (`isSidechain`) turns, injected (`isMeta`) entries, and trailing bookkeeping lines, and trusting each turn's `stop_reason` over the per-line content shape.

If the session is **currently open** in a live `claude` process, that process's own status is more authoritative than transcript inference, so it's reported instead:

```text
busy (live, pid 17041)
```

The tokens are stable and newline-terminated, so they script cleanly:

```bash
[ "$(crap --status "$id")" = waiting-for-user ] && echo "ready for you"
```

`--status` exits non-zero for a malformed id or a session that is neither live nor on disk.

### List every session for the current directory

Give `--status` **no id** and it lists every session recorded for the directory you're in — handy when a single project has several conversations going. Each row shows the state plus when the transcript was *started* and *last written*, read from the transcript's own timestamps (not file mtimes), so they reflect real activity:

```text
2 sessions for /Volumes/code/crap

┌──────────────────────────────────────┬────────────────────────┬─────────────────────┬─────────────────────┐
│ SESSION                              ┆ STATE                  ┆ STARTED             ┆ LAST                │
╞══════════════════════════════════════╪════════════════════════╪═════════════════════╪═════════════════════╡
│ c43eb4df-1ba3-4c42-84f2-ab76319a860c ┆ waiting-for-user       ┆ 2026-05-25 20:02:29 ┆ 2026-05-25 20:11:21 │
│ 1c8aad51-26aa-416d-8da9-a0b586fd0632 ┆ busy (live, pid 98519) ┆ 2026-05-25 18:43:05 ┆ 2026-05-25 20:29:44 │
└──────────────────────────────────────┴────────────────────────┴─────────────────────┴─────────────────────┘
```

Rows are ordered oldest-activity first, so the most recently used session sits at the bottom. Live sessions show their own status and pid; the rest show the inferred state. A session with no recorded activity shows `—` for its times.

### JSON output: `--json`

Add `--json` (only valid with `--status`) for machine-readable output instead of text — a single object for one id, an array for the directory listing. Keys are camelCase and timestamps are the raw ISO 8601 values, so it pipes straight into `jq`:

```bash
# Which sessions here are waiting on me?
crap --status --json | jq -r '.[] | select(.state == "waiting-for-user") | .sessionId'
```

```json
[
  {
    "sessionId": "c43eb4df-1ba3-4c42-84f2-ab76319a860c",
    "state": "waiting-for-user",
    "started": "2026-05-25T20:02:29.035Z",
    "last": "2026-05-25T20:11:21.375Z"
  }
]
```

`started` and `last` are `null` when the transcript records no timestamps.

## Find the forks of a session: `--forks`

Each fork that `crap` starts adds one line to a fork log. That is each `crap --here <id>` (an alias such as `ch` for `crap --here` too), each `--expand` fork, and each cross-user resume. Claude Code records no link between a fork and its source, so without the log you cannot find which sessions came from a session, or which session a fork came from.

The log is `forks.jsonl` in the `crap` directory of your data directory: `~/Library/Application Support/crap/forks.jsonl` on macOS, and `$XDG_DATA_HOME/crap/forks.jsonl` (else `~/.local/share/crap/forks.jsonl`) on Linux. Each line is one JSON object:

```json
{"time":"2026-10-28T01:51:22Z","source_id":"57570685-2d64-4431-8ab6-c021a12fa1af","fork_id":"9f8e7d6c-5b4a-4210-8edc-ba9876543210","source_user":"me","source_dir":"/Users/me/code/app","fork_dir":"/Users/me/code/app-worktrees/issue-7","kind":"here"}
```

- `time`: when `crap` started the fork, in RFC 3339 and UTC.
- `source_id`: the session that was forked. `fork_id`: the new session. For an `--expand` fork, `source_id` is the original session, not the expanded import.
- `source_user`: the account that owns the source transcript.
- `source_dir`: the directory that the source transcript records, or `null` when it records none.
- `fork_dir`: the directory where the fork runs.
- `kind`: `here` for `--here`, and `fork-at` for a fork in the original directory (a cross-user resume, or `crap --expand <id>`).
- `expanded`: `true` for an `--expand` fork. The other lines do not have this key, and a line without it reads as `false`.

A cross-user fork writes to your own log, never into the other home. If `crap` cannot write the log, it shows a warning and starts the fork, because the log must not stop a resume. `crap` writes each line with one `write` call in append mode, so two forks at the same time cannot mix their lines. `crap` writes the line before Claude starts, but Claude writes the fork transcript only after your first new input. Thus the log can name a fork that has no transcript.

To read the log for one session:

```bash
crap --forks 9f8e7d6c-5b4a-4210-8edc-ba9876543210
```

```text
Forks of 9f8e7d6c-5b4a-4210-8edc-ba9876543210 in the fork log:
  3c2b1a09-8f7e-4d6c-9b5a-4f3e2d1c0b9a  2026-10-28T03:00:41Z  not saved  /Users/me/code/app-worktrees/issue-8

Sources of 9f8e7d6c-5b4a-4210-8edc-ba9876543210 in the fork log:
  9f8e7d6c-5b4a-4210-8edc-ba9876543210 is a fork of 57570685-2d64-4431-8ab6-c021a12fa1af, made 2026-10-28T01:51:22Z (here) in /Users/me/code/app-worktrees/issue-7
  57570685-2d64-4431-8ab6-c021a12fa1af is not a fork in the fork log.
```

- If the session is a source, the report lists each fork with its time, `saved` or `not saved`, and its directory. `saved` means that `crap --status <fork-id>` finds the fork transcript, so `crap <fork-id>` can resume the fork. An `--expand` fork has `(expanded)` after its directory.
- If the session is a fork, the report shows its source, then the source of that source, up to a session that is not a fork. An `--expand` fork shows `(here, expanded)` or `(fork-at, expanded)`.
- If the session is both, the report shows both sections.

`--forks` resumes nothing and writes nothing. It exits `1` when no transcript and no line of the log has the id. If a line of the log cannot be read, `--forks` shows a warning and uses the other lines.

### Find forks that are not in the log: `--scan`

The log has only the forks that `crap` started after the log existed. Older forks, and forks that another tool made (for example `claude --fork-session` without `crap`), have no line. `--scan` finds them in the transcripts:

```bash
crap --forks 4cf057cc-eaa4-4e2b-b10d-60f031d545bd --scan
```

```text
The fork log has no fork of 4cf057cc-eaa4-4e2b-b10d-60f031d545bd, and 4cf057cc-eaa4-4e2b-b10d-60f031d545bd is not a fork in the fork log.

Transcripts that share uuid values with 4cf057cc-eaa4-4e2b-b10d-60f031d545bd (scan of /Users/me/.claude/projects):
  f7e903d0-5884-42c6-ae33-20a8763e5083  fork         23 shared  /Users/me/.claude/projects/-Users-me/f7e903d0-5884-42c6-ae33-20a8763e5083.jsonl
```

When Claude forks a session, it copies lines of the session into the new transcript, and each copied line keeps its `uuid`. The scan reads the `uuid` values of the transcript of the id. Then it reads each transcript in your own `~/.claude/projects` tree, skips the subagent transcripts, and reports each transcript that has one or more of those values. The relation column shows the direction:

- `fork`: the other transcript is a fork of the id. The id has lines older than the first shared line, and the other transcript has none.
- `source`: the other transcript is the source of the id.
- `related`: the two transcripts share lines, but the data does not show the direction. For example, a fork of a session that never compacted copies every line, so neither transcript has older lines.

The scan parses each line as JSON and reads only the top-level `uuid` key. Thus a transcript that holds a `uuid` value only in a tool result (the output of a `cat` of another transcript, for example) is not a pair. For speed, the scan first finds the files that hold one of the values as text, and then parses only those files. On 2026-10-28, a scan of 24 GB of transcripts took 26 seconds.

The scan reads only your own tree. A cross-user fork removes its copy of the foreign transcript when Claude writes the fork, so the scan cannot find the source of a cross-user fork. The log has that source.

If you installed the shell function before `--forks` existed, run `crap --shell-setup` again. The old function does not send `--forks` to the binary, so it reads the report as a resume target and stops at the `cd`. Until you run the setup again, `command crap --forks <id>` works.

## Options

- `[SESSION_ID]`: The Claude session id to resume (optional with `--status`, which then lists every session for the current directory). The lookup is **self-first** — your own tree first, then, only on a miss, every sibling home that has run Claude — so an id belonging to another account is found and forked automatically, with no flag
- `-f, --force`: Resume even if the session appears to be running in another process
- `--here`: Resume the session in the current directory (as a forked, new-id session) instead of its original one; also accepts a cross-user source (combined with `--user`), which is copied into your own tree rather than symlinked
- `--expand`: Resume a fork of the session with each compaction undone, so `/rewind` in the fork lists the prompts from before each compaction. With `--here`, the fork runs in the current directory. Without it, the fork runs in the original directory. It works with `--user`, it ignores `--force`, and it prints the size of the history on stderr. See [Resume with the full history](#resume-with-the-full-history---expand)
- `--user <name>`: Resume another user's session from a specific account — **not required** for a cross-user resume, since the no-flag path already falls back on its own; reach for it when you want to force one account in particular. `<name>` is resolved as a sibling of your home (`<home>/../<name>` — `/Users/<name>` on macOS, `/home/<name>` on Linux), and only that user's `~/.claude/projects` tree is searched (your own is skipped, so `--user` also disambiguates an id on purpose). The foreign transcript is copied into your own tree and resumed as a fork (fresh id) at its original directory — or in the current directory instead when combined with `--here` — the original is only ever read. Naming your own account is a same-user hit and resumes in place, also by another name for the same home (a different case on a case-insensitive filesystem, or a symlinked alias)
- `--status`: Print the session's conversational state (`waiting-for-user`, `busy`, `awaiting-assistant`, or `empty`; or `<status> (live, pid <pid>)` when open elsewhere) and exit, without resuming. With no id, lists every session for the current directory with its state and start/last times
- `--json`: With `--status`, emit JSON instead of text (one object for an id, an array for the directory listing)
- `--forks`: Show what the fork log knows about the session — each fork of it, with `saved` or `not saved`, and the chain of sessions it was forked from — and exit, without resuming
- `--scan`: With `--forks`, also read every transcript in your own `~/.claude/projects` tree and report each one that shares a top-level `uuid` value with the session, with the count and the relation (`fork`, `source`, or `related`)
- `--shell-setup`: Add the `crap` shell function to your ~/.zshrc or ~/.bashrc

## Shell Integration

Because a program can't change its parent shell's working directory — and can't see the shell alias that `CRAP_LAUNCHER` can name — `crap` ships as a shell function. Install it once:

```bash
crap --shell-setup
```

Then run `source ~/.zshrc` (or `~/.bashrc`), or open a new terminal. After that, `crap <session-id>` will `cd` into the session's directory and resume it.

> **Note:** `--shell-setup` supports bash and zsh. The bare `crap` binary still works without the function — it just prints the session's directory to stdout instead of changing into it.

### Manual Setup

If you prefer to add it manually, add this to your `~/.bashrc` or `~/.zshrc`:

```bash
# Start Claude with the argv in "$@". CRAP_LAUNCHER holds the launcher as shell
# text: an alias, a function, or a command with flags. When it is empty, plain
# claude starts Claude. A shell does not expand an alias in a function body
# that it parsed before the alias existed, and eval parses the text when this
# function runs.
function __crap_launch() {
    eval "${CRAP_LAUNCHER:-claude}"' "$@"'
}
function crap() {
    # These flags make the binary print to stdout and exit 0 without mutating
    # the parent shell: --status queries, --forks prints a report from the fork
    # log, --help/-h/--version/-V emit
    # informational text, and --shell-setup writes the rc file (not the live
    # shell) and prints activation instructions. Run them straight through so
    # their output reaches the terminal instead of being parsed as a
    # "<session-id>\n<dir>" resume target (which would otherwise `cd` into that
    # text and mangle it). --shell-setup matters on upgrades, when this very
    # function is already loaded and would otherwise swallow its instructions.
    case " $* " in
        *" --status "*|*" --forks "*|*" --help "*|*" -h "*|*" --version "*|*" -V "*|*" --shell-setup "*)
            command crap "$@"; return $? ;;
    esac
    local __crap_out
    __crap_out=$(command crap "$@") || return $?
    local __crap_mode
    __crap_mode=${__crap_out%%$'\n'*}
    if [ "$__crap_mode" = "__CRAP_HERE__" ] || [ "$__crap_mode" = "__CRAP_FORK_AT__" ]; then
        # A fork. Both wire shapes start "<mode>\n<session>\n<new-id>\n<link>".
        # A cross-user resume (__CRAP_FORK_AT__) adds a trailing <dir>: the
        # session's ORIGINAL directory, where the fork must run. The field that
        # can hold any text is always last (the link for --here, the dir for a
        # cross-user fork), so a path that contains newlines survives intact.
        local __crap_rest __crap_session __crap_newid __crap_link __crap_dir __crap_folder __crap_n0 __crap_watcher
        __crap_rest=${__crap_out#*$'\n'}
        __crap_session=${__crap_rest%%$'\n'*}
        __crap_rest=${__crap_rest#*$'\n'}
        __crap_newid=${__crap_rest%%$'\n'*}
        __crap_rest=${__crap_rest#*$'\n'}
        if [ "$__crap_mode" = "__CRAP_FORK_AT__" ]; then
            __crap_link=${__crap_rest%%$'\n'*}
            __crap_dir=${__crap_rest#*$'\n'}
            cd -- "$__crap_dir" || return 1
        else
            __crap_link=$__crap_rest
        fi
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            # Claude only needs the import (a symlink, or a copy for a
            # cross-user source) while it reads the transcript at startup;
            # once it writes the forked session file, the import is
            # vestigial. Watch the folder and drop it the moment a new .jsonl
            # appears, rather than letting it linger for the whole session.
            __crap_folder=$(dirname -- "$__crap_link")
            __crap_n0=$(find "$__crap_folder" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l | tr -dc '0-9')
            (
                __crap_i=0
                while [ "$__crap_i" -lt 600 ]; do
                    if [ "$(find "$__crap_folder" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l | tr -dc '0-9')" -gt "$__crap_n0" ]; then
                        rm -f -- "$__crap_link"
                        exit 0
                    fi
                    __crap_i=$((__crap_i + 1))
                    sleep 0.1
                done
            ) &
            __crap_watcher=$!
            disown 2>/dev/null
        fi
        # Build the resume argv: always --fork-session, so the original
        # transcript is left untouched, and always --session-id with the id
        # that the binary supplied, so the fork id is known after Claude exits.
        # A launcher in CRAP_LAUNCHER that keeps that id in its own argv then
        # resumes the fork after a Zellij resurrect, not the original.
        # The earlier "command crap" call has already consumed the function's
        # own arguments, so reusing the positional parameters here is safe.
        set -- --resume "$__crap_session" --fork-session --session-id "$__crap_newid"
        __crap_launch "$@"
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            kill "$__crap_watcher" 2>/dev/null
            rm -f -- "$__crap_link"
        fi
        # Claude writes the fork transcript only after the first new input, so
        # a fork that the user leaves at once is not saved. --status finds the
        # fork exactly when "crap <new-id>" can resume it, so tell the id only
        # then.
        if command crap --status "$__crap_newid" >/dev/null 2>&1; then
            printf 'Resume this fork with: crap %s\n' "$__crap_newid"
        fi
        return
    fi
    local __crap_session __crap_dir
    __crap_session=${__crap_out%%$'\n'*}
    __crap_dir=${__crap_out#*$'\n'}
    cd -- "$__crap_dir" || return 1
    __crap_launch --resume "$__crap_session"
}
```

If you add the function manually, paste it again after each upgrade of `crap`. An older copy does not read the output of a newer binary correctly.

The binary prints one of three output shapes on stdout:

- **Default:** `<session-id>\n<dir>`. The function uses the first line as the session id and all of the remaining text as the directory. Thus a path that contains a newline stays whole. The function goes into that directory with `cd` and resumes the session.
- **`--here`:** `__CRAP_HERE__\n<session-id>\n<new-id>\n<link-or-__CRAP_NO_LINK__>`. The binary already put the session into the project folder of the current directory. This import is a symlink for a same-user source, or a copy for a cross-user source. With `--expand`, the import is an expanded import, and `<session-id>` is the id of the import, not of the original. The function stays in the current directory.
- **Cross-user:** `__CRAP_FORK_AT__\n<session-id>\n<new-id>\n<link-or-__CRAP_NO_LINK__>\n<dir>`. The binary already copied the transcript of the other user into your own tree. A cross-user resume from `--user` or from the automatic fallback gives this shape, and so does `crap --expand <id>`, with the id of the expanded import as `<session-id>`. The function goes into `<dir>` with `cd`. `<dir>` is the original directory of the session.

The two fork shapes use one path through the function. The only difference is the `cd` for the cross-user shape. The field that can hold any text is always the last field, so a newline in a path does not break the fields.

If the link field is not `__CRAP_NO_LINK__`, a watcher in the background counts the `.jsonl` files in the project folder. When a new file (the fork) appears, the watcher removes the import. Thus the import does not stay for the full session. After Claude exits, the function stops the watcher with `kill`. Then it removes the import with `rm`, in case the watcher did not. If the link field is `__CRAP_NO_LINK__`, the binary made no import, and the function starts no watcher.

The function always resumes a fork with `--resume <session-id> --fork-session --session-id <new-id>`. `--fork-session` keeps the original transcript unchanged. `--session-id` gives the fork the id that the binary chose, so the function knows the fork id after Claude exits.

After the cleanup, the function prints `Resume this fork with: crap <new-id>`. It prints this line only when `crap --status <new-id>` finds the fork. Claude saves the fork only after your first new input. Thus, if you exit a fork at once, the function prints no line.

The function sends `--status`, `--forks`, `--help`, `-h`, `--version`, `-V`, and `--shell-setup` directly to the binary. These flags do not change the parent shell, and their output must go to the terminal. The function forwards `"$@"`, so flags such as `--force` and `--here` get to the binary. Both paths start Claude through `__crap_launch`, so the launcher choice is in one place. The `eval` in `__crap_launch` is intentional. A shell does not expand an alias in a function body that it parsed before the alias existed. `eval` parses the launcher text when the function runs, so an alias that `CRAP_LAUNCHER` names expands, and the flags of a launcher become separate words. The `command crap` calls get to the binary, not to the function of the same name.

## Exit Codes

- `0`: Success
- `1`: No session found with that id (for `--forks`: no transcript and no line of the fork log has that id)
- `2`: Session has no recorded working directory
- `3`: The session's original directory no longer exists
- `4`: Invalid session id
- `5`: Shell setup failed
- `6`: Could not determine your home directory
- `7`: The session is already running in another process (use `--force` to override)
- `8`: `--here`, `--expand`, or a cross-user resume (whether `--user` asked for one or the automatic fallback found it): could not import the session into the project folder (create the folder, or make the symlink for a same-user source / copy the transcript for a cross-user source / read the transcript and write the expanded import for `--expand`)
- `9`: `--here`: could not determine the current working directory
- `10`: `--here`: the requested new session id already names an existing session (choose a fresh id so the fork can't overwrite it)
- `11`: The session's original directory exists but can't be entered from this account (a sealed parent, or a missing search bit) — use `crap --here <id>` to fork it in the current directory instead
- `12`: `--user <name>` named no account with a `.claude/projects` tree (a typo, or an account that never ran Claude); the message lists the accounts you can resume from
- `13`: `--expand`: the transcript has a compaction that `crap` cannot undo; the message names the boundary or the summary, and no import was written
