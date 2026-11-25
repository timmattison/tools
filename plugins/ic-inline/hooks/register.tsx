// ic-inline: pictures inline in the Claude Code transcript, decoded by ic.
//
// Claude Code owns every cell of its screen and draws each one again when
// something changes, so the escape sequences that `ic photo.png` writes land
// where the next frame paints over them. This mod runs `ic --emit-rgba`
// instead, which prints the pixels, and it draws them with the engine's own
// elements: an Image where the terminal draws pixels, colored half blocks
// everywhere else (zellij, tmux, iTerm2, mosh).
//
// The ways to show a picture:
//   /ic <file>                  the person shows one, in the output row of the command
//   /ic diff [--mode M] <a> <b> the person shows the differences of two, the same way
//   show_image(path)            the model shows one, in the result row of the call
//
// This file is the one that talks to the engine. The engine reads a hooks
// module before it runs it, and it follows `$` only into the functions that
// this file declares at its top, so every hook and every call on `$` is here.
// The modules beside it are pure: picture (the arguments and the output of
// ic, and the kept decodes), cells (sizes and half blocks), draw (the tree of
// one picture), argument (paths) and options (userConfig).

import type { EngineInterface, Register, RenderElement, RenderViewport } from 'claude-code'

import { type Asked, fromHome, parseCommand, parsePath, startsAtHome } from './argument'
import { type Drawing, pictureTree, type PictureElements } from './draw'
import { type Options, readOptions, type Renderer } from './options'
import {
  DIFF_MODES,
  Decodes,
  IC_TIMEOUT_MS,
  icArguments,
  type Loaded,
  loadedFrom,
  notRun,
  type Picture,
  type Subject,
} from './picture'

const SHOWN = { plugin: 'ic-inline', key: 'shown' } as const
const PIXELS = { plugin: 'ic-inline', key: 'pixels' } as const

const COMMAND = 'ic'
const TOOL_NAME = 'show_image'
const TOOL = 'mcp__ic-inline__show_image'

/** The diff form of /ic, as its usage names it. */
const DIFF_FORM = `/ic diff [--mode ${DIFF_MODES.join('|')}] <first> <second>`

const USAGE = `Usage: /ic <image file>, or ${DIFF_FORM}. It shows the picture inline, decoded by ic.`

/** The usage that a refusal of the diff form ends with. */
const DIFF_USAGE = `Usage: ${DIFF_FORM}. It shows the differences of two images inline, decoded by ic --diff.`

const TOOL_DESCRIPTION = [
  'Show an image file to the user inline in the terminal transcript, decoded by ic.',
  'Use it to show the user a picture: a chart or a diagram you rendered, a screenshot, a photo.',
  'The user sees the picture. You get its size alone, not its content: to look at it yourself, use Read.',
  'PNG, JPEG, GIF, WebP, BMP, TIFF and the other formats of the Rust image crate work. A video does not.',
].join(' ')

/** The key of the probe Image in the band above the prompt. */
const PROBE_KEY = 'ic-inline-probe'

/** One transparent pixel: what the probe Image shows where it shows pixels. */
const PROBE_SOURCE = { rgba: 'AAAAAA==', width: 1, height: 1 } as const

/** The wait before each blit of the probe, so the band can draw it first. */
const PROBE_INTERVAL_MS = 250

/** The blits of the probe before the answer is `no`. */
const PROBE_ATTEMPTS = 6

/** The decodes that the rows draw again. A reload of the module starts it over. */
const decodes = new Decodes()

/**
 * The probe: whether this terminal draws an Image as pixels, asked of the
 * engine itself.
 *
 * The engine draws an Image as pixels in kitty and Ghostty and as its alt
 * text everywhere else, and no variable of the environment says which: a list
 * of terminals here would part company with the engine's own list the day
 * either one changed. But the engine refuses a blit of an Image that draws its
 * alt text. So under `auto` the band above the prompt holds one transparent
 * Image of one cell while the answer is unknown, and a few blits ask about it.
 * Until the answer is `yes`, pictures draw as half blocks, so a wrong `no`
 * costs resolution and never a picture.
 */
const probe: { requestId: string | undefined; isRunning: boolean } = {
  requestId: undefined,
  isRunning: false,
}

