# ic-inline

A Claude Code plugin that shows pictures inline in the transcript. [`ic`](../../src/ic/README.md) decodes them.

- `/ic <image file>` shows a picture to you, under the command.
- `show_image` is a tool for Claude. Claude uses it to show you a picture that it made or found: a chart, a diagram, a screenshot. The picture shows in the result row of the call.

## Install

Install an `ic` that knows `--emit-rgba` (`ic --help` lists it). The plugin needs that flag:

```bash
cargo install --git https://github.com/timmattison/tools ic
```

Then install the plugin. Type this at the prompt of a terminal session of Claude Code:

```
/plugin install ic-inline --marketplace timmattison/tools
```

Type `y` to add the marketplace, then select a scope.

To run it from a checkout, give the folder to Claude Code:

```bash
claude --plugin-dir ~/code/tools/plugins/ic-inline
```

## How it draws

Claude Code owns every cell of its screen, and it draws each cell again when something changes. An image that `ic photo.png` writes lands at a position that the next frame paints over. Thus the plugin does not let `ic` write to the terminal. It runs `ic --emit-rgba 720x720 <file>`, which prints the pixels as JSON, and it draws the pixels with the elements of Claude Code:

| Terminal | What you see |
| --- | --- |
| kitty, Ghostty | The real pixels, in an `Image` element. |
| Every other terminal (iTerm2, for example), every pane of zellij or tmux, and mosh | Colored half blocks (`▀`) in a `Raster` element: two pixels in each cell, 24-bit color. |

The plugin finds out which terminal it has from Claude Code itself. While the answer is unknown, the band above the prompt holds one transparent cell of an `Image`. The plugin asks Claude Code a few times (for about 1.5 seconds) whether that `Image` takes a new picture. Claude Code refuses when the `Image` draws its alt text in place of pixels. Pictures draw as half blocks until the answer is yes, so a wrong answer costs resolution and never a picture.

A transparent pixel takes the background of your terminal. Claude Code paints at most 1024 different color pairs at one time, so a photograph in half blocks shows some banding.

The desktop app, VS Code, and the mobile app have no `Raster` and no `Image`. There, the row shows its text.

## Options

Set these in `/config`, or in `settings.json` under `pluginConfigs["ic-inline"]`.

| Option | Default | What it does |
| --- | --- | --- |
| `renderer` | `auto` | `auto` asks the terminal. `blocks` always draws half blocks. `image` always draws pixels: a terminal that cannot shows the caption in their place. |
| `maxRows` | `24` | The tallest that a picture can be, in terminal rows. |
| `icPath` | `ic` | The `ic` to run: a name on `PATH`, or an absolute path. |

## What Claude sees

`show_image` gives Claude the size of the picture and not its content. To look at a picture, Claude uses `Read`. A failure, such as a missing file or a file that holds no picture, comes back to Claude as an error with the reason that `ic` gave.

If Claude Code asks your permission for `show_image`, add `mcp__ic-inline__show_image` to `permissions.allow` in your settings to stop the question.

## Errors

| Message | Cause |
| --- | --- |
| `this ic does not know --emit-rgba` | The `ic` on your `PATH` is older than the flag. Install it again (see [Install](#install)). |
| `could not run ic` | No `ic` is on the `PATH` of Claude Code. Install it, or set `icPath`. |
| `File not found`, `Failed to open image file` | `ic` read no picture at that path. Relative paths start at the working directory of the session. |

## Development

The tests drive the plugin through the engine of Claude Code, and they answer `process.run` as `ic` does:

```bash
claude plugin validate plugins/ic-inline
claude plugin test plugins/ic-inline
```

`tests/ic-inline.test.tsx` holds the real output of `ic --emit-rgba 720x720 tests/fixture.png`. If the JSON of `ic` changes, that test fails.

`hooks/register.tsx` is the one file that calls Claude Code. Claude Code reads a hooks module before it runs it, and it follows `$` only into the functions that the module declares at its top. The other modules are pure:

| Module | What it holds |
| --- | --- |
| `picture.ts` | The arguments of `ic`, the reading of its JSON and its errors, and the decodes kept for rows that draw again. |
| `cells.ts` | The size of a picture in cells, and the half-block cells of a `Raster`. |
| `draw.tsx` | The tree of one picture and its caption. |
| `argument.ts` | Paths: quotes, spaces, and `~`. |
| `options.ts` | The options, in range. |
