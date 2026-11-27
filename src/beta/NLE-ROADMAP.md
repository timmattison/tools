# NLE Editor Roadmap

Parent issue: [#148](https://github.com/timmattison/tools/issues/148)

## Dependency Graph

```
#170 + #761 (Dioxus scaffold) ──► #149 (Core engine) ─┬──► All filters (#152-#168)
                                                      ├──► #151 (Preview pane)
                                                      ├──► #158 (Export)
                                                      ├──► #157 (Multi-clip splicing)
                                                      └──► #169 (Plugin architecture)

#159 (Timeline research) ──────────► #150 (Timeline UI)

#170 + #761 (Dioxus scaffold) + #149 (Core engine) + #150 (Timeline UI) + #151 (Preview pane) = Minimum viable editor

#160 (Rendering pipeline) ─── Already exists. Needs only progress reporting.
```

## Phases

### Phase 0: Foundation (1-2 days)

| Issue | Description | Notes |
|-------|-------------|-------|
| #170 | Scaffold the desktop app | Sets up the dev tooling, the app shell with its layout regions, and the Rust functions that load and save recordings and run the existing exporters. #170 built the scaffold on Tauri 2 and React. |
| #761 | Move the scaffold to Dioxus | Dioxus 0.7 with the desktop (webview) renderer, in one Rust crate. The components call the Rust code directly, thus no IPC layer and no generated TypeScript types exist. `dx serve` runs the app, and `dx bundle` builds it. |
| #149 | Core editing engine | Done. The project model (clips, tracks, filters), the filter pipeline and its registry, the render step that flattens a project into one recording, the edit operations with undo and redo, and project save/load. The types are in `src/nle/src/types.rs`. The app keeps the `Editor` in its Dioxus state. |
| #159 | Research timeline UI components | Done. The findings, the frame times of a prototype, and the recommendation are in [specs/2026-11-26-nle-timeline-research.md](../../specs/2026-11-26-nle-timeline-research.md). No Dioxus crate or JavaScript library fits, thus #150 builds a custom timeline. |

### Phase 1: Quick Wins (3-5 days, all parallel)

Every filter below is a pure function `Event[] → Event[]`. They can all be developed and tested with zero UI — load a JSON recording, pipe through the filter, write a new JSON, play it with `beta play`.

All of these can be built **simultaneously** after Phase 0 types exist.

| Issue | Description | Effort | Notes |
|-------|-------------|--------|-------|
| #153 | Time dilation | Done | A span filter (`src/nle/src/filters/time_dilation.rs`): it divides the times and the length of a range by a speed, and the pipeline moves everything after by the change in the length. The speed panel of the properties panel applies it until the timeline can select a region (issue #823 and issue #785). |
| #154 | Pause removal/insertion | Done | `pause-removal`, `pause-insertion`, and `auto-trim` in `src/nle/src/filters.rs`. Each one is a span filter whose options parse as a Rust type, thus the render refuses bad options. Auto-trim cuts the time after the last event, and a pause in a range moves the events after the range also when no event of the range moves: each one changes the length of its region. A point past the end of the region by more than a microsecond would move events that are before the point, thus pause insertion refuses it. The microsecond is for the rounding of a range with decimal bounds. The click on the timeline that places a pause waits for issue #804, and the marker where a pause was removed waits for issue #805. |
| #155 | Typing speed normalization | Done | A span filter (`src/nle/src/filters/typing_speed.rs`): it puts the keys of each burst of typing one interval apart, at a rate in characters per second, with an optional jitter that repeats in each render. Each output keeps its delay after its key. A gap longer than the pause threshold and the gap after a line end stay. No panel applies it yet. |
| #167 | Idle time compression | Half day | Similar to #154 with additional grid-change heuristic. |
| #156 | Text replacement/redaction | Done | A span filter (`src/nle/src/filters/redaction.rs`) that replaces text in `event.data` and keeps each time. It joins the events of a stream, so a match can cross events and escape sequences, and it never searches or changes an escape sequence. Presets find emails, IP addresses, AWS keys, tokens, and hostnames. The redaction panel of the properties panel applies it. The timeline marks of the redactions wait for issue #818. |
| #168 | Cursor customization | 1 day | Done. The `cursor` filter (`src/nle/src/filters/cursor.rs`) rewrites DECTCEM, DECSCUSR, mode 12, and OSC 12 in the output events: honor, always visible, always hidden, or smart (hidden during a burst of output), and a shape, a blink, and a color. The render-only items wait for #768 and #151 in a follow-up issue: custom shapes, size, opacity, glow, an outline, a smooth blink, animation, the draw of the shape, the blink, and the color in the video export, CSS in the web export, and the editor preview. |
| #160 | Rendering pipeline progress | Half day | Add structured progress reporting to existing `beta export video`. |

### Phase 2: Viewer and Export (1 week)

At the end of this phase: load a recording, apply filters, preview the result, export it. Useful even without the timeline UI.

| Issue | Description | Notes |
|-------|-------------|-------|
| #151 | Terminal preview pane | Done. The preview replays the rendered recording through the emulator of `src/termstate` (issue #768), the emulator of the video export, and draws the cell grid as RSX (`src/nle/src/preview.rs`, `src/nle/src/ui/preview.rs`). Snapshots of the terminal let a seek replay only from the nearest snapshot. The pane has Play/Pause, Stop, a time readout, and the five themes of `beta export --theme`. A range input in the timeline region is the playhead. It is in the time of the render, after the filters, and the timeline of #150 shows project time, before the filters. A playhead on the timeline waits for a map between the two times. |
| #158 | Export edited recordings | Flatten timeline to Beta JSON, pipe to existing Rust exporter. |
| #164 | Chapter index | Manual markers first. Auto-detect commands is a future enhancement. |

### Phase 3: Timeline UI (2-3 weeks)

The big UI push. This is the most complex part of the project.

| Issue | Description | Notes |
|-------|-------------|-------|
| #150 | Timeline UI component | Part 1 is done: the timeline shows the clips on their tracks in project time, with a ruler of adaptive ticks, a zoom by buttons, keys, and Cmd or Ctrl with the wheel, a horizontal scroll, tiles of 512 px that each draw their event density as one SVG path, and a mark where the terminal size changes (`src/nle/src/timeline.rs`, `src/nle/src/ui/timeline.rs`). It takes the design of the #159 research ([specs/2026-11-26-nle-timeline-research.md](../../specs/2026-11-26-nle-timeline-research.md)). The follow-up issues of #150 hold the playhead on the timeline and its keys, the clip drag, the edge trim, the region selection and the cut, and the map from project time to the time of the render that the playhead waits for. |
| #157 | Multi-clip splicing | Start with hard cuts (clear screen between clips). Transitions later. Terminal size differences between clips need handling. |

### Phase 4: Advanced Filters (1-2 weeks, parallel)

These filters need deeper terminal state analysis or more complex heuristics.

| Issue | Description | Why harder |
|-------|-------------|-----------|
| #152 | Smart backspace removal | Done. The `backspace-removal` span filter (`src/nle/src/filters/backspace.rs`) removes a typed key and its erase (Backspace, Ctrl+W, Ctrl+U), with their output and their time, only when the screen of the `termstate` emulator after the erase equals the screen before the key. Thus the backspaces that a TUI writes in its output stay. |
| #162 | Output fast-forward | Manual region selection works immediately. Auto-detection (future) needs prompt heuristic. |
| #166 | Color scheme conversion | Operates at ANSI escape sequence level within `event.data`. Must remap palette codes without breaking other sequences. |
| #161 | Keystroke overlay / HUD | Done. The `keystroke-hud` screen filter (`src/nle/src/filters/keystroke_hud.rs`) reads the input events of its region, gives each special key a label, and draws the keys in a box with output events. It replays the events through the `termstate` emulator, thus it puts back the cells under the box and keeps the box in place when the output scrolls. |

### Phase 5: Polish and Extensions (ongoing)

| Issue | Description | Notes |
|-------|-------------|-------|
| #163 | Captions and annotations | Text overlays at timestamps. UI-heavy. |
| #165 | Terminal crop and zoom | Dynamic viewport requires deep escape sequence understanding. |
| #169 | Plugin architecture | Designed in [the plugin spec](../../specs/2026-11-26-nle-plugin-architecture-design.md): WASM components on wasmtime, with a WIT contract. The spec lists the slices of the implementation. Its slice 0 (the event form `FilterFn` returns a `Result`, like the in-place form and the span form) comes before the first built-in filter of the event form. |

## Difficulty Assessment

### Easiest (half day each)
- #153 (time dilation)
- #154 (pause removal)
- #155 (typing speed normalization)
- #167 (idle time compression)
- #160 (progress reporting on existing pipeline)

### Medium (1 day each)
- #156 (text replacement — ANSI-aware)
- #168 (cursor customization)
- #158 (export — flatten + pipe to existing code)

### Hard (multi-day to multi-week)
- #150 (timeline UI — DAW-style component, most complex UI work)
- #152 (smart backspace — needs terminal state analysis)
- #157 (multi-clip splicing — terminal state transitions between clips)
- #165 (crop/zoom — dynamic viewport)
- #169 (plugin architecture — API design, manifest, sandboxing)

## Shared Infrastructure

**Prompt detection** is needed by both #162 (output fast-forward) and #164 (chapter index) for their auto-detect enhancements. When implemented, it should be a shared utility. The simplest heuristic: the last output event before a user input event is likely a prompt. This won't work in TUI applications but covers standard shell workflows.

## Architectural Decisions

1. **Dioxus app**: One Rust crate, with the desktop (webview) renderer of Dioxus 0.7. The components call the Rust code directly, thus no IPC layer and no second copy of the types exist. The state of the app is Dioxus state (signals), which is Rust, thus the one source of truth is in Rust by construction. On 2026-11-25, Tim chose Dioxus in place of Tauri (issue #761).

2. **Filter interface**: `Fn(&[Event], &FilterOptions) -> Vec<Event>` is the core API (`FilterFn` in `src/nle/src/pipeline.rs`). A filter gets the events of its region, with times relative to the start of the region. The pipeline moves the events after the region by the change in the time of the last event of the region. A filter that changes the length of its region is a span filter (`SpanFilterFn`, `FilterRegistry::register_span`): it also gets the length of its region and answers a new length, and the pipeline moves the events after the region by the change in the length. It can also answer an error, for example for options that do not fit its region. Time dilation (#153) is a span filter, because a region with no event must also change its length. The filters of #154 are span filters for the same reason, and idle time compression (#167) has the same need. A span filter also gets a `SpanContext`: the events before and after its region, and the size of the terminal. Thus a filter that keeps a model of the screen, for example backspace removal (#152), replays the screen up to its region. Typing speed (#155) is a span filter too, because the event form cannot refuse a bad option. Some filters (#166 color scheme, #168 cursor) operate at the ANSI escape sequence level within `event.data` rather than at the timestamp level. Such a filter uses the in-place form (`InPlaceFilterFn`, `register_in_place`): it also gets a `RegionView` (the length of its region, and whether anything comes after it), it answers a `Result`, and nothing on the timeline moves. Thus a filter that sets a state of the terminal can set it back at the end of a range. The reader of escape sequences in `src/nle/src/ansi.rs` keeps its state from one event to the next. #156 and #166 can use it.

3. **Existing code to reuse**:
   - `src/main.rs` lines 85-113: `Recording`, `Event`, `EventType` structs (the types in `src/nle/src/types.rs` must mirror these exactly)
   - `src/export/web.rs`: `TerminalPlayer` JavaScript class. #151 did not use it: the preview uses the Rust emulator of `src/termstate`, so the preview and the video export draw from one emulator.
   - `src/export/video.rs`: Frame-to-FFmpeg pipeline (#160 already exists)
   - `src/termstate/src/lib.rs` (the `termstate` crate): VT100 emulator (reference for #152's state analysis)

4. **Plugins**: Third-party filters, analyzers, and renderers are WASM components that run on wasmtime, in a sandbox with no file, no network, and no real clock. One WIT package, `nle:plugin`, is the contract. A plugin filter has the event form, the in-place form, or the span form, like a built-in filter, and the built-in filters stay native closures. Slice 0 of the spec changes the return type of the event form of decision 2 to `Result<Vec<Event>, String>`, the error type of the in-place form and the span form, because a plugin filter can fail. See [the plugin spec](../../specs/2026-11-26-nle-plugin-architecture-design.md) (issue #169).
