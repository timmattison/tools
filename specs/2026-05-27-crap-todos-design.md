# `crap todos` — extract todo/task lists from a Claude session

Date: 2026-05-27
Status: design approved, pending spec review

## Problem

`crap` can already resume a Claude session, bring it to the current directory
(`--here`), and report its conversational state (`--status`). It has no way to
answer *"what was this session actually working on?"* — the to-do checklist or
task list the agent (and its subagents) maintained. That state lives in the
session transcript and is currently only visible by hand-grepping JSONL.

This feature adds `crap todos <uuid>`: given a session id, find the session,
reconstruct the **latest** todo/task list for the main thread **and every
subagent**, and display it nicely in the terminal, with a `--json` form for
scripts.

## Two tracking mechanisms (both in scope, auto-detected)

A Claude agent records progress with one of two distinct, mutually exclusive
tool families. The feature detects which one each agent used and folds its
event stream to the latest state.

### TodoWrite (classic todo list)

- Recorded as `tool_use` blocks named `TodoWrite`.
- Each call carries `input.todos`, a **full snapshot** of the entire list:
  `[{ "content": str, "status": "pending"|"in_progress"|"completed",
  "activeForm": str }]`.
- Because every call replaces the whole list, the **last** `TodoWrite` call in
  the agent's transcript is the current state. No folding needed beyond
  "take the last one".

### TaskCreate / TaskUpdate (task system)

- `TaskCreate` `tool_use` input: `{ "subject": str, "description": str,
  "activeForm": str }`. The task **id is not in the input** — it is assigned by
  the runtime and returned in the following `tool_result`, whose text reads
  `"Task #<N> created successfully: <subject>"`.
- `TaskUpdate` `tool_use` input: `{ "taskId": str, "status": str }`. Observed
  statuses: `in_progress`, `completed`. `pending` is the implicit initial state
  at creation. `TaskStop` may introduce a stopped/cancelled state; treat any
  unrecognised status as an opaque pass-through string rather than failing.
- Reconstruction = **fold the stream**: seed task `N` (subject, `pending`) from
  each `TaskCreate` result line, then apply each `TaskUpdate` in order to mutate
  status. Tasks are ordered by ascending numeric id.

Parsing the id from the `tool_result` text is the source of truth for the
id→subject mapping (creation order also yields `1..N`, but the result text is
explicit and robust to interleaving).

## Session & subagent layout

```
~/.claude/projects/<encoded-project>/
  <uuid>.jsonl                         # main thread
  <uuid>/subagents/
    agent-<hex>.jsonl                  # one per subagent
    agent-<hex>.meta.json              # { agentType, description, toolUseId }
```

- `find_session_file` (already in `crap`) locates `<uuid>.jsonl` under any
  project folder. The subagents directory is its sibling: replace the `.jsonl`
  extension with `/subagents`.
- Each subagent is labelled from its `agent-<hex>.meta.json` (`description` +
  `agentType`). Older sessions may lack the `.meta.json`; fall back to the
  `Agent` tool_use in the main thread matched by `toolUseId`, then to the bare
  `agent-<hex>` file stem.
- The `subagents/` directory may be absent (no subagents) — that is normal, not
  an error.
- Subagents are ordered by first-activity transcript timestamp (reuse
  `transcript_time_span`); the main thread is always listed first.

A subagent may drive the *parent's* task list rather than owning its own; in
that case its own transcript has no `TodoWrite`/`TaskCreate` calls and it is
reported as `kind: "none"` ("no todo/task list recorded").

## CLI surface — flags become subcommands

`crap` converts from a flag-driven CLI to a clap subcommand enum. Backward
compatibility is preserved by making `resume` the **default subcommand**, so a
bare `crap <uuid>` still resumes.

| New form | Old form | Behavior |
| --- | --- | --- |
| `crap resume <uuid>` (also bare `crap <uuid>`) | bare `crap <uuid>` | cd into the session's original dir + resume |
| `crap here <uuid>` | `crap --here <uuid>` | symlink into cwd, resume forked |
| `crap status [<uuid>]` | `crap --status [<uuid>]` | conversational state (token / table) |
| `crap todos [<uuid>]` | — *(new)* | extract todo/task lists |
| `crap shell-setup` | `crap --shell-setup` | install shell function |

