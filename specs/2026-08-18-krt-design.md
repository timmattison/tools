# krt — Knights of the Round Trip (design)

**Date:** 2026-08-18
**Status:** Approved (brainstorming complete)
**Branch:** `krt`

## 1. Purpose

`krt` records the network path to a destination, hop by hop, for as long as you let it
run. It writes every probe result to a JSONL file for later analysis, and it shows a
live table of every hop while it runs.

The tool exists for one job that no tool in this repository does today: **a long,
unattended, append-only recording of a path**, in a shape that DuckDB or a dataframe
reads without help. You start it before a flaky call, you leave it, and afterward you
have the evidence.

### Goals

- One continuous tracer that gives both the route and the latency of every hop.
- An append-only JSONL file, flushed every round, named after the source and the
  destination.
- A live table with the last value and the aggregates (loss, min, average, maximum,
  standard deviation) for every hop.
- Correct behavior when the path flaps: two routers at one hop position stay separate.
- A `--replay` mode that folds a recorded file through the same code and prints the
  same table.

### Non-goals (these go to `WISHLIST.md`, which section 15 creates)

- Several destinations in one run.
- Autonomous System (AS) number lookup.
- File rotation and compression.
- An alert when a metric crosses a threshold.
- Path MTU discovery.
- Replay through the live table with playback controls.
- A web view or a remote view.

## 2. Prior art, and why this tool still exists

