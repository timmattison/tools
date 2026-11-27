# NLE plugin architecture design

This document answers issue #169 (branch `issue-169`), under the parent issue
#148. It defines how a third party writes a filter, a renderer, or an analyzer
for the NLE editor in `src/nle`. It contains no code. The issues of the
implementation come from [the slices](#the-implementation-slices) after this
document merges.

The plugin ABI is a contract that third parties build against. A change to that
contract after a release breaks their plugins. Thus the design comes before the
code.

**A rule in this document is a decision.** If the code of a slice disagrees
with a rule, the code or this document has an error. The slice corrects one of
the two. The WIT of the two filter worlds is the contract of slice 2. The WIT
sketches of the analyzer world and the renderer world show the direction, and
their slices make them final.

## The decisions

1. **The runtime is wasmtime with the WIT component model**, not Extism. The
   host pins the newest LTS line of wasmtime. [The runtime](#the-runtime) gives
   the comparison.
2. **The ABI is one WIT package, `nle:plugin`**, with one world for each plugin
   type: `filter-plugin`, `span-filter-plugin`, `analyzer-plugin`, and
   `renderer-plugin`. The two filter worlds match the two forms of a filter in
   `src/nle`: the event form `FilterFn` and the span form `SpanFilterFn`. Each
   world exports a versioned interface, thus the host reads the API version of
   a plugin from the plugin itself.
3. **One plugin is one folder, one manifest, one component, and one function.**
4. **Each call gets a new instance.** No state survives from one call to the
   next, and the same input always gives the same output. A failure at a
   limit is also the same on each machine, because fuel, not the clock,
   bounds the work of a call.
5. **The sandbox gives a plugin no file, no network, no environment, no real
   clock, and no real random source.** Limits bound its memory and its work. A
   wall-clock deadline is only a backstop.
6. **The event form `FilterFn` returns a `Result`, like the span form.** A
   plugin can fail, and the pipeline must report that failure. Both forms
   carry the error as a `String`. This change comes before the first built-in
   filter of the event form.
7. **The options are typed.** The manifest declares each option, the host makes
   sure of each value before the call, and typed values cross the boundary.
8. **The filter kind of a plugin is `plugin:<name>`.** A built-in kind never
   contains `:`, thus a plugin never takes the place of a built-in filter.
9. **A plugin loads from a local folder.** A URL installs a plugin into that
   folder, after the user accepts it. The editor never runs a plugin directly
   from the network.
10. **Hot reload applies only to development folders** that the user names
    with a flag.
11. **The API version stays below 1.0.0** until each of the four worlds
    ships. Below 1.0.0, the version obeys the convention
    of cargo: an addition increments the patch number, and a break increments
    the minor number. The semver promise comes at
    [the 1.0.0 gate](#the-100-gate).
12. **The six open questions** of the issue each get a recommendation in
    [The open questions](#the-open-questions).

## The current state

The design starts from the code on `main` at commit `9c12c0325`.

- `src/nle/src/pipeline.rs` defines two forms of a filter. The event form,
  `FilterFn`, is `dyn Fn(&[Event], &FilterOptions) -> Vec<Event> + Send + Sync`.
  The span form, `SpanFilterFn`, is
  `dyn Fn(Span, &FilterOptions) -> Result<Span, String> + Send + Sync`, and
  `Span` holds the events and the length of a region. `FilterRegistry` keeps
  one filter of either form for each `FilterKind`, from `register` or
  `register_span`. The doc comments state the steps of one filter call: the
  region split, the relative times, the shift, and the sort.
- A filter of the event form cannot report an error. A span filter answers
  `Err` with a message, and the pipeline then fails with
  `NleError::RenderError`, whose text names the kind and holds the message.
  For both forms, the pipeline refuses an answered time that is not finite or
  is below zero.
- `App` in `src/nle/src/ui.rs` starts with `filters::registry()` of
  `src/nle/src/filters.rs`. It registers one built-in filter: time dilation
  (issue #153), a span filter. No built-in filter of the event form exists
  yet. The other filters of issues #152 thru #168 are open.
- `FilterKind` is a string, and `FilterOptions` is a JSON object. The project
  file keeps both in `AppliedFilter`. The project file has no version field.
- `src/nle` has no terminal state, no frame, and no analysis type.
  `TerminalState` and `Cell` are in the library crate `termstate`
  (`src/termstate/src/lib.rs`), which issue #768 added. `beta` uses
  `termstate`, but `src/nle` does not depend on it yet. The video export of
  `beta` draws into an `RgbImage`. The preview pane (issue #151), the export of an edited
  recording (issue #158), and the chapter index (issue #164) are open.
- The workspace has no WASM runtime. It already uses `dirs`, `notify`, `sha2`,
  and `reqwest`. `rust-toolchain.toml` installs no WASM target.
- `scripts/rust-checks.sh` runs clippy and the tests without
  `--all-features`. Thus code behind a cargo feature that is off by default gets
  no lint and no test.

## The runtime

The issue names two runtimes: Extism, and wasmtime. Extism runs on wasmtime
too, thus the choice is about the layer above wasmtime. The other choice is
wasmtime with the WIT component model and no layer between.

### The facts on 2026-11-26

- `extism` 1.41.0 came out on 2026-10-09. It depends on `wasmtime ^49` and
  `wasmtime-wasi ^49`, with WASI preview 1 only. An Extism plugin is a core
  module that imports `extism:host/env`, not a component.
- `wasmtime` 51.0.0 came out on 2026-11-20. Wasmtime ships a major version on
  the 20th of each month. A normal version gets security fixes for 2 months. A
  version that 12 divides is an LTS version, and it gets security fixes for 24
  months. Now the versions 50 and 51, and the LTS versions 36 and 48, get fixes
  ([the release policy](https://docs.wasmtime.dev/stability-release.html)).
- Version 49 got its last patch, 49.0.7, on 2026-11-19. Thus the newest Extism
  runs on a wasmtime that gets no more security fixes. An Extism host can move
  to a new wasmtime only after Extism releases.
- WASI 0.3 is stable since 2026-06-11, and wasmtime has it on by default since
  version 46. The pinned toolchain of this repository, 1.98.0, builds
  `wasm32-wasip2`, which is WASI 0.2.

### The comparison

| Property | Extism 1.41.0 | wasmtime with WIT |
| --- | --- | --- |
| The contract | Bytes in and bytes out. The two sides agree on JSON or MessagePack by convention. Each language reads and writes that format by hand, or through `xtp-bindgen`, whose last commit is from 2025-02-12. | A WIT world. `wasmtime::component::bindgen!` and `wit-bindgen` generate typed code for both sides. The link refuses a component that does not match the world. |
| The API version | Not part of the ABI. A manifest field must carry it. | Part of each export name, for example `nle:plugin/filter@0.1.0`. The link of wasmtime knows semver. |
| The frame of a renderer | A copy of the full image into the plugin and back, for each frame. | A resource that the host owns. The plugin calls its methods, and no image crosses. |
| Security fixes | Only after an Extism release. Version 1.41.0 is on wasmtime 49, which gets no fixes. | The host takes each patch of wasmtime directly. |
| The sandbox | No path and no host when the manifest lists none. A `null` in `allowed_hosts` lets every host. An HTTP host function exists. | Only what the host links. The host links no file and no network interface. |
| Limits | `timeout_ms`, `memory.max_pages`, and a fuel limit | `StoreLimits`, an epoch deadline, and fuel |
| Calls from many threads | `Plugin::call` takes `&mut self`, and a `Pool` hands out plugins. | A new `Store` for each call, from one `InstancePre`, which is `Clone`, `Send`, and `Sync`. |
| Guest languages | Official PDKs for Rust, JavaScript, Python, Go, Haskell, AssemblyScript, .NET, C, C++, and Zig | Rust through `wasm32-wasip2` (tier 2 since Rust 1.82), Go through TinyGo and `wit-bindgen-go`, Python through `componentize-py`, C and C++ through `wit-bindgen` and wasi-sdk, JavaScript and TypeScript through `jco componentize` |
| TypeScript | The JavaScript PDK compiles to WASM with QuickJS-ng: ES2020, no event loop. | `jco componentize` embeds SpiderMonkey, which is about 8 MB in each plugin. |
| Dependencies | `wasmtime` and `wasmtime-wasi`, plus the Extism crates, `tracing-subscriber`, `toml`, `glob`, `uuid`, and `ureq` (optional) | `wasmtime` and `wasmtime-wasi` |

The official Go toolchain builds only `wasip1`, thus a Go author uses TinyGo
for a component.

### The choice

**wasmtime with the WIT component model.** The contract is the risk of this
issue, and WIT makes the contract a file that both sides compile against. A
plugin that does not match the contract fails at load, not in the middle of a
render. The API version is in the component, and the frame of a renderer
stays in the host. The security fixes of wasmtime reach the editor with no
wait.

Extism gives a faster start to authors in more languages, and smaller
TypeScript plugins. Its contract is a convention in prose, its frames copy,
and its wasmtime lags. The component toolchains are younger than the Extism
PDKs, and a TypeScript component is large. The Rust guest crate and the author
guide pay that cost for the first-class language.

**The host pins the newest LTS line of wasmtime**, now 48, which gets security
fixes until 2028-08-20. A normal version gets fixes for only 2 months, thus a
pin to it needs a major upgrade each month or two. The host moves to the next
LTS line, 60, after 2027-08-20.

## The ABI

### The package

All of the contract is one WIT package, `nle:plugin`. It holds these parts:

| Part | Kind | Content |
| --- | --- | --- |
| `types` | interface | The event, the options, and the shared records |
| `log` | interface | One function that writes a line to the plugin log of the editor |
| `filter` | interface | The `apply` function of a filter of the event form |
| `span-filter` | interface | The `span` record, and the `apply` function of a filter of the span form |
| `analyzer` | interface | The `analyze` function of an analyzer |
| `canvas` | interface | The `frame` resource that a renderer draws on |
| `renderer` | interface | The `render` function of a renderer |
| `filter-plugin` | world | Imports `log`, exports `filter` |
| `span-filter-plugin` | world | Imports `log`, exports `span-filter` |
| `analyzer-plugin` | world | Imports `log`, exports `analyzer` |
| `renderer-plugin` | world | Imports `log` and `canvas`, exports `renderer` |

The worlds export an interface, not a bare function. The name of an exported
interface carries the package version, for example
`nle:plugin/filter@0.1.0`. Thus the host reads the API version of a component
from its export names before it instantiates the component. The host refuses a
version that it does not know, with a message that names both versions.

The WIT files live in the host crate, `src/nle-plugins/wit/`. The Rust guest
crate reads the same files. There is one copy.

`wasm-tools` 1.263.0 parses all of the WIT blocks of this document as one
package. A dummy component of each world, from `wasm-tools component embed
--dummy`, exports `nle:plugin/filter@0.1.0`, `nle:plugin/span-filter@0.1.0`,
`nle:plugin/analyzer@0.1.0`, or `nle:plugin/renderer@0.1.0`.

### The shared types

```wit
package nle:plugin@0.1.0;

interface types {
    /// The kind of a recording event. It matches `EventType` of beta.
    enum event-kind { output, input }

    /// One event. `time` is in seconds. A filter and an analyzer see times
    /// relative to the start of their region.
    record event {
        time: f64,
        kind: event-kind,
        data: string,
    }

    /// The value of one option. The manifest declares its type.
    variant option-value {
        number(f64),
        integer(s64),
        boolean(bool),
        text(string),
        choice(string),
    }

    record option-entry {
        key: string,
        value: option-value,
    }
}

interface log {
    enum level { debug, info, warn, error }

    /// Writes one line to the plugin log of the editor.
    write: func(level: level, message: string);
}
```

The host gives every option that the manifest declares, with its default when
the project sets no value. Thus a plugin never needs a default of its own.

### The filter worlds

This WIT is the contract of slice 2. A filter has one of the two forms of
`src/nle/src/pipeline.rs`, and each form has its own world. The `type` of the
manifest picks the form.

The event form is for a filter that changes the events of its region:

```wit
interface filter {
    use types.{event, option-entry};

    /// The filter. It obeys the steps on `FilterFn` in
    /// `src/nle/src/pipeline.rs`: it gets the events of its region with
    /// times relative to the region start, and it answers new events in the
    /// same relative time. The order of the answer does not matter.
    ///
    /// An `err` answer fails the render with the message.
    apply: func(events: list<event>, options: list<option-entry>) -> result<list<event>, string>;
}

world filter-plugin {
    import log;
    export filter;
}
```

The span form is for a filter that changes the length of its region, for
example a change of speed (issue #153), a pause insertion (issue #154), or an
idle time compression (issue #167). The event form cannot change the length
of a region that holds no event.

```wit
interface span-filter {
    use types.{event, option-entry};

    /// A region of the timeline.
    record span {
        /// The events of the region, with times relative to its start.
        events: list<event>,
        /// The length of the region in seconds.
        length: f64,
    }

    /// The span filter. It obeys the steps on `SpanFilterFn` in
    /// `src/nle/src/pipeline.rs`: it gets the events and the length of its
    /// region, and it answers new events and a new length. The pipeline
    /// moves the events after the region by the change in the length.
    ///
    /// An `err` answer fails the render with the message.
    apply: func(region: span, options: list<option-entry>) -> result<span, string>;
}

world span-filter-plugin {
    import log;
    export span-filter;
}
```

The host wraps the component in a closure, and it registers that closure in
`FilterRegistry` like a built-in filter: a `filter` plugin through
`FilterRegistry::register`, and a `span-filter` plugin through
`FilterRegistry::register_span`. Thus the pipeline and the editor do not know
that a filter is a plugin. The region split, the time checks, the shift, and
the sort stay in the pipeline.

### The analyzer world

This sketch shows the direction. Slice 5 makes it final.

```wit
interface analyzer {
    use types.{event, option-entry};

    /// A point on the timeline, for example the start of a command.
    record marker {
        time: f64,
        label: string,
    }

    /// A named number, for example the count of the commands.
    record metric {
        name: string,
        value: f64,
        unit: option<string>,
    }

    record analysis {
        markers: list<marker>,
        metrics: list<metric>,
    }

    /// Reads the events of the whole timeline. It changes nothing.
    analyze: func(events: list<event>, options: list<option-entry>) -> result<analysis, string>;
}

world analyzer-plugin {
    import log;
    export analyzer;
}
```

The issue gives an analyzer no options. This design gives it options, because
the manifest of every plugin type declares options, and the settings panel
works the same for each type.

The editor shows the markers on the timeline (issue #150) and in the chapter
index (issue #164). It shows the metrics in a panel.

### The renderer world

This sketch shows the direction. Slice 6 makes it final.

```wit
interface canvas {
    record rgba { r: u8, g: u8, b: u8, a: u8 }
    record rect { x: s32, y: s32, width: u32, height: u32 }
    record text-style { size: f32, color: rgba, bold: bool }

    /// The frame of the host: an RGBA image that the host owns.
    resource frame {
        width: func() -> u32;
        height: func() -> u32;
        /// The pixels of `area`, 4 bytes for each pixel, row by row.
        read-pixels: func(area: rect) -> list<u8>;
        /// Blends `pixels` onto `area` with their alpha.
        write-pixels: func(area: rect, pixels: list<u8>) -> result<_, string>;
        fill-rect: func(area: rect, color: rgba);
        /// Draws text with the font of the host.
        draw-text: func(x: s32, y: s32, text: string, style: text-style);
    }
}

interface renderer {
    use types.{event, option-entry};
    use canvas.{frame};

    record rgb { r: u8, g: u8, b: u8 }
    flags cell-style { bold, italic, underline }

    record cell {
        ch: char,
        fg: rgb,
        bg: rgb,
        style: cell-style,
    }

    /// The screen at the time of the frame.
    record terminal-state {
        columns: u32,
        rows: u32,
        /// The cells, row by row.
        cells: list<cell>,
        /// The cursor as (column, row), when it shows.
        cursor: option<tuple<u32, u32>>,
    }

    record render-context {
        /// The time of the frame on the timeline, in seconds.
        time: f64,
        /// The size of one cell in pixels.
        cell-width: u32,
        cell-height: u32,
        /// The input events of the last `lookback` seconds of the manifest.
        recent-input: list<event>,
    }

    render: func(
        state: terminal-state,
        frame: borrow<frame>,
        context: render-context,
        options: list<option-entry>
    ) -> result<_, string>;
}

world renderer-plugin {
    import log;
    import canvas;
    export renderer;
}
```

The frame is a resource that the host owns. The plugin draws through its
methods, thus no copy of the image crosses the boundary for each frame. The
plugin can write to no other place. Each method clips to the frame. `draw-text`
uses the font of the host, thus a captions plugin (issue #163) and a keystroke
overlay (issue #161) need no font of their own.

A renderer keeps no state between frames, because each frame gets a new
instance. The frame of a time is thus the same when the preview seeks to it and
when the export reaches it. An effect that changes with time computes from
`context.time` and `context.recent-input`.

### Failures

Each of these failures stops the call, and the render fails with a
`NleError::RenderError` that names the plugin, its version, and the cause:

- The plugin answers `err(message)`.
- The plugin traps, for example on a Rust panic.
- The call passes its fuel limit, its memory limit, or the deadline of the
  backstop.
- The component does not export the interface of its manifest `type`, or it
  exports a version that the host does not know.

A write past the bound of the log is not a failure. In one call, the host
keeps only the first 64 KiB of the bytes that the plugin writes through `log`,
standard output, and standard error together. The host drops each byte after
that bound. The write still returns at once with no error, and the call
continues. Thus a plugin that writes past the bound gives the same answer as
with no write. [Slice 2](#slice-2-the-wasm-filter-host) decides how the
editor keeps the plugin log of the editor and shows it.

## The host

The host is a library crate, `src/nle-plugins`. `src/nle` depends on it with no
cargo feature, because the checks of the workspace do not build a feature that
is off by default.

- **One `Engine` for each process**, with the Cranelift compiler, NaN
  canonicalization, fuel, and epoch interruption on. NaN canonicalization makes
  the float results of a plugin the same on each platform. The threads
  proposal and the relaxed SIMD proposal stay off, because each can make two
  runs give different results.
- **One `InstancePre` for each plugin.** The host compiles and links a plugin
  once, when it loads. A call makes a new `Store` and a new instance from the
  `InstancePre`, and drops both when the call ends. The closure that wraps a
  plugin is thus `Send + Sync`, which `FilterFn` and `SpanFilterFn` ask for.
- **An empty WASI context.** Some guest toolchains import WASI even when the
  plugin uses none of it. The host links WASI 0.2 with a `WasiCtxBuilder`
  context of `wasmtime-wasi` that has no preopened directory, no environment,
  and no arguments. The host also calls `allow_tcp(false)`, `allow_udp(false)`,
  and `allow_ip_name_lookup(false)`, because the documentation of wasmtime 51
  does not agree with itself about the socket defaults. The wall clock and the
  monotonic clock always give the same fixed time. The random sources give a
  fixed sequence. The contract needs no async function, thus it does not
  depend on WASI 0.3. When a guest toolchain imports WASI 0.3, the host links
  an empty context for it too.
- **No host function waits.** Fuel and an epoch deadline act only in WASM
  code, thus neither one stops a call that waits inside a host function. The
  clock functions of `wasmtime-wasi` wait on a real Tokio timer:
  `subscribe-duration` and `subscribe-instant` of WASI 0.2, and `wait-for` and
  `wait-until` of WASI 0.3. A duration that overflows waits forever. Thus,
  with these functions, a plugin that calls `std::thread::sleep` holds the
  render thread forever. After the host links WASI, it defines each of these
  clock functions again, with `Linker::allow_shadowing` on for those
  definitions only. Each new function returns at once: a pollable that it
  gives is ready at once, because the clock of a plugin does not move.
  Standard input is closed. Standard output and standard error go through one
  `StdoutStream` of the host, under the bound of the log of
  [the failures](#failures). The host does not use `MemoryOutputPipe`. When
  its buffer is full, its `check_write` answers `StreamError::Closed`, and the
  `print!` of Rust std panics on that error. Its `poll_write` for WASI 0.3 then
  writes no byte. The stream of the host accepts each write and never waits.
  Its `check_write` always gives a permit, its `write` never fails, and its
  `poll_write` for WASI 0.3 always answers the full length.
- **Limits.** A `StoreLimits` bounds the memory of each instance. Fuel bounds
  the work of each call. Fuel counts the WASM instructions of the call, and
  that count does not depend on the speed or the load of the machine. Thus a
  call that passes the fuel limit fails on each machine, at the preview and at
  the export. Fuel does not count the work of a host function. An epoch
  deadline is only a backstop for a call that spends its time in host
  functions that return, for example a loop that asks `get-random-bytes` for
  its limit of 64 MiB at each step. The epoch check runs in WASM code, thus
  the deadline stops such a call when a host function returns. Its bound is
  many times the time that the fuel limit takes on a slow machine. Only the
  backstop depends on the machine. A thread of the host increments the epoch.
  Slice 2 sets the three defaults, and the settings of the editor can change
  them.
- **A cache of compiled code.** A compile with Cranelift costs time at each
  start of the editor. The host turns on the compilation cache of wasmtime
  (the `cache` feature of the crate), with its directory under
  `dirs::cache_dir()`. The host does not call `Component::deserialize`, because
  that function is `unsafe`: it trusts the bytes that it reads. Compiled code
  also fits only the exact version of wasmtime that made it.

A new instance for each call costs time, and fuel makes the code of a plugin
slower. Slice 2 measures a render of 100,000 events through a plugin that
answers its input, with fuel on, and it records the result in the pull
request. If the instantiation is a large part of that time, slice 2 turns on
the pooling allocator.

## The manifest

Each plugin folder holds `plugin.json`. The example of the issue is valid:

```json
{
  "name": "my-filter",
  "version": "1.0.0",
  "type": "filter",
  "description": "Does something cool to your recording",
  "entry": "./plugin.wasm",
  "options": {
    "threshold": { "type": "number", "default": 3.0, "label": "Threshold (seconds)" },
    "mode": { "type": "enum", "values": ["fast", "slow"], "default": "fast", "label": "Mode" }
  }
}
```

### The fields

| Field | Necessary | Rule |
| --- | --- | --- |
| `name` | yes | 1 to 64 characters of `a-z`, `0-9`, and `-`. It starts with a letter. It is unique among the loaded plugins. |
| `version` | yes | A semver version of the plugin itself. The editor shows it, and the errors name it. |
| `type` | yes | `filter`, `span-filter`, `analyzer`, or `renderer`. The component must export the interface of this type. |
| `description` | yes | One line of text for the plugin list. |
| `entry` | yes | A relative path to the component, inside the plugin folder. An absolute path, a `..` part, and a symbolic link that leaves the folder are refused. |
| `options` | no | The options. An absent field is no options. |
| `lookback` | no | Only for a renderer: the seconds of `recent-input`. The default is 0. |

The editor refuses an unknown field. A typo in a field name thus fails at load,
and the plugin does not run with a default that its author did not mean.

### The options

Each key of `options` is the key of one option. A key is `a-z`, `0-9`, and `_`,
and it starts with a letter. Each option has a `type`, a `default`, and a
`label`, and it can have a `help` text. The types:

| Type | Value | Extra fields | Widget |
| --- | --- | --- | --- |
| `number` | a finite JSON number | `min`, `max`, `step` | a number field, or a slider when `min` and `max` are both set |
| `integer` | a JSON integer in the range of `s64` | `min`, `max` | a number field with step 1 |
| `boolean` | `true` or `false` | none | a checkbox |
| `string` | a JSON string | `max_length` | a text field |
| `enum` | one of `values` | `values`, a list of unique strings that is not empty | a select box |

The default must obey the type and its bounds. Else the plugin does not load.

`FilterOptions` in the project file stays a JSON object. Before each call, the
host makes the typed `list<option-entry>` from it:

1. Each declared key that the project does not set gets its default.
2. A key that the manifest does not declare fails the render, with a message
   that names the key and the plugin. A silent skip of the key changes the
   output of an old project with no notice.
3. A value of the wrong type, or outside its bounds, fails the render in the
   same way.

### The filter kind in a project

A filter plugin of either form registers under the kind `plugin:<name>`. A
project that uses
it keeps that kind in `AppliedFilter`. When the plugin is absent, the render
fails with the existing error of `FilterRegistry`, which names the kind. The
project does not keep the version of the plugin. A plugin is like a font in a
document: the project names it, and the machine that opens the project gives
it.

## Distribution

### The plugins directory

The editor loads each folder in `<data dir>/nle/plugins/` when it starts,
except a folder whose name starts with `.`.
`<data dir>` is `dirs::data_dir()`: `~/Library/Application Support` on macOS
and `$XDG_DATA_HOME` or `~/.local/share` on Linux. A plugin that fails to load
does not stop the editor. The editor shows each failed folder with its reason.
If two folders have the same `name`, neither loads, and the error names both
folders. A development folder of [hot reload](#hot-reload) is the one
exception.

### Load from a URL

A URL installs a plugin. It does not run one from the network.

1. The URL names a `plugin.json` on `https`. The editor refuses every other
   scheme, also as the target of a redirect.
2. The editor downloads the manifest and the component that `entry` names,
   relative to the URL. It reads at most 64 KiB of the manifest and at most
   64 MiB of the component. At a limit, it stops the read and refuses the
   install, thus a body with no end does not fill the memory.
3. The editor shows the name, the version, the type, the description, the
   options, the size, the SHA-256 of the component, and the source URL.
4. If the user accepts, the editor writes the folder into a temporary folder
   of the plugins directory, whose name starts with `.`. Then it renames the
   temporary folder into place in one step, and it loads the plugin. Thus a
   crash in the middle of an install leaves no half folder that loads. A file
   beside the manifest keeps the source URL and the SHA-256. If the user
   refuses, the editor writes nothing.
5. An update is a new install from the same URL, with the same confirmation.
   Before the rename, the editor renames the old folder to a name that starts
   with `.`. After the rename, it deletes the old folder.

The sandbox is the security boundary, not the source of the plugin. The
confirmation tells the user what they install, and the SHA-256 lets them
compare it with the one that the author publishes.

### Hot reload

`nle --plugin-dev <dir>` loads the plugin in `<dir>` in addition to the
installed plugins, and it watches `<dir>` with `notify`. The flag can occur
more than one time. When the manifest or the component changes, the editor
loads the plugin again and builds a new `FilterRegistry`. It gives the new
registry to the editor, and it renders the preview again. If the new version
fails to load, the editor keeps the old version and shows the error.

A development folder takes the place of an installed plugin with the same
`name`, and the plugin list shows that replacement. Thus an author can
develop a new version of a plugin that is also installed. The rule of two
folders with the same `name` applies inside the plugins directory, and among
the development folders.

The editor does not watch the plugins directory. Only the install flow of the
editor writes there, and that flow loads the plugin that it installs.

## Security

| Risk | Answer |
| --- | --- |
| A plugin reads or writes a file, or uses the network | The world imports no such interface, and the WASI context gives no directory and no socket. |
| A plugin reads the environment, for example a token | The WASI context gives no environment variable. |
| A plugin runs forever | The fuel limit stops the call. No host function waits, because each clock function of the WASI context returns at once. The epoch deadline stops a call that spends its time in host functions that return. |
| A plugin takes all of the memory | `StoreLimits` stops the growth of an instance. The host keeps at most 64 KiB of the log of one call. Slice 2 decides the bound of the plugin log of the editor across calls. |
| A plugin escapes through a defect of the runtime | The host pins an LTS line of wasmtime, which gets security fixes for 24 months. A known advisory against wasmtime must stop the checks, thus slice 2 waits for issue #740, which puts `cargo audit` in the one list of `scripts/rust-checks.sh`, at pre-push and in CI. |
| A renderer writes outside its frame | The frame resource is the only target, and each method clips to the frame. |
| A plugin puts HTML or JavaScript into the webview | A plugin gets no access to the webview. The settings panel comes from the options schema. |
| A URL serves a changed component | The install shows the SHA-256, the editor never downloads an update by itself, and the sandbox applies to the changed component too. |

## The open questions

The issue closes with six open questions. Each one gets a recommendation.

1. **Built-in filters as plugins: no.** The built-in filters stay native
   closures of the event form or the span form. The editor thus builds and
   runs with no WASM toolchain, a built-in filter costs no instantiation, and
   a debugger steps through it. The API gets its exercise from the other
   direction. The conformance test of slice 2 runs an example plugin of each
   form. The example span filter implements time dilation, the first built-in
   filter (issue #153). A test asserts that the plugin and the native filter
   give equal answers on one fixture.
2. **Versioning: the WIT package version is the API version.** The host reads
   it from the export names of the component, thus the manifest has no `api`
   field that can disagree with the component. Before 1.0.0, an addition
   increments the patch number and a break increments the minor number, as in
   cargo. Wasmtime links two `0.x` versions as compatible by the same rule. The
   author guide says that a `0.x` version can break plugins. From 1.0.0, a
   minor version only adds, and the host links the current major version and
   the major version before it. The version of the editor is not part of the
   contract. The `version` field of the manifest is the version of the plugin,
   for the user and for the errors.
3. **Marketplace or registry: no.** A registry is a service that needs
   hosting, review, and abuse handling. A curated list of plugins in the author
   guide, which grows through pull requests, comes first. A plugin installs
   from its URL. This question opens again when that list is too long for one
   page.
4. **Export formats from plugins: no.** An exporter writes files and often runs
   `ffmpeg`. The sandbox forbids both. The exports of issues #158 and #160 stay
   native. A later `exporter-plugin` world can take the rendered recording and
   answer bytes and a file extension, and the host writes the file. That world
   is an addition, thus it needs no change to the other worlds.
5. **TypeScript plugins: not first-class, and no JavaScript engine in the
   host.** An embedded engine, for example QuickJS or Boa, adds a second
   sandbox to keep secure, and a second way to call each plugin type. The WIT
   contract does not depend on a language, thus `jco componentize` already
   builds a TypeScript plugin as a component. That plugin holds about 8 MB of
   SpiderMonkey, and each call makes a new instance of that full JavaScript
   engine. Rust is the first-class guest language, with a guest crate and an
   example. Go, Python, C, and TypeScript authors use the component toolchains
   of their languages, and the author guide links them. This question opens
   again if users ask for TypeScript and the size of `jco` output stops them.
6. **A custom UI panel: no.** A Dioxus component is Rust code that compiles
   into the editor, thus a WASM plugin cannot give one. HTML or JavaScript from
   a plugin in the webview runs next to the interface of the editor, outside
   the sandbox. The editor generates the settings panel from the options
   schema. More layout control comes as more optional fields in the schema,
   for example groups and the order of the fields. The editor still draws
   every widget.

## The implementation slices

Each slice fits one review: at most 1,500 changed lines and 5 acceptance
criteria. Each slice keeps its documentation and its tests. Slices 0 and 1 can
start at once.

| Slice | Content | Comes after |
| --- | --- | --- |
| 0 | `FilterFn` returns a `Result` | nothing |
| 1 | The manifest and the options schema | nothing |
| 2 | The WASM filter host | slices 0 and 1, and issue #740 |
| 3 | The plugins directory in the editor | slice 2 |
| 4 | The generated settings panel | slices 1 and 3 |
| 5 | Analyzer plugins | slice 3 |
| 6 | Renderer plugins | slice 3, issue #151, and issue #158 |
| 7 | Install from a URL | slice 3 |
| 8 | Hot reload of development plugins | slice 3 |

### Slice 0: `FilterFn` returns a `Result`

- `FilterFn` is `dyn Fn(&[Event], &FilterOptions) -> Result<Vec<Event>, String> + Send + Sync`,
  and `FilterRegistry::register` takes a closure of that signature. The error
  type is the one of `SpanFilterFn`.
- An `Err` fails `FilterRegistry::run` and `FilterRegistry::render` as step 3
  of `SpanFilterFn` does: with `NleError::RenderError`, whose text names the
  kind and holds the message.
- The steps on `FilterFn` state the error step.

This slice is small, and it must land before the first built-in filter of the
event form. Each event filter that lands after it uses the new signature, and
no filter needs a second change. The time dilation filter is a span filter,
and this slice does not change it.

### Slice 1: The manifest and the options schema

A new library crate, `src/nle-plugins`, with no WASM runtime yet.

- It parses `plugin.json`, and it refuses each broken rule of
  [the fields](#the-fields) and [the options](#the-options) with an error that
  names the field.
- It makes the typed options from a `FilterOptions` by the three steps of
  [the options](#the-options).
- It has a test for each rule, with multi-byte text in a label, a description,
  and a `string` value.

### Slice 2: The WASM filter host

- The WIT package `nle:plugin@0.1.0` holds `types`, `log`, `filter`,
  `filter-plugin`, `span-filter`, and `span-filter-plugin`.
- The engine, the instances, the WASI context, the limits, and the cache obey
  [the host](#the-host).
- A function loads one plugin folder, and it answers a `FilterFn`, a
  `SpanFilterFn`, or a load error.
- Two example plugins in Rust, built for `wasm32-wasip2`, run in a conformance
  test: one of each form. The span filter implements time dilation. The test
  asserts these results:
  - The span filter and the native time dilation filter give equal answers on
    one fixture.
  - Two calls with the same input give the same answer.
  - A file open and a socket connect fail.
  - The clock gives the fixed time.
  - A sleep of one hour through `std::thread::sleep`, and a wait through
    `subscribe-instant` for one hour after the fixed time, each answer in
    less than 60 seconds. A thread of the test bounds each call, thus a wait
    on the real clock fails the test and does not hang it.
  - A trap, an endless loop, and a memory bomb each fail with an error that
    names the cause. The endless loop fails at the fuel limit, not at the
    deadline.
  - A plugin that writes 1 MiB to standard output, to standard error, and
    through the `write` function of `log` gives the same answer as with no
    write. The host keeps only the first 64 KiB of that call.
- An author guide tells a plugin author how to build, test, and install a
  filter plugin of each form in Rust.

One question stays open, and this slice decides it: how the editor keeps the
plugin log of the editor, and how a person reads it. The spec gives only the
bound of 64 KiB for each call. The pull request of the slice records the
answer, with these items:

- Where the editor keeps the plugin log of the editor, and if each plugin has
  a log of its own.
- The bound of the plugin log of the editor across calls.
- How the host copies the bytes of a call into the plugin log of the editor:
  the level of each `log` line, a character that the bound cuts, and a note of
  the bytes that the bound dropped.
- Where a person reads the plugin log of the editor.

The slice adds the `wasm32-wasip2` target to `rust-toolchain.toml`. The test
builds the example plugin into a fixed directory under `target/`, and the lock
of cargo keeps two test runs apart. The slice records in its pull request the
build time that wasmtime adds to a clean build. The conformance test runs in
the one list of `scripts/rust-checks.sh`, like each other test of the
workspace. If it is too slow for the pre-commit hook, the slice moves it to the
pre-push stage of that list, which issue #740 adds. The slice does not make a
second list.

The slice uses the newest LTS line of wasmtime, now 48. It takes the newest
patch of that line that is older than the 7-day publish cooldown. It turns off
each default feature of wasmtime that the host does not use.

### Slice 3: The plugins directory in the editor

- The editor loads each folder of [the plugins directory](#the-plugins-directory)
  when it starts, except a folder whose name starts with `.`.
- It registers each filter plugin under `plugin:<name>`, through
  `FilterRegistry::register` or `FilterRegistry::register_span` by its `type`.
- It shows each folder that failed to load, with the reason.
- Two folders with the same `name` both fail, and the error names both.

### Slice 4: The generated settings panel

- The panel shows one widget for each option, by the table of
  [the options](#the-options), with the label and the help text.
- A value that breaks its type or its bounds shows an error under its widget,
  and the edit does not apply.
- The panel adds a plugin filter with `Edit::AddFilter`, and it changes the
  options of one with `Edit::ReplaceFilter`. Thus undo and redo work.
- A `dioxus-ssr` test covers each widget type.

If no filter list exists in the editor when this slice starts, the slice adds a
small one.

### Slice 5: Analyzer plugins

- The WIT package gets the `analyzer` interface and the `analyzer-plugin`
  world, and its version goes to `0.1.1`. A filter plugin built for `0.1.0`
  still loads, and a test asserts it.
- The project keeps a list of applied analyzers with their options, like its
  list of filters. The field has a serde default, thus an old project file
  still loads.
- The editor runs each applied analyzer after each render, and it shows the
  markers and the metrics in a panel.
- When the timeline of issue #150 exists, the markers show on it.

### Slice 6: Renderer plugins

- The WIT package gets `canvas`, `renderer`, and `renderer-plugin`, and its
  version goes up by one patch number.
- The project keeps a list of applied renderers with their options. The field
  has a serde default, thus an old project file still loads.
- The host builds `terminal-state` from `termstate::TerminalState`. It gives
  the frame to each applied renderer, in the order
  of the list.
- The export of issue #158 runs the renderers on each frame.

If the preview of issue #151 draws in the webview with xterm.js, no RGBA frame
exists in Rust for the preview. The renderers then apply to the export only,
and the preview shows a notice. Issue #151 makes that choice. This slice obeys
it.

### Slice 7: Install from a URL

- The editor installs a plugin by the five steps of
  [load from a URL](#load-from-a-url).
- A test serves a plugin from a local `https` server with a test certificate.
  It covers the refusal of the user, a wrong scheme, a redirect to `http`, a
  body over each size limit, and an `entry` that leaves the folder.
- A temporary folder that an install leaves behind does not load at the next
  start, and a test asserts it.

### Slice 8: Hot reload of development plugins

- `nle --plugin-dev <dir>` loads and watches the plugin in `<dir>`, by
  [hot reload](#hot-reload).
- A change of the component gives a new render with the new plugin.
- A broken change keeps the old plugin and shows the error.
- A development folder takes the place of an installed plugin with the same
  `name`, and the plugin list shows that replacement.

### The 1.0.0 gate

The API goes to `1.0.0` when slices 2, 5, and 6 are on `main` and the example
plugins exercise all four worlds. From then, the host links the current major
version and the major version before it. This gate is a decision for Tim, not a
slice.

## What this design does not cover

- A plugin that adds an export format. See open question 4.
- The publication of the guest crate on crates.io. The author guide points at
  the git repository until that decision.
- Signed plugins. The SHA-256 of an install is a check of identity, not a
  signature.