- `--force` is a flag on `resume` and `here`.
- `--json` is a flag on `status` and `todos`.
- `--verbose` is a flag on `todos` (see Display).
- Bare `crap <uuid>` maps to `resume` via clap's default-subcommand pattern
  (`subcommand` optional + a top-level positional, or
  `args_conflicts_with_subcommands`). An unknown first token that is a valid
  UUID resolves to `resume <uuid>`; an unknown non-UUID token errors with a
  "did you mean a subcommand?" style message from clap.

## Shell-function rewrite (highest-risk change)

The installed `crap()` shell function currently branches on **flags**. It must
branch on the **subcommand** (`$1`):

- `todos | status | shell-setup | help | --help | -h | --version | -V` → run
  straight through `command crap "$@"` and return (print, never `cd`).
- `here` → the existing `__CRAP_HERE__` sentinel flow (symlink + `--fork-session`
  + watcher cleanup), unchanged internally.
- `resume`, or a bare UUID (default) → the `<id>\n<dir>` parse, `cd`, resume.

`with_old_end_marker()` must be set so existing installs upgrade in place per
the repo's `shellsetup` rules; the marker is a distinctive line from the current
shell block. The rewrite gets dedicated tests (a parser unit-tested at the Rust
level for the routing decision, plus a shellcheck pass on the emitted block).

## Data model (new module `src/crap/src/todos.rs`)

Pure, `&str`-in functions (no filesystem fixtures), mirroring how
`classify_session_state` / `transcript_time_span` are written and tested.

```rust
enum ListKind { Todos, Tasks, None }      // serializes to "todos"|"tasks"|"none"

struct Item {                              // unified todo/task item
    id: Option<String>,                    // task id; None for todos
    label: String,                         // todo.content or task.subject
    status: String,                        // pending|in_progress|completed|<other>
    detail: Option<String>,                // task.description (verbose only); None for todos
}

struct AgentList {
    agent: String,                         // "main" or "agent-<hex>"
    agent_type: Option<String>,            // from meta.json
    description: Option<String>,           // from meta.json / Agent tool_use
    kind: ListKind,
    items: Vec<Item>,
}

struct SessionTodos {
    session_id: String,
    cwd: Option<String>,
    agents: Vec<AgentList>,                // main first, then subagents by first-activity
}
```

Core pure functions:

- `extract_todowrite_latest(contents: &str) -> Option<Vec<Item>>` — last
  `TodoWrite` snapshot for one transcript.
- `extract_tasks(contents: &str) -> Option<Vec<Item>>` — fold
  `TaskCreate`/`TaskUpdate`, parsing ids from `tool_result` text.
- `extract_agent_list(contents: &str) -> (ListKind, Vec<Item>)` — auto-detect:
  prefer tasks if any `TaskCreate` is present, else todos, else `None`.
- `parse_task_created(result_text: &str) -> Option<(String, String)>` — pull
  `(id, subject)` out of `"Task #N created successfully: <subject>"`.

Filesystem glue in `main.rs` (or a thin resolver) walks main + subagents,
reads `.meta.json`, and assembles `SessionTodos`.

## Terminal display (`crap todos <uuid>`)

```
Session 96281973  ·  /…/muxiavelli-worktrees-issue-99  ·  tasks: 4/5 done

main
  ✔ Slice 1: CSS active-panel indicator rule
  ✔ Slice 2: Terminal active prop stamps class
  ✔ Slice 3: usePanelActive hook
  ✔ Slice 4: Wire TerminalPanel + e2e
  ▶ Validate: full feedback loop            (in progress)

subagent · general-purpose · "Slice 3: usePanelActive hook"
  ☐ (no todo/task list recorded)
```

- Glyphs (Unicode, per repo style): `✔` completed, `▶` in_progress, `☐`
  pending; unknown status → the raw token in parentheses.
