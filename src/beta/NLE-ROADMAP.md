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
| #153 | Time dilation | ~30 lines | Multiply timestamps in a range, shift everything after. |
| #154 | Pause removal/insertion | Half day | Find gaps > threshold, clamp them, shift subsequent events. Nearly identical pattern to #153. |
| #155 | Typing speed normalization | Half day | Re-space input event timestamps to a target characters-per-second. |
| #167 | Idle time compression | Half day | Similar to #154 with additional grid-change heuristic. |
| #156 | Text replacement/redaction | 1 day | Regex replace on `event.data`. Needs care with ANSI escape sequence boundaries. |
| #168 | Cursor customization | 1 day | Intercept/rewrite DECTCEM and cursor shape escape sequences. |
| #160 | Rendering pipeline progress | Half day | Add structured progress reporting to existing `beta export video`. |

### Phase 2: Viewer and Export (1 week)

At the end of this phase: load a recording, apply filters, preview the result, export it. Useful even without the timeline UI.

| Issue | Description | Notes |
|-------|-------------|-------|
| #151 | xterm.js preview pane | Reference code already exists in `web.rs` (the `TerminalPlayer` JavaScript class). Consider snapshotting terminal state at intervals for efficient seeking. |
| #158 | Export edited recordings | Flatten timeline to Beta JSON, pipe to existing Rust exporter. |
| #164 | Chapter index | Manual markers are done. The project holds them, the properties panel adds, edits, removes, and searches them, and the render moves them through the filters. `beta export web` shows them on the progress bar, and `beta export video` writes them into an MP4. Markers on the timeline and a jump to a chapter wait for #150 and #151. Auto-detect commands is a future enhancement. |

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
| #152 | Smart backspace removal | Must correlate input backspaces with output effects while not breaking TUI apps. Needs a mini terminal emulator to distinguish user typos from application behavior. |
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

2. **Filter interface**: `Fn(&[Event], &FilterOptions) -> Vec<Event>` is the core API (`FilterFn` in `src/nle/src/pipeline.rs`). A filter gets the events of its region, with times relative to the start of the region. The pipeline moves the events after the region by the change in the time of the last event of the region. Some filters (#166 color scheme, #168 cursor) operate at the ANSI escape sequence level within `event.data` rather than at the timestamp level — same signature, different transformation target.

3. **Existing code to reuse**:
   - `src/main.rs` lines 85-113: `Recording`, `Event`, `EventType` structs (the types in `src/nle/src/types.rs` must mirror these exactly)
   - `src/export/web.rs`: `TerminalPlayer` JavaScript class (prototype for #151)
   - `src/export/video.rs`: Frame-to-FFmpeg pipeline (#160 already exists)
   - `src/export/terminal_renderer.rs`: VT100 emulator (reference for #152's state analysis)