export const register: Register = (on, rawOptions) => {
  const options = readOptions(rawOptions)

  on('session.start', async ($, e, next) => {
    await $.command.register({
      name: COMMAND,
      description: 'Show an image, or the differences of two, inline in the transcript, decoded by ic',
      argumentHint: `<image file> | diff [--mode ${DIFF_MODES.join('|')}] <first> <second>`,
    })
    await $.tool.register({
      name: TOOL_NAME,
      description: TOOL_DESCRIPTION,
      inputSchema: {
        type: 'object',
        properties: {
          path: {
            type: 'string',
            description: 'The image file: absolute, relative to the working directory, or starting with ~/.',
          },
        },
        required: ['path'],
      },
    })
    return next(e)
  })

  on('command.run', { command: COMMAND }, async ($, e) => {
    const asked = await askedOf($, e.args)
    if (asked.kind === 'empty') return { text: USAGE }
    if (asked.kind === 'refused') return { text: `${asked.reason}. ${DIFF_USAGE}` }

    const loaded = await load($, options.icPath, asked, true)
    if (!loaded.isLoaded) return { text: `Could not ${failedOf(asked)}: ${loaded.reason}` }
    return { text: `Showed ${shownOf(asked, loaded.picture)} inline.` }
  })

  on('ui.render', { component: 'CommandOutput', props: { command: COMMAND } }, async ($, e, next) => {
    if (e.surface !== 'terminal' || e.props.isErrored) return next(e)
    const asked = await askedOf($, e.props.args)
    if (asked.kind === 'empty' || asked.kind === 'refused') return next(e)

    return (await drawRow($, $.ui.resolve(e), e.viewport, asked, options)) ?? next(e)
  })

  on('tool.call', { tool: TOOL }, async ($, e) => {
    const given: unknown = 'path' in e ? e.path : undefined
    const path = typeof given === 'string' ? await pathOf($, given) : undefined
    if (path === undefined) return { deny: `${TOOL_NAME} needs a path: the image file to show.` }

    const loaded = await load($, options.icPath, { kind: 'image', path }, true)
    if (!loaded.isLoaded) return { deny: `Could not show ${path}: ${loaded.reason}` }

    await $.state.set({ ...SHOWN, id: e.tool_use_id }, { path })
    const { sourceWidth, sourceHeight } = loaded.picture
    return {
      result: `Showed ${path} to the user inline (${sourceWidth}×${sourceHeight} pixels). The user sees it; you do not.`,
    }
  }).catch(($, e, next) =>
    // The hook never calls `next`: nothing beneath it serves this tool, so a
    // hook that failed answers with its reason in place of a bare failure.
    next.called ? next(e) : { deny: `${TOOL_NAME} failed inside ic-inline: ${next.error.message ?? next.error.kind}` },
  )

  on('ui.render', { component: 'ToolResult', props: { tool: TOOL } }, async ($, e, next) => {
    if (e.surface !== 'terminal' || e.props.isErrored) return next(e)
    const { value: shown } = await $.state.get({ ...SHOWN, id: e.requestId })
    if (shown === undefined) return next(e)

    const subject: Subject = { kind: 'image', path: shown.path }
    return (await drawRow($, $.ui.resolve(e), e.viewport, subject, options)) ?? next(e)
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    if (options.renderer !== 'auto' || e.surface !== 'terminal') return next(e)
    const { value = 'unknown' } = await $.state.get(PIXELS)
    if (value !== 'unknown') return next(e)

    probe.requestId = e.requestId
    if (!probe.isRunning) {
      probe.isRunning = true
      $.clock.after(PROBE_INTERVAL_MS, () => probeLater($, 1))
    }

    const below = await next(e)
    const { Box, Image } = $.ui.resolve(e)
    return (
      <Box flexDirection="column">
        {below}
        <Image key={PROBE_KEY} source={PROBE_SOURCE} columns={1} rows={1} alt=" " />
      </Box>
    )
  })
}

/**
 * The path that ic reads for `text`: `undefined` for no path, and a leading
 * `~` made into HOME, which is read only when the path needs it.
 */
async function pathOf($: EngineInterface, text: string): Promise<string | undefined> {
  const path = parsePath(text)
  return path === undefined ? undefined : homePath($, path)
}

/** `path` with a leading `~` made into HOME, which is read only when the path needs it. */
async function homePath($: EngineInterface, path: string): Promise<string> {
  return startsAtHome(path) ? fromHome(path, await $.env.get('HOME')) : path
}