- Color via `colored`, consistent with the rest of `crap`.
- An agent with no list is shown explicitly ("no todo/task list recorded"),
  never silently dropped.
- Header summary counts completed/total across the **main** thread's list
  (with `(N active)` when any are in_progress).
- Long labels are truncated with the UTF-8-safe `chars()` approach (per repo
  rule), with the full text always available via `--verbose`.

### `--verbose`

Appends each task's full `description` text under its line (indented, wrapped to
terminal width). Todos have no separate description, so `--verbose` is a no-op
for todo lists. In JSON, `--verbose` populates `items[].detail`.

## JSON output (`crap todos <uuid> --json`)

```json
{
  "sessionId": "96281973-…",
  "cwd": "/…/issue-99",
  "agents": [
    {
      "agent": "main",
      "agentType": null,
      "description": null,
      "kind": "tasks",
      "items": [
        { "id": "1", "label": "Slice 1: CSS …", "status": "completed", "detail": null },
        { "id": "5", "label": "Validate: …",    "status": "in_progress", "detail": null }
      ]
    },
    {
      "agent": "agent-ad26357556b16d09a",
      "agentType": "general-purpose",
      "description": "Slice 3: usePanelActive hook",
      "kind": "none",
      "items": []
    }
  ]
}
```

- `detail` is `null` unless `--verbose` is passed (then the task description for
  task items; still `null` for todo items).
- `agentType`/`description` are `null` for the main thread and for subagents
  lacking a `.meta.json` with no recoverable label.

## No-id form (`crap todos`)

Mirrors `crap status` with no id: a table over every session recorded for the
current directory, one row per session with a progress summary.

```
SESSION     KIND    PROGRESS        LAST
96281973…   tasks   4/5 (1 active)  2026-05-27 14:42
c44fbaf3…   todos   5/5             2026-05-25 18:43
```

- `PROGRESS` summarises the **main** thread's list (`done/total`, `(N active)`
  when any in_progress); a session with no list shows `—`.
- `--json` emits an array of `{ sessionId, kind, done, total, active, last }`.
- Ordering matches `status`: ascending by last-activity, ties by id.

## Error handling

Reuse `crap`'s existing exit-code discipline:

- Invalid UUID → `INVALID_SESSION_ID`.
- No matching session file → `SESSION_NOT_FOUND`.
- No home dir → `NO_HOME_DIR`.
- Missing `subagents/` dir, unreadable individual subagent file, or absent
  `.meta.json` → **not** fatal: that subagent is skipped or labelled from
  fallbacks, and the rest of the report is produced.
- `cwd` unavailable for the no-id form → `HERE_PWD_UNAVAILABLE` (same as
  `--status` no-id today).

## Testing (TDD red→green per repo rules)

Pure functions are unit-tested on `&str` literals — no shared fixtures, no
hardcoded paths (parallel-safe per CLAUDE.md):

- `parse_task_created`: well-formed line, missing prefix, subject containing
  `:` and `#`, multibyte subject.
- `extract_tasks`: create-only (all pending), create+updates, out-of-order
  updates, update referencing unknown id (ignored), unknown status passes
  through.
- `extract_todowrite_latest`: single snapshot, multiple snapshots (last wins),
  empty list, multibyte content.
- `extract_agent_list`: tasks-present prefers tasks, todos-only, neither →
  `None`.
- Label truncation: Japanese / emoji / accented inputs (no panic, correct
  width).
- Subcommand routing parser used by the shell function (Rust-level decision
  table), and a shellcheck pass on the emitted `SHELL_CODE`.
- Display + JSON formatters: golden-string assertions for a representative
  `SessionTodos` (with and without `--verbose`).

Any temp files used by integration-style tests must be keyed on
`std::process::id()` + nanos.

## Out of scope

- Live-process precedence (there is no live source for todo/task state the way
  there is for conversational status; the transcript is authoritative).
- Watching/streaming todo changes live.
- Editing or writing todo/task state — read-only.
- Nested subagents beyond the flat `subagents/` directory.
```
