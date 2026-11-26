# NLE timeline: research and recommendation

- Issue: #159 (parent #148). Issue #150 builds the timeline from this document.
- Date: 2026-11-26.
- Decisions on issue #159:
  - D-159-1: this file, a link from `src/beta/NLE-ROADMAP.md`, and a comment
    on issue #150.
  - D-159-2: a temporary prototype in `dioxus-desktop` that measures the frame
    times of each way to draw. The prototype code does not merge.

## Recommendation

Build the timeline in RSX, with virtualization in tiles. Draw the event
density of each tile as one SVG `path`. Do not use a JavaScript timeline
library, and do not use a canvas for the first version.

1. **Build a custom timeline.** No Dioxus crate and no plain JavaScript
   library gives the set that issue #150 needs (see
   [What exists to reuse](#what-exists-to-reuse)). The nearest library,
   vis-timeline, keeps its own copy of the clips, and issue #149 makes the Rust
   `Editor` the one source of truth.
2. **Draw the clips as RSX elements.** Each clip is one `div` with an absolute
   position. A drag changes only its `transform: translate3d(...)`.
3. **Divide each clip into tiles of 512 px, and render only the tiles in the
   window and in an overscan of 600 px.** A `Tile` component takes only the
   clip, the tile index, and the pixels per second as props. Thus Dioxus skips
   each tile during a drag and a pan, and renders the tiles again only on a
   zoom. In the real timeline, a filter changes the events of a clip, thus the
   props of a tile must also hold a revision number of those events.
4. **Draw the event density of a tile as one SVG `path`.** Rust counts the
   events of each pixel column (a binary search for the first and the last
   event, then one pass), and the path has one vertical segment for each
   column that holds events. The count and the path text are plain Rust
   functions, thus a unit test can do a check of them with no webview.
5. **Never make one DOM node for each event, and never make one `div` for each
   pixel column.** In the measurements, one node for each event missed 68 of
   120 frames of a zoom at 10,000 events. One `div` for each column missed up
   to 36 of 120 frames at 1,000,000 events.
6. **Make the edge handles, the playhead, the selection region, the ruler, and
   the marks of the terminal size changes RSX elements.** They are few, and
   the DOM gives their hit areas, their cursor shapes, their focus, and their
   text with no more code. Take the tick rule of the ruler from Remotion
   Studio: the label step is the first round interval that leaves room for one
   label, and the minor ticks are at least 16 px apart (see
   [Interaction design from Remotion Studio](#interaction-design-from-remotion-studio)).
   `egui_timeline` uses the same rule, with a minimum gap of 4 px. Remotion
   draws its ruler on a canvas. At 16 px between the minor ticks, a window of
   1,200 px holds at most 75 ticks, thus RSX elements are sufficient.
7. **Keep the hit test and the drag state in Rust, and apply one edit when the
   drag ends.** Use the pointer events of Dioxus. During the drag, a signal
   holds the preview position, and only the transform of the clip changes. On
   `pointerup`, call `Editor::apply` one time. Each call of `Editor::apply`
   adds one step to the undo history, thus one edit for each pointer move
   gives one undo step for each move. Escape cancels the drag and applies no
   edit. The Animations panel of Chrome DevTools uses the same rule: a move
   only draws, and the release changes the model. A drag that must continue
   outside the timeline needs one `document::eval` for `setPointerCapture` or
   for listeners on `window`, because Dioxus 0.7 gives no method for pointer
   capture. Each kind of drag gives one edit:
   - A move of a clip is one `MoveClip`.
   - A drag of the end edge is one `TrimClip`. It changes `out_point`, and
     the start of the clip stays.
   - A drag of the start edge by d adds d to `in_point` and to
     `timeline_offset`, thus the end edge stays. Remotion Studio does the
     same (`getTimelineSequenceLeftEdgeDragValues`). The `Edit` of
     `src/nle/src/editor.rs` has no variant for this drag. `TrimClip` keeps
     `timeline_offset`, and a `TrimClip` with a `MoveClip` gives two undo
     steps. Thus issue #150 adds one edit that changes `in_point` and
     `timeline_offset` together, for example a `TrimClip` that also takes the
     new `timeline_offset`.
8. **Take no Dioxus drag crate.** `dioxus-gestures` and `dioxus-dnd` hold a
   drag to no axis, snap to no clip edge, and resize no edge (see
   [Dioxus crates](#dioxus-crates)). The drag of issue #150 is a set of
   pointer handlers in Rust, on top of the hit test.
9. **Take the defaults of each interaction from Remotion Studio.** Take its
   zoom anchor at the pointer, its trim areas of 8 px, its trim threshold of
   4 px, its snap distance of 10 px, Escape to cancel, and its keys (see
   [Interaction design from Remotion Studio](#interaction-design-from-remotion-studio)).
   Remotion gives the move of a clip no threshold in px: the move starts
   after half a frame of movement. The NLE counts time in seconds, thus give
   the move the trim threshold of 4 px. Remotion snaps a clip only to the start of the timeline. A clip that must
   join the end of another clip needs more targets: the playhead and the edges
   of the other clips.
10. **Keep the canvas as the fallback for a later dense layer.** If a layer
    needs more than SVG can hold, for example thumbnails of the terminal,
    draw that layer on a `<canvas>` in the `canvas-eval` way. Rust bins the
    data, and one long-lived `document::eval` draws the bars that Rust sends.
    The interactive objects stay RSX. If the zoom of the real timeline misses
    frames, move the density layer to this way. The change stays inside the
    tile component.

The SVG path and the two canvas ways all held 60 frames each second at
1,000,000 events (see [Measurements](#measurements)). The SVG path is the
recommendation for these reasons:

- The state and the drawing stay in Rust. A canvas needs a JavaScript draw
  program, a message format between Rust and JavaScript, and its own scale
  for the device pixel ratio.
- The DOM gives the hit areas, the cursor shapes, the focus, and the text of
  the interactive objects. On a canvas, the program must do each of these.
- `canvas-cached` puts a second copy of the events in JavaScript, and its
  binning is JavaScript code. It also took 62 to 68 ms to send 1,000,000 event
  times when the recording loaded.

Issue #150 also needs a playhead that moves during playback. In each RSX way,
the drag of a clip changes one transform in each frame, and it held 60 frames
each second, also with 100,000 DOM nodes. Thus a playhead `div` that moves
with a `transform` is sufficient.

## The constraints

The constraints come from issues #148, #149, #150, and #761:

- The app is Dioxus 0.7 (0.7.10 in `Cargo.lock`) with the desktop renderer.
  The renderer draws HTML in a webview: wry, which is WKWebView on macOS.
- The state of the editor is Rust. The `Editor` of `src/nle/src/editor.rs`
  holds the `EditProject`, and the timeline changes it only through
  `Editor::apply`. A JavaScript library that keeps its own model of the clips
  is a second copy of the state.
- Issue #150 needs: horizontal scroll and zoom, clips as blocks on tracks with
  an event-density display, a drag that moves a clip, a drag of a clip edge
  that sets the in or out point, a playhead to drag or click, a region
  selection by drag, a cut at the playhead, more than one track, zoom by wheel
  and by buttons, a time ruler with adaptive ticks, a mark where the terminal
  size changes, and keyboard shortcuts.
- A recording can hold many events. One hour of output with typing can hold
  more than 100,000 events.

## What exists to reuse

All facts in this section were read from their sources on 2026-11-26. The
sources are at the end of this document.

### Dioxus crates

No Dioxus crate gives a media timeline (clips on tracks, edge trim, a ruler).

| Crate | Version | Targets | What it gives | Fit for the timeline |
| --- | --- | --- | --- | --- |
| `dioxus-primitives` (DioxusLabs/dioxus-components) | 0.0.0 on crates.io (2025-04-13); the repository is active | Dioxus 0.7 | `slider`, `scroll_area`, `virtual_list`, `drag_and_drop_list` | No timeline and no resize. Its `pointer.rs` tracks the pointer with window listeners in a `document::eval`. |
| `dioxus-dnd` | 3.1.0 (4.0.0-alpha.1 targets Dioxus 0.8) | Dioxus 0.7 | Drop zones, a `canvas` module with free drops, `SnapGrid`, bounds | A drop model. It gives no continuous move of a clip and no edge trim. |
| `dioxus-gestures` | 0.2.3 (2025-12-14) | Dioxus ^0.7.2 | `Drag`, `Pinch`, and `Hover` hooks on pointer events | The README says it has tests on the web only. |
| `dioxus-dnd-kit` | 0.4.1 | Dioxus ^0.7 | Sortable lists | Not a timeline. |
| `dioprism-timeline` | 0.2.0-alpha.1 | Dioxus ^0.8.0-alpha | Keyframe animation timeline | An alpha for the wrong Dioxus version. |
| `dioxus-audio` | 0.2.1 | Dioxus ^0.7 | A player scrubber and waveform views | A player for the browser, not an editor. |

Dioxus 0.7 gives `onpointerdown`, `onpointermove`, `onpointerup`, and the
capture events (`ongotpointercapture`, `onlostpointercapture`). It gives no
method to call `setPointerCapture`: the source of `dioxus-html` 0.7.10 holds
no such call. Thus a drag that must continue outside the timeline element
needs one `document::eval`, either to call `setPointerCapture` or to listen on
`window`. `dioxus-primitives` uses the second way.

No Dioxus crate gives the primitives of a clip drag: a move held to one axis,
a snap to the edges of other clips, and a resize from an edge. The two drag
crates were read from their crates.io packages:

- `dioxus-gestures` 0.2.3: its `Drag` gives `on_start`, `on_update`,
  `on_end`, and `on_cancel`, and it starts after 5 px of movement
  (`src/state/gestures/drag.rs`). It has no axis lock, no snap, and no edge
  resize. It calls `setPointerCapture` through `document::eval`
  (`src/state/state.rs`).
- `dioxus-dnd` 3.1.0: its `Axis` sets only the layout of a `SortableList`
  (`src/sortable.rs`). Its `SnapGrid` snaps a free drop to a square grid
  (`src/canvas.rs`), and its collision strategies pick a drop target
  (`src/core/collision.rs`). Neither snaps to the edge of another item. It
  has no edge resize. It calls `set_pointer_capture` only with the feature `web`
  (`src/core/platform.rs`), thus on desktop it draws a layer over the whole
  window to catch the pointer.

### Rust editors and DAWs

- `egui_timeline` 0.5.0 (nannou-org, MIT OR Apache-2.0) is a musical
  timeline for egui. Its ruler picks the tick step from a minimum gap of 4 px
  between steps (`MIN_STEP_GAP`). Its `TimelineApi` trait has
  `shift_timeline_start` and `zoom`. It leaves the hit test of the clips to the
  host. egui cannot run in the webview, thus the value is the tick rule.
- OpenCut has a desktop app on GPUI. Its `panels/timeline.rs` is a placeholder
  that shows the text "Timeline".
- Meadowlark (AGPL-3.0) moved to Codeberg, and the GitHub copy is old.
- `oximedia-timeline`, `oximedia-edit`, and `avio` describe timeline models
  for video. They have few downloads, and they hold no interface.

No Rust project gives a hit-test or edge-drag algorithm in a form to copy.
The timeline model of issue #150 is small: a map from time to pixels, a hit
test of a point against the clip rectangles, and a test of the clip edges
within a few pixels. Issue #150 writes it in Rust.

### Plain JavaScript libraries

These libraries need no React, Vue, or Angular. Each one can run in the
webview, and Rust can drive it through `document::eval`.

| Library | Draws with | License | Last release | Clips on tracks | Zoom ruler | Playhead scrub | Region select | Edge trim | Drag callback |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| vis-timeline 8.5.6 | DOM | Apache-2.0 OR MIT | 2026-10-26 | Items in groups | Yes | `customTime` | Not found | Yes | `onMove`, `onMoving`, `timechanged` |
| animation-timeline-js 2.3.5 | canvas | MIT | 2024-07-21 | Keyframe rows, no clip blocks | Yes | `timeChanged` | Keyframes | Not found | `drag`, `dragFinished`, `keyframeChanged` |
| wavesurfer.js 8.1.1 (Timeline and Regions plugins) | canvas waveform, DOM regions | BSD-3-Clause | 2026-11-24 | One waveform, no tracks | Ticks | Yes | Yes | Yes | `region-update`, `region-updated` |
| peaks.js 4.0.0 | canvas (Konva) | LGPL-3.0 | 2025-08-30 | Segments on one waveform | Yes | `player.seeked` | Segments | Yes | `segments.dragged`, `segments.dragend` |
| Konva 10.11.0 | canvas scene graph | MIT | 2026-11-23 | Nothing for timelines | No | No | No | No | `dragmove`, `dragend` |

No library has the whole set: clips on more than one track, edge trim, region
selection, a playhead, and an adaptive ruler. vis-timeline is the nearest, but
it has no event-density display. Also, it keeps its items in its own
`DataSet`, which is a second copy of the state of issue #149. The others are
for one waveform (wavesurfer.js, peaks.js) or for keyframes
(animation-timeline-js). peaks.js is LGPL-3.0.

### What other editors use

Only the editors with a source that states it are in this list:

- Canvas: the timeline of openvideodev/react-video-editor (formerly
  designcombo) draws on a canvas through Fabric. Its ruler and playhead are
  separate React components.
- DOM and canvas: Remotion Studio draws its clips, its handles, and its
  playhead as DOM, and its ruler on a `<canvas>`. The playhead is a `div` that
  moves with `translateX`.
- DOM: Omniclip (Lit web components with `pointermove` listeners on
  `window`), the dope sheet of Theatre.js (React), and vis-timeline.
- SVG: the Animations panel of Chrome DevTools draws its grid and each
  animation row as `<svg>`, and its scrubber is a `div` that moves with
  `translateX`. A drag only draws again, and the release calls the model one
  time (`setOffset` or `setTiming`).
- Waveforms are on a canvas in each editor that states it: wavesurfer.js,
  peaks.js, and the waveform-playlist editor.

No source states the timeline technology of Clipchamp, Kapwing, Canva, CapCut,
Descript, BandLab, or the Lottie Creator of LottieFiles. The pattern of the
editors with a source is a mix: the dense layer (waveform, density, ruler,
thumbnails) is on a canvas, and the interactive objects (clips, handles,
playhead) are DOM.

### Interaction design from Remotion Studio

Remotion Studio is React, thus issue #150 can take only its rules, not its
code. The facts are from commit `19e4da7` of `remotion-dev/remotion`.
Remotion Studio counts time in frames, and the NLE counts time in seconds.

In the Source column, a file name with no directory is in
`packages/studio/src/components/Timeline/`. A path that starts with
`packages/` starts at the root of the repository. Each other path starts at
`packages/studio/src/`.

| Interaction | What Remotion Studio does | Source |
| --- | --- | --- |
| Zoom range | The zoom is in px per frame. The maximum is 30 px per frame. The minimum shows the whole composition in the window. | `helpers/get-timeline-max-zoom.ts` |
| Zoom input | A button multiplies or divides the zoom by 1.2. Cmd or Ctrl with the wheel multiplies it by `exp(-deltaY * 0.06)`. The keys are `+`, `-`, and `0` to reset. | `TimelineZoomControls.tsx`, `TimelinePinchZoom.tsx`, `components/keyboard-shortcuts.ts` |
| Zoom anchor | A wheel zoom keeps the time under the pointer at the same place. If the pointer is within 8 px of the playhead, the playhead is the anchor. | `TimelinePinchZoom.tsx` |
| Scroll in playback | When the playhead leaves the window, the view moves one window width. | `timeline-scroll-logic.ts`, `TimelinePlayCursorSyncer.tsx` |
| Playhead | A pointer down on the timeline pauses, moves the playhead to the frame under the pointer, and captures the pointer. Near an edge of the window, the view scrolls each 100 ms. The playhead is a line of 1 px. Its `translateX` is written through a ref, outside the React render. | `TimelineDragHandler.tsx`, `timeline-scroll-logic.ts`, `TimelineSlider.tsx` |
| Move | A move of a clip has no threshold in px. It starts when the pointer moves half a frame, because the change rounds to whole frames. A vertical move of more than 6 px starts a move to another track. | `TimelineSequenceRightEdgeDragHandle.tsx` (`useTimelineSequenceFromDrag`) |
| Trim | Each clip edge has a hit area of 8 px. A trim starts after 4 px of movement. A drag of the left edge changes the start and the trim of the clip together, thus the right edge stays. A clip that is too narrow loses a handle, and the handles do not shrink. A tooltip shows the signed change and the frame of the edge. | `get-timeline-trim-handle-layout.ts`, `TimelineSequenceRightEdgeDragHandle.tsx`, `TimelineTrimTooltip.tsx` |
| Snap | The snap is on by default, and `Shift+M` turns it off and on. A moved clip snaps to the start of the timeline within 10 px. No snap to the playhead or to other clips was found. | `state/editor-snapping.ts`, `TimelineSequenceRightEdgeDragHandle.tsx` |
| Drag session | Each drag captures the pointer and listens on `window` for `pointermove`, `pointerup`, `pointercancel`, and `blur`. Escape cancels the drag. | `helpers/escape-cancelable-pointer-session.ts`, `packages/sdk/src/pointer-session.ts` |
| Selection | A drag from an empty area starts a marquee after 3 px. A clip that touches the marquee is selected. Shift and Cmd or Ctrl change the selection on a click. | `TimelineSelection.tsx` |
| In and out marks | `i` sets the in mark, `o` sets the out mark, and `x` clears both. A drag moves a mark. The area outside the marks is 50 % darker. | `components/keyboard-shortcuts.ts`, `components/TimelineInOutToggle.tsx`, `TimelineInOutPointer.tsx` |
| Cut | `Cmd+Shift+D` or `Ctrl+Shift+D` splits the selected clips at the playhead. It refuses a split at or outside an edge of a clip. | `components/keyboard-shortcuts.ts`, `split-selected-timeline-item.ts` |
| Playback keys | Space plays and pauses. `j`, `k`, and `l` play in reverse, pause, and play forward. `a` and `e` go to the start and the end. Cmd with an arrow moves 1 frame. For a media asset, an arrow moves 1 frame, Shift with an arrow moves 1 s, and Alt with an arrow goes to the start or the end. | `components/keyboard-shortcuts.ts`, `components/PlayPause.tsx` |
| Ruler | A `<canvas>` of 39 px draws the ruler. The label step is the first of 1, 2, 5, 10, 15, 30, 60, 120, 180, 300, 600, 900, 1200, 1800, and 3600 s that leaves room for a label. The minor ticks are at least 16 px apart, and the medium ticks at least 48 px. | `TimelineTimeIndicators.tsx` |
| Virtualization | Only the rows are virtualized, with `@tanstack/react-virtual` and an overscan of 20 rows. | `TimelineVirtualization.tsx` |

## Measurements

The prototype of decision D-159-2 measured five ways to draw the timeline on
2026-11-26. Its code is not in the repository.

### The prototype

- **The scene.** 4 tracks with 2 clips on each track. Each clip is 600 s
  long, and a gap of 30 s comes after each clip, also after the last clip.
  Thus the clips of a track start at 0 s and 630 s, and the timeline is
  1,260 s long. The window is 1,200 × 240 CSS px, at a device pixel ratio
  of 2.
- **The events.** 10,000, 100,000, or 1,000,000 events in total, divided
  equally between the 8 clips. Each clip has 200 bursts, as in a terminal
  recording, and each event is within 2 s of the center of its burst. The
  generator is deterministic.
- **The start zoom.** `fit` shows all 1,260 s in 1,200 px (0.95 px/s), thus
  all events are visible. `zoom` starts at 50 px/s, which shows 24 s.
- **The state.** In each way, Rust keeps the clip offsets, the scroll, the
  zoom, and the drag, and Rust handles each mouse and wheel event.

The five ways:

| Way | Kind | What draws the event density |
| --- | --- | --- |
| `dom` | RSX with tiles | One `div` for each event. |
| `dom-binned` | RSX with tiles | One `div` for each pixel column that holds events. |
| `svg-binned` | RSX with tiles | One SVG `path` for each tile, with one segment for each pixel column. |
| `canvas-eval` | canvas through `document::eval` | Rust bins the visible events in each frame and sends the bars through one long-lived `document::eval`. JavaScript draws them. |
| `canvas-cached` | canvas through `document::eval` | Rust sends all event times one time. In each frame, Rust sends only the view state, and JavaScript bins and draws. |

Each RSX way uses tiles of 512 px and an overscan of 600 px.

### The driver

A JavaScript driver in one `document::eval` sends synthetic `mousedown`,
`mousemove`, `mouseup`, and `wheel` events to the timeline. It sends one event
at the start of each frame, and it records the time between frames from
`requestAnimationFrame`. At the end of each gesture, it waits until the last
event has its acknowledgement. In an RSX way, the acknowledgement is the change
of a counter that Rust renders. In a canvas way, it is the end of the draw.

- **Drag:** 241 frames. The driver moves clip 0 back and forth, 2 px in each
  frame.
- **Pan:** 241 frames. One wheel step of 12 px in each frame, to the right and
  then to the left.
- **Zoom:** 121 frames. One ctrl+wheel step of 3 % in each frame, in for 30
  frames and then out for 30 frames, two times. Each step changes the pixels
  per second, thus each visible tile renders again.

A frame of more than 20 ms missed the budget of 16.7 ms at 60 Hz. In WebKit,
`performance.now()` has a resolution of 1 ms, thus a frame on time measures
16 to 19 ms.

### The environment

- Apple M4 Pro with 14 cores and 64 GiB, macOS 27.2, Rust 1.98.0, a release
  build of `dioxus-desktop` 0.7.10 on wry 0.53.5 (WKWebView).
- Other work ran on the machine at the same time. The 1-minute load average
  was 6.5 to 42.5 at the start of the runs.
- The screen was locked, and the displays were asleep. WebKit hides the page
  of a window that it thinks is occluded, and then `requestAnimationFrame`
  does not fire. The prototype calls the private setter
  `_setWindowOcclusionDetectionEnabled:` of WKWebView with `NO`, and then
  hides and shows the webview. After that, the page was visible, and the idle
  frames had a median of 17 ms.
- Round 1 ran 29 configurations. It did not run `dom` at 1,000,000 events at
  `fit`, because that is 1,000,000 DOM nodes. Round 2 ran the four other ways
  again at 100,000 and 1,000,000 events. A cell with two values gives round 1
  and then round 2.

### The results

- **Drag late** and **Pan late:** the frames of more than 20 ms, of 240.
- **Zoom late:** the frames of more than 20 ms, of 120.
- **Load:** the time from the load of the events to the acknowledgement. In
  an RSX way, Rust renders the first tiles. In `canvas-cached`, Rust also sends
  all event times to JavaScript.
- **Nodes:** the DOM elements of the page at the end of the run.

| Way | Events | Start zoom | Drag late | Pan late | Pan longest (ms) | Zoom late | Zoom p95 (ms) | Zoom longest (ms) | Load (ms) | Nodes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `dom` | 10000 | fit | 0 | 0 | 19 | 68 | 37 | 40 | 14 | 10036 |
| `dom-binned` | 10000 | fit | 0 | 0 | 19 | 6 | 21 | 31 | 7 | 3324 |
| `svg-binned` | 10000 | fit | 0 | 0 | 19 | 0 | 19 | 19 | 3 | 52 |
| `canvas-eval` | 10000 | fit | 0 | 0 | 19 | 0 | 18 | 19 | 3 | 12 |
| `canvas-cached` | 10000 | fit | 0 | 0 | 19 | 0 | 18 | 19 | 3 | 12 |
| `dom` | 10000 | zoom | 0 | 0 | 19 | 0 | 18 | 19 | 2 | 401 |
| `dom-binned` | 10000 | zoom | 1 | 0 | 19 | 0 | 18 | 19 | 2 | 387 |
| `svg-binned` | 10000 | zoom | 0 | 0 | 19 | 0 | 18 | 19 | 3 | 52 |
| `canvas-eval` | 10000 | zoom | 0 | 0 | 19 | 0 | 18 | 19 | 2 | 12 |
| `canvas-cached` | 10000 | zoom | 0 | 0 | 19 | 0 | 18 | 19 | 3 | 12 |
| `dom` | 100000 | fit | 0 | 102 | 49 | 120 | 984 | 1114 | 108 | 100036 |
| `dom-binned` | 100000 | fit | 0 / 0 | 0 / 0 | 19 / 19 | 18 / 16 | 25 / 24 | 32 / 26 | 7 / 7 | 3779 |
| `svg-binned` | 100000 | fit | 0 / 0 | 0 / 0 | 20 / 19 | 1 / 0 | 18 / 18 | 22 / 19 | 3 / 3 | 52 |
| `canvas-eval` | 100000 | fit | 0 / 0 | 0 / 0 | 19 / 19 | 0 / 0 | 19 / 18 | 19 / 19 | 3 / 3 | 12 |
| `canvas-cached` | 100000 | fit | 0 / 0 | 0 / 0 | 19 / 19 | 0 / 0 | 18 / 18 | 19 / 19 | 9 / 9 | 12 |
| `dom` | 100000 | zoom | 0 | 1 | 25 | 3 | 19 | 24 | 6 | 3662 |
| `dom-binned` | 100000 | zoom | 0 / 0 | 0 / 0 | 19 / 19 | 0 / 0 | 18 / 18 | 19 / 19 | 5 / 47 | 2569 |
| `svg-binned` | 100000 | zoom | 0 / 0 | 0 / 0 | 19 / 19 | 0 / 0 | 18 / 19 | 19 / 19 | 3 / 2 | 52 |
| `canvas-eval` | 100000 | zoom | 0 / 0 | 0 / 0 | 19 / 19 | 0 / 0 | 18 / 18 | 19 / 19 | 1 / 2 | 12 |
| `canvas-cached` | 100000 | zoom | 0 / 0 | 0 / 0 | 19 / 19 | 0 / 0 | 19 / 18 | 19 / 19 | 9 / 7 | 12 |
| `dom-binned` | 1000000 | fit | 0 / 0 | 0 / 0 | 19 / 19 | 13 / 15 | 23 / 23 | 30 / 26 | 10 / 10 | 3846 |
| `svg-binned` | 1000000 | fit | 0 / 0 | 0 / 0 | 19 / 19 | 0 / 0 | 19 / 18 | 19 / 19 | 5 / 3 | 52 |
| `canvas-eval` | 1000000 | fit | 0 / 0 | 0 / 0 | 19 / 19 | 0 / 0 | 18 / 18 | 19 / 19 | 3 / 3 | 12 |
| `canvas-cached` | 1000000 | fit | 0 / 0 | 0 / 0 | 19 / 19 | 0 / 0 | 18 / 18 | 19 / 19 | 65 / 68 | 12 |
| `dom` | 1000000 | zoom | 0 | 12 | 146 | 117 | 178 | 194 | 38 | 33745 |
| `dom-binned` | 1000000 | zoom | 0 / 0 | 0 / 0 | 19 / 19 | 36 / 33 | 26 / 27 | 32 / 29 | 14 / 11 | 5881 |
| `svg-binned` | 1000000 | zoom | 0 / 0 | 0 / 0 | 19 / 20 | 0 / 0 | 18 / 19 | 19 / 19 | 3 / 3 | 52 |
| `canvas-eval` | 1000000 | zoom | 0 / 0 | 0 / 0 | 19 / 19 | 0 / 0 | 18 / 18 | 19 / 19 | 2 / 3 | 12 |
| `canvas-cached` | 1000000 | zoom | 0 / 0 | 0 / 0 | 19 / 19 | 0 / 0 | 18 / 18 | 19 / 19 | 62 / 62 | 12 |

What the results show:

- **A drag held 60 frames each second in each way.** Each way missed at most
  1 of 240 frames, also with 100,000 DOM nodes. A drag changes only the
  transform of one clip, and Dioxus skips each tile, because the props of the
  tile do not change.
- **A pan held 60 frames each second in each way but `dom`.** `dom` missed 102
  of 240 frames at 100,000 events at `fit`.
- **The zoom separates the ways.** `dom` missed 68 of 120 frames at 10,000
  events, and 120 of 120 frames at 100,000 events, with a p95 of 984 ms.
  `dom-binned` missed up to 36 of 120 frames, with a p95 of up to 27 ms. It
  missed frames in each configuration with more than 3,000 DOM nodes.
  `svg-binned`, `canvas-eval`, and `canvas-cached` missed 0 or 1 of 120 frames
  at each event count, with a p95 of 18 or 19 ms.
- **The bridge of `document::eval` costs little.** In `canvas-eval`, Rust
  sends the bars of all visible clips in each frame, and the frames stayed on
  time at 1,000,000 events. For one drag step at a time, the median from the
  mouse event to the acknowledgement was 0 to 3 ms in each way.
- **`canvas-cached` pays at load.** It took 62 to 68 ms to send 1,000,000
  event times to JavaScript.

### The limits of the measurements

- The frames are capped at 60 Hz. Thus the three ways that held 60 frames each
  second cannot be ranked by their headroom. A frame that took less than
  16.7 ms is not visible in these numbers.
- The displays were asleep. WebKit drew each frame, but the numbers do not
  include the composite to a display that is on.
- Other work ran on the machine, thus some late frames can come from that
  load. The two rounds agree within a few frames.
- The driver sends synthetic DOM events. A real mouse adds the input latency
  of the system, which is the same for each way.
- The prototype draws no ruler, no handles, no playhead, and no text. The
  recommendation makes these RSX elements in each way, thus they add the same
  cost to each way.

## Sources

- Dioxus: `dioxus-html` 0.7.10 and `dioxus-desktop` 0.7.10 in the cargo
  registry (`events/pointer.rs`, `events/mounted.rs`, `query.rs`, `edits.rs`).
- <https://github.com/DioxusLabs/dioxus-components> and
  <https://raw.githubusercontent.com/DioxusLabs/dioxus-components/HEAD/primitives/src/pointer.rs>
- <https://crates.io/crates/dioxus-dnd>, <https://crates.io/crates/dioxus-gestures>,
  <https://crates.io/crates/dioxus-dnd-kit>, <https://crates.io/crates/dioprism-timeline>,
  <https://github.com/sagikazarmark/dioxus-audio>
- The crates.io packages of `dioxus-gestures` 0.2.3 and `dioxus-dnd` 3.1.0:
  <https://crates.io/api/v1/crates/dioxus-gestures/0.2.3/download> and
  <https://crates.io/api/v1/crates/dioxus-dnd/3.1.0/download>
- <https://github.com/nannou-org/egui_timeline>
- <https://github.com/OpenCut-app/OpenCut/blob/HEAD/apps/desktop/src/panels/timeline.rs>
- <https://github.com/MeadowlarkDAW/Meadowlark>
- <https://github.com/visjs/vis-timeline>, <https://visjs.github.io/vis-timeline/docs/timeline/>
- <https://github.com/ievgennaida/animation-timeline-control>
- <https://github.com/katspaugh/wavesurfer.js>
- <https://github.com/bbc/peaks.js>
- <https://github.com/konvajs/konva>
- <https://github.com/openvideodev/react-video-editor>
- <https://github.com/omni-media/omniclip>
- <https://github.com/remotion-dev/remotion/blob/HEAD/packages/studio/src/components/Timeline/TimelineSlider.tsx>
- Remotion Studio at commit `19e4da7c86fa1140eabb452fefe37cd0d803ad86`:
  <https://github.com/remotion-dev/remotion/tree/19e4da7c86fa1140eabb452fefe37cd0d803ad86/packages/studio/src>
  (the files that the table of
  [Interaction design from Remotion Studio](#interaction-design-from-remotion-studio)
  names, and `packages/sdk/src/pointer-session.ts`)
- The Animations panel of Chrome DevTools at commit
  `2c757d71ea7698b3a130374615eaac91ecfad887`:
  <https://github.com/ChromeDevTools/devtools-frontend/tree/2c757d71ea7698b3a130374615eaac91ecfad887/front_end/panels/animation>
  (`AnimationTimeline.ts`, `AnimationUI.ts`)
- The timeline of Lottie Creator:
  <https://docs.lottiefiles.com/en/creator/07_animation/timeline.md> (it does
  not state what draws the timeline)
- <https://github.com/theatre-js/theatre>
- <https://github.com/naomiaro/waveform-playlist>
