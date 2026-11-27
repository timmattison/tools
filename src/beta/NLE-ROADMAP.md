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
| #159 | Research timeline UI components | Research task. Can run in parallel with #170. Informs #150. |

### Phase 1: Quick Wins (3-5 days, all parallel)

Every filter below is a pure function `Event[] → Event[]`. They can all be developed and tested with zero UI — load a JSON recording, pipe through the filter, write a new JSON, play it with `beta play`.

All of these can be built **simultaneously** after Phase 0 types exist.

| Issue | Description | Effort | Notes |
|-------|-------------|--------|-------|
| #153 | Time dilation | Done | A span filter (`src/nle/src/filters/time_dilation.rs`): it divides the times and the length of a range by a speed, and the pipeline moves everything after by the change in the length. The speed panel of the properties panel applies it until #150 can select a region. |
| #154 | Pause removal/insertion | Half day | Find gaps > threshold, clamp them, shift subsequent events. Nearly identical pattern to #153. |
| #155 | Typing speed normalization | Half day | Re-space input event timestamps to a target characters-per-second. |
| #167 | Idle time compression | Half day | Similar to #154 with additional grid-change heuristic. |
| #156 | Text replacement/redaction | 1 day | Regex replace on `event.data`. Needs care with ANSI escape sequence boundaries. |
| #168 | Cursor customization | 1 day | Done. The `cursor` filter (`src/nle/src/filters/cursor.rs`) rewrites DECTCEM, DECSCUSR, mode 12, and OSC 12 in the output events: honor, always visible, always hidden, or smart (hidden during a burst of output), and a shape, a blink, and a color. The render-only items wait for #768 and #151 in a follow-up issue: custom shapes, size, opacity, glow, an outline, a smooth blink, animation, the draw of the shape, the blink, and the color in the video export, CSS in the web export, and the editor preview. |
| #160 | Rendering pipeline progress | Half day | Add structured progress reporting to existing `beta export video`. |

### Phase 2: Viewer and Export (1 week)

At the end of this phase: load a recording, apply filters, preview the result, export it. Useful even without the timeline UI.

| Issue | Description | Notes |
|-------|-------------|-------|
| #151 | xterm.js preview pane | Reference code already exists in `web.rs` (the `TerminalPlayer` JavaScript class). Consider snapshotting terminal state at intervals for efficient seeking. |
| #158 | Export edited recordings | Flatten timeline to Beta JSON, pipe to existing Rust exporter. |
| #164 | Chapter index | Manual markers first. Auto-detect commands is a future enhancement. |

### Phase 3: Timeline UI (2-3 weeks)

The big UI push. This is the most complex part of the project.

| Issue | Description | Notes |
|-------|-------------|-------|
| #150 | Timeline UI component | Informed by #159 research. Horizontal scrolling, zoom, clip dragging, playhead, selection ranges. Likely canvas-based for performance. |
| #157 | Multi-clip splicing | Start with hard cuts (clear screen between clips). Transitions later. Terminal size differences between clips need handling. |

### Phase 4: Advanced Filters (1-2 weeks, parallel)

These filters need deeper terminal state analysis or more complex heuristics.

| Issue | Description | Why harder |
|-------|-------------|-----------|
| #152 | Smart backspace removal | Done. The `backspace-removal` span filter (`src/nle/src/filters/backspace.rs`) removes a typed key and its erase (Backspace, Ctrl+W, Ctrl+U), with their output and their time, only when the screen of the `termstate` emulator after the erase equals the screen before the key. Thus the backspaces that a TUI writes in its output stay. |
| #162 | Output fast-forward | Manual region selection works immediately. Auto-detection (future) needs prompt heuristic. |
| #166 | Color scheme conversion | Operates at ANSI escape sequence level within `event.data`. Must remap palette codes without breaking other sequences. |
| #161 | Keystroke overlay / HUD | Parse input events into displayable key labels. Needs escape sequence → key name mapping. |

### Phase 5: Polish and Extensions (ongoing)

| Issue | Description | Notes |
|-------|-------------|-------|
| #163 | Captions and annotations | Text overlays at timestamps. UI-heavy. |
| #165 | Terminal crop and zoom | Dynamic viewport requires deep escape sequence understanding. |
| #169 | Plugin architecture | Design can start earlier, but API should stabilize first. TypeScript primary, WASM for advanced. |

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

2. **Filter interface**: `Fn(&[Event], &FilterOptions) -> Vec<Event>` is the core API (`FilterFn` in `src/nle/src/pipeline.rs`). A filter gets the events of its region, with times relative to the start of the region. The pipeline moves the events after the region by the change in the time of the last event of the region. A filter that changes the length of its region is a span filter (`SpanFilterFn`, `FilterRegistry::register_span`): it also gets the length of its region and answers a new length, and the pipeline moves the events after the region by the change in the length. Time dilation (#153) is a span filter, because a region with no event must also change its length. A span filter also gets a `SpanContext`: the events before its region, and the size of the terminal. Thus a filter that keeps a model of the screen, for example backspace removal (#152), replays the screen up to its region. Pause insertion (#154) and idle time compression (#167) have the same need. Some filters (#166 color scheme, #168 cursor) operate at the ANSI escape sequence level within `event.data` rather than at the timestamp level. Such a filter uses the in-place form (`InPlaceFilterFn`, `register_in_place`): it also gets a `RegionView` (the length of its region, and whether anything comes after it), it answers a `Result`, and nothing on the timeline moves. Thus a filter that sets a state of the terminal can set it back at the end of a range. The reader of escape sequences in `src/nle/src/ansi.rs` keeps its state from one event to the next. #156 and #166 can use it.

3. **Existing code to reuse**:
   - `src/main.rs` lines 85-113: `Recording`, `Event`, `EventType` structs (the types in `src/nle/src/types.rs` must mirror these exactly)
   - `src/export/web.rs`: `TerminalPlayer` JavaScript class (prototype for #151)
   - `src/export/video.rs`: Frame-to-FFmpeg pipeline (#160 already exists)
   - `src/termstate/src/lib.rs` (the `termstate` crate): VT100 emulator (reference for #152's state analysis)