[Trippy](https://trippy.rs) is a network diagnostic tool with an `mtr`-style terminal
view. Its engine is on crates.io as `trippy-core` 0.13.0 under Apache-2.0. That engine
already solves the hard part: ICMP, UDP, and TCP probes, IPv4 and IPv6, continuous
rounds, ECMP strategies, and per-hop statistics.

Trippy does **not** stream an append-only record forever. Its JSON report mode runs a
fixed number of rounds and then prints one document. `krt` fills that gap.

So `krt` takes `trippy-core` as its tracer and owns the four things that gap needs: the
JSONL schema, the file naming, the aggregate fold, and the table. `krt` writes no ICMP
plumbing.

## 3. The one change from the original request

The request asked for two mechanisms: a traceroute every 30 seconds, and a separate
continuous ping of every host that the traceroute found.

These are one mechanism. A traceroute round sends a TTL-limited probe to every hop and
reads the reply. That reply **is** a ping of that hop. A second probe engine adds
packets and adds code, and it measures the same thing.

So `krt` runs one tracer. Each round gives the route and one latency sample for every
hop. The 30-second number becomes `--interval`, the round period, which sets the
traceroute rate and the ping rate together.

### Why the default interval is 1 second

| `--interval` | Samples per hop per minute | File growth, 20-hop path |
| ------------ | -------------------------- | ------------------------ |
| `1s`         | 60                         | ~85 MB per day           |
| `5s`         | 12                         | ~17 MB per day           |
| `30s`        | 2                          | ~3 MB per day            |

One sample per hop per 30 seconds is too thin to call ping data. A loss rate needs
samples, and a standard deviation over two samples per minute means little. So the
default is `1s`, which is also the `mtr` default, and `--interval` is the knob for a run
that must last for days.

## 4. Architecture

```
                    +--------------------------------------+
   trippy-core ---> | trace.rs   the only trippy consumer   |
   (tracer thread)  | Round<'_>  ->  krt::record::Round     |
                    +------------------+-------------------+
                                       | channel (owned records)
                                       v
                    +--------------------------------------+
                    | main.rs    the run loop               |
                    +---+-------------------------------+--+
                        |                               |
                        v                               v
          +-------------------------+     +---------------------------+
          | record.rs  JSONL writer |     | stats.rs  pure fold       |
          |  append, flush, end     |     |  Round -> HopTable        |
          +-------------------------+     +-------------+-------------+
                        ^                               |
                        | (--replay reads)               v
          +-------------------------+     +---------------------------+
          | record.rs  JSONL reader |     | ui.rs   HopTable -> frame |
          +-------------------------+     +---------------------------+
```

### 4.1 `trace.rs` is a hard wall

The documentation of `trippy-core` says: *"the public API is not stable and is highly
likely to change in the future."* Two rules answer that risk.

1. The manifest pins the exact version: `trippy-core = "=0.13.0"`.
2. **No module except `trace.rs` names a trippy type.** The tracer callback receives a
   borrowed `Round<'_>` and converts it into an owned `krt::record::Round` before it
   sends it. Nothing borrowed crosses the channel.

The interface is one function and one type:

```rust
/// Configuration for one tracing run, in krt's own vocabulary.
pub struct TraceConfig { /* target, interval, ttl range, protocol, family, privilege */ }

/// Start the tracer on its own thread and return a receiver of completed rounds.
///
/// # Errors
/// Returns an error when the target will not resolve, when the platform needs
/// privileges that the process does not hold, or when the socket will not open.
pub fn spawn(config: &TraceConfig) -> Result<Receiver<record::Round>, TraceError>;
```

A future engine swap touches one file. This is the depth that
[CLAUDE.md](../CLAUDE.md) asks for: a narrow entrance in front of a large body of hidden
work.

### 4.2 `stats.rs` folds krt records, not tracer state

`trippy_core::State` already computes loss, best, worst, average, standard deviation,
and jitter. `krt` does not use it.

The reason is the `--replay` mode. If the live aggregate comes from tracer state, then
reading the aggregate back out of a recorded file needs a second implementation, and the
two drift. One fold, fed live by the channel and offline by the file reader, gives
identical numbers in both directions.

That fold is a pure function, so it tests with no network and no privileges:

```rust
/// The aggregate view of every hop seen so far.
pub struct HopTable { /* rows, keyed and ordered by ttl */ }

impl HopTable {
    /// Fold one more round into the table.
    pub fn observe(&mut self, round: &record::Round);
}
```

## 5. Privileges

These facts come from the source of `trippy-privilege` 0.13.0, not from memory.

| Platform | Needs privileges? | Reason |
| -------- | ----------------- | ------ |
| macOS    | No                | It sends through `IPPROTO_ICMP` sockets with the `IP_HDRINCL` socket option. |
| Linux    | Yes               | It supports `IPPROTO_ICMP` but not `IP_HDRINCL`, so it needs `CAP_NET_RAW`. |
| Windows  | Yes               | It needs an elevated token. |

At startup `krt` calls `Privilege::acquire_privileges()` and then reads
`has_privileges()` and `needs_privileges()`.

- The platform does not need privileges: run with `PrivilegeMode::Unprivileged`.
- The platform needs privileges and holds them: run with `PrivilegeMode::Privileged`.
- The platform needs privileges and does not hold them: print the remedy for that
  platform and exit with code 2.

The remedy text is platform-specific and exact:

```
krt: this platform needs raw socket privileges to send probes.
  Linux:   sudo setcap 'cap_net_raw+p' $(which krt)
  Windows: run krt from an elevated prompt
```

`krt` never falls back to a degraded trace without saying so.

## 6. The JSONL schema

### 6.1 File rules

The file opens in **append** mode. One source and one destination therefore keep one
file across many runs. The `run` field separates the runs inside it.

The writer flushes after every record. A `kill -9` loses at most one round.

A reader **must ignore a `type` value that it does not know**. A new record type in a
later version therefore stays backward compatible.

### 6.2 The four record types

```jsonc
// Written once when a run starts.
{"type":"run","run":"2026-08-18T12:00:00.123Z","krt":"0.1.0 (abc1234, clean)",
 "source":{"addr":"1.2.3.4","kind":"public"},
 "target":{"arg":"example.com","addr":"93.184.216.34","family":"ipv4"},
 "config":{"interval_ms":1000,"protocol":"icmp","first_ttl":1,"max_ttl":30,
           "multipath":"classic","privilege":"unprivileged","dns":true},
 "host":"tims-mac"}

// Written when a reverse DNS lookup finishes. Never repeated for the same address.
{"type":"name","run":"2026-08-18T12:00:00.123Z","ts":"2026-08-18T12:00:02.001Z",
 "addr":"192.168.1.1","host":"router.lan"}

// Written once per round.
{"type":"round","run":"2026-08-18T12:00:00.123Z","seq":142,
 "ts":"2026-08-18T12:34:56.789Z","dur_ms":1004,
 "ttl_range":[1,14],"reached":true,
 "hops":[{"ttl":1,"addr":"192.168.1.1","rtt_ms":1.23,"icmp":"time_exceeded"},
         {"ttl":14,"addr":"93.184.216.34","rtt_ms":24.10,"icmp":"echo_reply"}]}

// Written on a clean quit.
{"type":"end","run":"2026-08-18T12:00:00.123Z","ts":"2026-08-18T13:00:00.000Z",
 "rounds":1420,"reason":"quit"}
```

### 6.3 Two decisions inside that shape

**A hop that did not answer is absent.** `ttl_range` says which TTLs the round probed.
So the sent count of a TTL is the number of rounds whose range covers it, and the lost
count is that number minus the answers. A timeout therefore costs zero bytes, and a
30-hop path with 3 answers writes a short line.

**A name lives in its own record.** Reverse DNS arrives late and rarely changes. A name
on every round line repeats the same string thousands of times per hour.

### 6.4 The `run` identifier

The `run` field is the RFC 3339 start timestamp of the run, with milliseconds, in UTC.
It sorts, it reads, and it needs no new dependency. A `uuid` adds a dependency and reads
worse in a file you open by hand.

## 7. The aggregate model

### 7.1 Keys

A hop position can answer from more than one address, because of load balancing or
because the route changed. So the table holds two levels.

- A **TTL row** for each hop position. Its statistics cover every answer at that TTL.
  It answers "how is this position on the path behaving".
- An **address row** under a TTL that saw more than one address. Its statistics cover
  that one router. It answers "which router, and how is each one behaving".

This is why the loss column differs between the two levels, and the difference is not
cosmetic:

- The TTL row shows **Loss%**: `(sent - answered_at_this_ttl) / sent`. This is the true
  loss of that position.
- An address row shows **Share%**: `answered_by_this_addr / answered_at_this_ttl`. A
  loss percentage per address is misleading, because two routers that split the traffic
  evenly each look like 50 percent loss when they lose nothing.

The TTL row does mix two routers into one average. That mix is honest here, because the
address rows sit directly under it and show the split. The rejected alternative hid the
split and showed only the mixed number.

### 7.2 The statistics

For each key, over the round-trip times observed:

| Field    | Definition |
| -------- | ---------- |
| `sent`   | Rounds whose `ttl_range` covered this TTL. |
| `recv`   | Answers received. |
| `last`   | The most recent round-trip time. |
| `min`    | The smallest round-trip time. |
| `avg`    | The arithmetic mean. |
| `max`    | The largest round-trip time. |
| `stddev` | The population standard deviation, by Welford's online algorithm. |
| `jitter` | The absolute difference between the last two round-trip times. |

Welford's algorithm keeps the mean and the standard deviation in constant memory and
stays numerically stable over millions of samples. A naive sum of squares loses
precision on a long run, which is the run this tool is for.

### 7.3 Memory over a long run

The aggregates are constant in size per key, and the number of keys is bounded by the
TTL range times the number of distinct routers. The sparkline keeps a ring buffer of the
last 60 round-trip times per key. So a run of any length holds a bounded amount of
memory. Nothing in the live path grows without limit.

## 8. The table

```
 krt  example.com → 93.184.216.34   src 1.2.3.4   round 142   1s   1.2.3.4-example.com.jsonl (2.1 MB)

 TTL  Host                             Loss%   Sent   Last    Min    Avg    Max  StDev  Recent
   1  router.lan (192.168.1.1)          0.0%    142    1.2    0.9    1.3    4.5    0.4  ▁▁▂▁▁▁█▂▁
   2  ???                             100.0%    142      -      -      -      -      -
   3  10.0.0.1                          0.7%    142    8.4    7.9    9.1   40.2    3.1  ▂▂▁▂▃▂▁▂▂
   7  ae-1.core.example.net (+1)        1.4%    142   18.2   17.6   18.9   23.0    1.0  ▃▃▄▃▃▃▄▃▃
      ├ ae-1.core.example.net           51.4%▹   72   18.2   17.6   18.5   22.1    0.9  ▃▃▄▃▃▃▄▃▃
      └ 203.0.113.9                     48.6%▹   68   18.7   17.9   19.2   23.0    1.1  ▃▄▃▃▄▃▃▄▃
  14  example.com (93.184.216.34) ★     0.0%    142   24.1   23.0   24.8   61.0    2.2  ▃▃▃█▃▃▃▃▃
```

- `★` marks the destination.
- `(+1)` on a TTL row says how many more addresses that position saw.
- `▹` marks an address row. On such a row the percentage column is Share%, not Loss%,
  and the Sent column holds the answer count of that one router, not a probe count.
  The example adds up: TTL 7 sent 142 probes and lost 2, so 140 answers split 72 and 68.
- `???` is a position that never answered.
- The sparkline uses the Unicode block elements that [CLAUDE.md](../CLAUDE.md) requires:
  `▁▂▃▄▅▆▇█`. There is no ASCII fallback.

### 8.1 Keys

| Key      | Action |
| -------- | ------ |
| `q`, `Ctrl-C` | Write the `end` record, flush, restore the terminal, exit 0. |
| `p`      | Pause the table. The recording continues. |
| `n`      | Toggle names and raw addresses. |
| `r`      | Reset the aggregates. The file is untouched. |
| `?`      | Show the help overlay. |

### 8.2 Width

The table adapts to the terminal width. The Host column absorbs the change, and a name
too long for it truncates by character, never by byte. Display width comes from
`unicode-width`, so a name with wide glyphs still lines up. This obeys the UTF-8 safety
rule in [CLAUDE.md](../CLAUDE.md).

## 9. The command line

```
krt <destination> [OPTIONS]
```

| Flag | Default | Meaning |
| ---- | ------- | ------- |
| `-o`, `--output <FILE>` | derived | The JSONL path. Overrides the derived name. |
| `-i`, `--interval <DUR>` | `1s` | The round period. Accepts `500ms`, `1s`, `2m`. |
| `--first-ttl <N>` | `1` | The first TTL to probe. |
| `--max-ttl <N>` | `30` | The last TTL to probe. |
| `--protocol <P>` | `icmp` | `icmp`, `udp`, or `tcp`. |
| `--multipath <M>` | `classic` | `classic`, `paris`, or `dublin`. UDP and TCP only. |
| `-4`, `-6` | auto | Force the address family. |
| `--no-dns` | off | Skip reverse DNS. Show addresses only. |
| `--source <IP>` | discovered | Override the source label in the derived filename. |
| `--headless` | off | No table. Print one status line per minute. |
| `--duration <DUR>` | none | Stop after this much time. |
| `--rounds <N>` | none | Stop after this many rounds. |
| `--replay <FILE>` | none | Fold a recorded file and print the table. Then exit. |
| `--run <ID>` | the last run | With `--replay`, pick which run in the file to fold. |
| `-V`, `--version` | | `krt 0.1.0 (abc1234, clean)`, through `buildinfo`. |

`--headless` serves a `nohup`, a `launchd` job, or a `systemd` unit. `--replay` reads
the **last** run in the file by default, and `--run <ID>` picks another.

## 10. The source label and the filename

The default filename is `SOURCE-DESTINATION.jsonl` in the working directory.

`krt` finds SOURCE in this order:

1. The `--source` value, when given. The record marks this source `"kind":"override"`.
2. One HTTPS GET to a public IP service at startup, with a 3-second timeout. The record
   marks this source `"kind":"public"`.
3. The local egress address. `krt` opens a UDP socket toward the destination and reads
   its own local address. **No packet leaves the machine for this step.** The record
   marks this source `"kind":"local"`.

Step 3 never fails in a way that stops the run, so a captive network or an air-gapped
network still records. A fallback prints one warning line before the table starts.

Both halves of the name are sanitized: `:`, `/`, `\`, and whitespace each become `-`.
An IPv6 source therefore gives `2001-db8--1-example.com.jsonl`.

**Note on privacy.** The default name carries your public IP address. A file you share
carries it too. `--output` avoids this.

## 11. Errors and shutdown

| Event | Behavior |
| ----- | -------- |
| A write to the JSONL file fails | **Fatal.** Restore the terminal, print the error, exit 3. |
| The destination will not resolve | Exit 1 with the resolver error, before the terminal changes. |
| The platform needs privileges it does not hold | Exit 2 with the platform remedy. |
| The public IP lookup fails | Not fatal. Fall back to the local address and warn once. |
| A reverse DNS lookup fails | Not fatal. Show the raw address. |
| The tracer thread dies | Fatal. Write an `end` record with `"reason":"error"`, then exit 4. |
| A panic anywhere | A panic hook restores the terminal before the message prints. |

A failed write is fatal because the recording is the whole purpose of the tool. A run
that keeps a pretty table while it silently records nothing is worse than a run that
stops.

The terminal is restored by an RAII guard, so a normal exit, an error exit, and a panic
all take the same path.

## 12. Dependencies

| Crate | Why |
| ----- | --- |
| `trippy-core` (`=0.13.0`) | The tracer. Pinned, because its API is declared unstable. |
| `trippy-privilege` | The privilege matrix in section 5. |
| `trippy-dns` | A cheaply cloneable, non-blocking, caching reverse resolver. Default method: the system resolver. |
| `ratatui`, `crossterm` | The table. Already workspace dependencies. |
| `clap`, `serde`, `serde_json`, `chrono`, `anyhow`, `thiserror` | Already workspace dependencies. |
| `reqwest` with the `blocking` feature | One HTTPS GET for the public IP. |
| `unicode-width` | Display width for the Host column. |
| `buildinfo` | `--version`, as every tool in this repository does. |

`krt` writes **no async code and starts no runtime of its own**. `trippy-core` runs on
its own thread and calls back once per round, and `trippy-dns` is synchronous and
non-blocking. The one HTTPS GET uses the blocking client of `reqwest`, which keeps its
`tokio` runtime private and drops it when the call returns. An async main function for
one startup request is not worth its weight.

New workspace entries: `trippy-core`, `trippy-privilege`, and `trippy-dns` go in
`[workspace.dependencies]` under a new `# Network tracing` heading. `reqwest` already
exists there, and the crate adds the `blocking` feature locally, because cargo features
are additive.

## 13. Module layout

```
src/krt/
  Cargo.toml
  src/
    main.rs     CLI, wiring, the run loop, the terminal guard, the signal handler
    trace.rs    the only module that names a trippy type
    record.rs   the schema types, the writer, the reader
    stats.rs    the pure fold, HopTable and HopStats
    source.rs   the public IP discovery and the filename derivation
    ui.rs       the ratatui render of a HopTable
```

Every target root declares a position on the lints its crate raises, and the manifest
carries `[lints] workspace = true`. `repo_guards::workspace_lints` and
`repo_guards::target_lints` both fail `cargo test` otherwise.

## 14. Test plan

The work is test-first, red then green, per `~/.claude/TESTING.md`. **No test
touches the network, and no test needs privileges.** Every temporary path keys on the
process id plus a nanosecond timestamp, per the parallel-safety rule in
[CLAUDE.md](../CLAUDE.md).

### `stats.rs` — the fold

- Loss: a hop inside `ttl_range` that never answers reaches 100 percent.
- Loss: a hop that answers 9 rounds out of 10 reaches 10 percent.
- `min`, `avg`, `max`, and `last` over a known sample set.
- `stddev`: Welford against a hand-computed value.
- `jitter`: the absolute difference of the last two samples.
- Two addresses at one TTL: the TTL row aggregates both, and the two address rows carry
  the correct Share% that sums to 100 percent.
- A hop that answers and then stops: the aggregates keep the history and the loss climbs.
- A `ttl_range` that shrinks when the target moves closer: a TTL outside the range does
  not count as lost.

### `record.rs` — the schema

- A round trip through serde for each of the four record types.
- A committed golden fixture file, so a schema change fails a test instead of a reader.
- An unknown `type` value parses without error and is skipped.
- A truncated final line is reported, and the records before it still load.

### `source.rs` — the name

- `1.2.3.4` and `example.com` give `1.2.3.4-example.com.jsonl`.
- An IPv6 source gives `2001-db8--1-example.com.jsonl`.
- A destination typed as `https://example.com/path` sanitizes to a legal name.
- `--output` wins over every derivation.

### `ui.rs` — the render

- A fixed `HopTable` renders to expected glyphs.
- A multi-byte hostname truncates by character, and never panics. The cases are
  Japanese, an emoji, and an accented name, per the UTF-8 rule.
- A narrow terminal drops columns in a defined order and still lines up.

### End to end

- `--replay` over a committed fixture prints an expected table and exits 0.
- `--replay` over a file with two runs reads the last run, and `--run` picks the first.
- A round stream that ends with no rounds still writes a `run` record and an `end`
  record, and nothing between them.

### The mutation test

The enforced-helper rule asks for proof that a guard can fail. The golden fixture is
the guard on the schema, and a guard that silently matches anything is worse than no
guard. So one test builds a copy of the fixture with one field renamed, runs the same
comparison over that copy, and asserts that the comparison reports a mismatch.

## 15. Definition of done

1. `src/krt` builds, and `cargo test`, `cargo clippy`, `cargo fmt --check`, and
   `cargo machete` are clean at the workspace level.
2. `krt --version` prints `krt 0.1.0 (<hash>, <clean|dirty>)`.
3. A live run against a real destination writes a file that `--replay` reads back, and
   the two tables match.
4. `README.md` **and** `TLDR.md` both list `krt`. README appends, TLDR is alphabetical.
   Nothing enforces this, so it is a checklist item.
5. `WISHLIST.md` holds the non-goals from section 1.

## 16. Open risks

| Risk | Response |
| ---- | -------- |
| The `trippy-core` API changes and breaks the build on an upgrade. | The version is pinned exactly, and `trace.rs` is the only file to fix. |
| The public IP service goes away or rate-limits. | The fallback in section 10 keeps the run alive. The service URL is a constant, so a change is one line. |
| A 1-second interval writes 85 MB per day. | `--interval` is documented with the table in section 3. Rotation and compression are on the wishlist. |
| `trippy-core` is Apache-2.0 and this repository is MIT. | Apache-2.0 is permissive and imposes no copyleft on a binary that links it. The attribution belongs in the README entry. |
