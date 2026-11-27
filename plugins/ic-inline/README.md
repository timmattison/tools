# ic-inline

A Claude Code plugin that shows pictures inline in the transcript. [`ic`](../../src/ic/README.md) decodes them.

- `/ic <image file>` shows a picture to you, under the command.
- `/ic diff [--mode MODE] <first> <second>` shows you one picture of the differences between two images, under the command.
- `show_image` is a tool for Claude. Claude uses it to show you a picture that it made or found: a chart, a diagram, a screenshot. The picture shows in the result row of the call.
- `show_diff` is a tool for Claude. Claude uses it to show you what changed between two images, for example a screenshot before and after a change. The picture shows in the result row of the call.

## Install

Install an `ic` that knows `--emit-rgba` and `--diff` (`ic --help` lists them). The plugin needs `--emit-rgba` for each picture, and `--diff` for the differences:

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

## Show the differences between two images

```
/ic diff before.png after.png
/ic diff --mode boxes before.png after.png
/ic diff --mode mask "~/shots/before change.png" ~/shots/after.png
```

The plugin runs `ic --diff [--diff-mode MODE] --emit-rgba 720x720 -- <first> <second>`, and it draws that picture the same way as `/ic` draws one image. The caption names the two files: `before.png vs after.png · 800×600`. When the two files have the same name, the caption shows the two paths: `old/shot.png vs new/shot.png`. [`ic --diff`](../../src/ic/README.md#show-the-differences-between-two-images) compares the two images pixel by pixel, with no tolerance, and it scales neither one.

`--mode` selects how the picture shows the differences. With no `--mode`, the plugin gives no `--diff-mode` to `ic`, thus `ic` applies its default, `highlight`. Red is RGBA `(255, 0, 0, 255)`.

| `--mode`              | A pixel that is the same | A pixel that differs                                    |
| --------------------- | ------------------------ | ------------------------------------------------------- |
| `boxes`               | the second image         | the second image, with a red outline around each region |
| `highlight` (default) | the first image          | opaque red                                              |
| `mask`                | opaque black             | opaque red                                              |

The rules of the command form:

- The first word is `diff`. Thus `/ic diff.png` and `/ic "diff"` still show one image.
- A path that holds a space must be in double quotes. A single quote is part of a path, thus `Tim's.png` needs no quotes.
- A path that starts with `~/` starts at your home directory, also in double quotes, the same as for `/ic`.
- `--mode MODE` or `--mode=MODE` can come before, between, or after the two paths.
- After the word `--`, each word is a path. Put `--` before a path that starts with `--`.

The command refuses each of these with a message that names the cause, and it runs no `ic`:

- A count of paths that is not two.
- A mode that is not `boxes`, `highlight`, or `mask`, and a `--mode` with no mode.
- A `--mode` that occurs two times.
- An option other than `--mode`.
- A double quote that is not closed, and an empty path (`""`).

`show_diff` takes `first`, `second`, and an optional `mode`, which is one of the three modes of the table. It runs the same `ic` command, and it refuses a call with the same cause: a path that is absent, empty, or not text, and a mode that is not one of the three.

## How it draws

Claude Code owns every cell of its screen, and it draws each cell again when something changes. An image that `ic photo.png` writes lands at a position that the next frame paints over. Thus the plugin does not let `ic` write to the terminal. It runs `ic --emit-rgba 720x720 -- <file>`, which prints the pixels as JSON, and it draws the pixels with the elements of Claude Code:

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

`show_image` and `show_diff` give Claude the size of the picture and not its content. To look at a picture, Claude uses `Read`. A failure, such as a missing file or a file that holds no picture, comes back to Claude as an error with the reason that `ic` gave.

If Claude Code asks your permission for a tool, add `mcp__ic-inline__show_image` or `mcp__ic-inline__show_diff` to `permissions.allow` in your settings to stop the question.

## Errors

| Message | Cause |
| --- | --- |
| `this ic does not know --emit-rgba`, `this ic does not know --diff` | The `ic` on your `PATH` is older than the flag. Install it again (see [Install](#install)). |
| `could not run ic` | No `ic` is on the `PATH` of Claude Code. Install it, or set `icPath`. |
| `File not found`, `Failed to open image file` | `ic` read no picture at that path. Relative paths start at the working directory of the session. |

## Development

The tests drive the plugin through the engine of Claude Code, and they answer `process.run` as `ic` does:

```bash
claude plugin validate plugins/ic-inline
claude plugin test plugins/ic-inline
```

`tests/ic-inline.test.tsx` holds a copy of the output of `ic --emit-rgba 720x720 -- tests/fixture.png`. The sandbox of `claude plugin test` runs no process, so this test cannot see a change of the JSON of `ic`. The Rust test `emit_rgba_prints_the_json_that_the_plugin_tests_hold` in `src/ic/tests/emit-rgba.rs` fails when the output of `ic` is not that copy word for word. When it fails, copy the new output into the test file.

`hooks/register.tsx` is the one file that calls Claude Code. Claude Code reads a hooks module before it runs it, and it follows `$` only into the functions that the module declares at its top. The other modules are pure:

| Module | What it holds |
| --- | --- |
| `picture.ts` | The modes of `--diff`, the arguments of `ic` for one image or for a diff, the reading of its JSON and its errors, and the decodes kept for rows that draw again. |
| `cells.ts` | The size of a picture in cells, and the half-block cells of a `Raster`. |
| `draw.tsx` | The tree of one picture and its caption. |
| `argument.ts` | The text after `/ic`: one path, or the diff form with its words, its `--mode`, and its refusals. Paths: quotes, spaces, and `~`. |
| `options.ts` | The options, in range. |