/** What the text after /ic asks for, with each path that ic reads made from HOME as `pathOf` makes it. */
async function askedOf($: EngineInterface, text: string): Promise<Asked> {
  const asked = parseCommand(text)
  if (asked.kind === 'image') return { kind: 'image', path: await homePath($, asked.path) }
  if (asked.kind === 'diff') {
    return { ...asked, first: await homePath($, asked.first), second: await homePath($, asked.second) }
  }
  return asked
}

/**
 * The picture of `subject`, decoded by the ic at `icPath`. `isFresh` runs ic
 * again, as a request of the person or the model does: a file can have
 * changed. A draw of a row takes the kept decode. Never rejects.
 */
function load($: EngineInterface, icPath: string, subject: Subject, isFresh: boolean): Promise<Loaded> {
  const argv = icArguments(icPath, subject)
  const held = isFresh ? undefined : decodes.held(argv)
  if (held !== undefined) return held

  const loading = runIc($, icPath, argv)
  decodes.keep(argv, loading)
  return loading
}

/** Run `argv`, which starts with the ic at `icPath`, and read what it printed. Never rejects. */
async function runIc($: EngineInterface, icPath: string, argv: string[]): Promise<Loaded> {
  try {
    const run = await $.process.run(argv, { timeoutMs: IC_TIMEOUT_MS })
    return loadedFrom(run, icPath)
  } catch (error) {
    return notRun(icPath, error)
  }
}

/** What a request for `subject` could not do, for the message of a failure. */
function failedOf(subject: Subject): string {
  return subject.kind === 'image' ? `show ${subject.path}` : `compare ${subject.first} and ${subject.second}`
}

/** What a request for `subject` showed, with the size of the picture in the file. */
function shownOf(subject: Subject, picture: Picture): string {
  const size = `${picture.sourceWidth}×${picture.sourceHeight}`
  return subject.kind === 'image'
    ? `${subject.path} (${size})`
    : `the differences between ${subject.first} and ${subject.second} (${size})`
}

/**
 * How pictures draw: always pixels under `image`, never under `blocks`, and
 * under `auto` once the probe says that the terminal draws them. Read while a
 * row draws, it subscribes the row, which draws again when the probe ends.
 */
async function drawingOf($: EngineInterface, renderer: Renderer): Promise<Drawing> {
  if (renderer !== 'auto') return renderer === 'image' ? 'pixels' : 'blocks'
  const { value } = await $.state.get(PIXELS)
  return value === 'yes' ? 'pixels' : 'blocks'
}

/** The tree of the picture of `subject`, or `undefined` when ic gives none. */
async function drawRow(
  $: EngineInterface,
  elements: PictureElements,
  viewport: RenderViewport | undefined,
  subject: Subject,
  options: Options,
): Promise<RenderElement | undefined> {
  const loaded = await load($, options.icPath, subject, false)
  if (!loaded.isLoaded) return undefined
  const drawing = await drawingOf($, options.renderer)
  return pictureTree(elements, viewport, loaded.picture, subject, drawing, options.maxRows)
}

/** Run a blit of the probe from a timer, its failure to the debug log. */
function probeLater($: EngineInterface, attempt: number): void {
  blitProbe($, attempt).catch((error: unknown) => {
    $.ui.log(`ic-inline: the probe of the terminal failed: ${String(error)}`, { to: 'debug' })
  })
}

/** Blit the probe Image: taken is `yes`, refused PROBE_ATTEMPTS times is `no`. */
async function blitProbe($: EngineInterface, attempt: number): Promise<void> {
  let deny: string | undefined
  try {
    deny =
      probe.requestId === undefined
        ? 'the band has not drawn'
        : (await $.ui.blit({ requestId: probe.requestId, key: PROBE_KEY, source: PROBE_SOURCE })).deny
  } catch (error) {
    deny = String(error)
  }

  if (deny !== undefined && attempt < PROBE_ATTEMPTS) {
    $.clock.after(PROBE_INTERVAL_MS, () => probeLater($, attempt + 1))
    return
  }
  try {
    await $.state.set(PIXELS, deny === undefined ? 'yes' : 'no')
  } finally {
    probe.isRunning = false
  }
}
