// The behavior of ic-inline, through the engine: the /ic command and its diff
// form, the show_image and show_diff tools, the rows they draw, and the probe
// that decides between pixels and half blocks. One block calls the pure
// half-block arithmetic of hooks/cells.ts directly.
//
// The test kit runs no process, so every test answers `process.run` the way
// `ic --emit-rgba` answers. FIXTURE_JSON is a copy of the output of that flag
// for tests/fixture.png. These tests cannot see a change of the JSON of `ic`.
// The Rust test `emit_rgba_prints_the_json_that_the_plugin_tests_hold` in
// src/ic/tests/emit-rgba.rs holds the copy to the real output.

import type { On, RenderSurface, RenderViewport } from 'claude-code'
import type { Engine } from 'claude-code/testing'
import { describe, expect, mock, test } from 'claude-code/testing'

import { halfBlockCells } from '../hooks/cells'

const PLUGIN = 'ic-inline'
const COMMAND = 'ic'
const TOOL = 'mcp__ic-inline__show_image'
const DIFF_TOOL = 'mcp__ic-inline__show_diff'

/** The path that the fixture answers to. */
const FIXTURE_PATH = 'fixture.png'

/**
 * `ic --emit-rgba 720x720 -- tests/fixture.png`, as `ic` printed it. The Rust
 * test `emit_rgba_prints_the_json_that_the_plugin_tests_hold` holds this copy
 * to the real output.
 *
 * The picture is 4 by 4 pixels: the top two rows red, the bottom two rows
 * blue, and the top right pixel transparent.
 */
const FIXTURE_JSON =
  '{"width":4,"height":4,"source_width":4,"source_height":4,"rgba":"/wAA//8AAP//AAD/AAAAAP8AAP//AAD//wAA//8AAP8AAP//AAD//wAA//8AAP//AAD//wAA//8AAP//AAD//w=="}\n'

/** What an `ic` of a release before --emit-rgba writes for the flag. */
const OLD_IC_STDERR =
  "error: unexpected argument '--emit-rgba' found\n\n  tip: to pass '--emit-rgba' as a value, use '-- --emit-rgba'\n\nUsage: ic [OPTIONS] [FILE]...\n\nFor more information, try '--help'.\n"

/** What an `ic` of a release before --diff writes for the flag. */
const OLD_DIFF_STDERR =
  "error: unexpected argument '--diff' found\n\n  tip: to pass '--diff' as a value, use '-- --diff'\n\nUsage: ic [OPTIONS] [FILE]...\n\nFor more information, try '--help'.\n"

/** The two images that the diff tests compare. */
const FIRST = 'before.png'
const SECOND = 'after.png'

/** The modes of `ic --diff-mode`. */
const DIFF_MODES = ['boxes', 'highlight', 'mask'] as const

const UPPER_HALF = 0x2580
const LOWER_HALF = 0x2584
const SPACE = 0x20
const RED = 0xff0000
const BLUE = 0x0000ff
/** The mean of the four opaque pixels (200, 0, 0), (0, 200, 0), (0, 0, 200) and (100, 100, 100): 300 / 4 = 75 in each channel. */
const MEAN_GRAY = 0x4b4b4b
/** The color of a pixel [0, 255, 0, 255], full green. */
const GREEN = 0x00ff00
const TERMINAL_DEFAULT = 0x01000000

/** A transcript wide enough for every picture of these tests. */
const WIDE: RenderViewport = { columns: 120, rows: 50 }

/** How long the probe can take, as the mocked clock counts it. */
const PROBE_TIME_MS = 2_000

/** The text of the row that the engine draws itself, as the stub below draws it. */
const ENGINE_ROW = 'the engine draws this row'

/**
 * Stand for the engine beneath the plugin: a row it draws itself, a command or
 * a tool no plugin answered. The kit has no default for these events.
 */
function stubEngine(on: On): void {
  on('ui.render', ($, e) => {
    const { Text } = $.ui.resolve(e)
    return <Text>{ENGINE_ROW}</Text>
  })
  on('command.run', ($, e) => ({ text: `Unknown command: /${e.command}` }))
  on('tool.call', ($, e) => ({ deny: `No such tool: ${e.tool}` }))
}

/** What one fake run of `ic` answers. */
type IcAnswer = { exitCode: number; stdout: string; stderr: string }

/** The answer of a successful run that printed `json`. */
const printed = (json: string): IcAnswer => ({ exitCode: 0, stdout: json, stderr: '' })

/**
 * The JSON that `ic --emit-rgba` prints for an opaque gray picture of `width`
 * by `height` pixels.
 */
function grayJson(width: number, height: number): string {
  const rgba = new Uint8Array(width * height * 4).fill(128)
  return `${JSON.stringify({ width, height, source_width: width, source_height: height, rgba: rgba.toBase64() })}\n`
}

/**
 * The key under which stubIc answers a run of ic: the paths after `--`, joined
 * by ` vs `. One path is its own key, and the diff of two is `a.png vs b.png`.
 */
function pathsKey(argv: readonly string[]): string {
  return argv.slice(argv.indexOf('--') + 1).join(' vs ')
}

/**
 * Answer `process.run` as `ic` would, from `answers` by the paths of the run
 * (see pathsKey), and record every argument vector. Paths with no answer are a
 * missing file.
 */
function stubIc(on: On, answers: Record<string, IcAnswer>): { calls: string[][] } {
  stubEngine(on)
  const calls: string[][] = []
  on('process.run', async ($, e) => {
    calls.push([...e.argv])
    const key = pathsKey(e.argv)
    const answer = answers[key] ?? {
      exitCode: 1,
      stdout: '',
      stderr: `Error: File not found: ${key}\n`,
    }
    return { value: { ...answer, isStdoutTruncated: false, isStderrTruncated: false } }
  })
  return { calls }
}

/** The fixture at FIXTURE_PATH, and nothing else. */
const stubFixture = (on: On) => stubIc(on, { [FIXTURE_PATH]: printed(FIXTURE_JSON) })

/** The differences of FIRST and SECOND, printed as the fixture, and nothing else. */
const stubDiff = (on: On) => stubIc(on, { [`${FIRST} vs ${SECOND}`]: printed(FIXTURE_JSON) })

/** The props of the output row of `/ic <args>`. */
function commandRow(args: string, text: string, surface: RenderSurface = 'terminal', viewport = WIDE) {
  return {
    plugin: PLUGIN,
    surface,
    component: 'CommandOutput' as const,
    props: { command: COMMAND, args, text, isErrored: false },
    viewport,
  }
}

/** The props of the band above the prompt. */
function band() {
  return {
    plugin: PLUGIN,
    surface: 'terminal' as const,
    component: 'AbovePrompt' as const,
    props: {
      hasSurvey: false,
      isCollapsed: false,
      isWorking: false,
      maxRows: 10,
      bodyColumns: 100,
      scroll: { offset: 0, bodyRows: 10 },
      view: {},
    },
    viewport: WIDE,
  }
}

/** Run `/ic <args>` and give the text of its output row. */
async function runIc($: Engine, args: string): Promise<string> {
  const { text } = await $.command.run({
    command: COMMAND,
    args,
    origin: { kind: 'composer' },
    presentation: { isFullscreen: true, columns: WIDE.columns },
  })
  return text ?? ''
}

/** The cells of a Raster as `[codePoint, foreground, background]` triplets. */
function cellsOf(base64: string): number[][] {
  const bytes = Uint8Array.fromBase64(base64)
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const cells: number[][] = []
  for (let at = 0; at < bytes.byteLength; at += 12) {
    cells.push([view.getUint32(at, true), view.getUint32(at + 4, true), view.getUint32(at + 8, true)])
  }
  return cells
}

/**
 * Record the `tool_use_id` of every picture that a tool keeps under `key`:
 * `shown` for show_image, `diffed` for show_diff.
 */
function recordShown(on: On, key: 'shown' | 'diffed' = 'shown'): string[] {
  const ids: string[] = []
  // A matcher takes one key, not a union of keys, so the body reads the key.
  on('state.set', { plugin: PLUGIN }, ($, e, next) => {
    if (e.key === key && e.id !== undefined) ids.push(e.id)
    return next(e)
  })
  return ids
}

/** The props of the result row of the call `id` of `tool`, which gave `output`. */
function resultRow(tool: string, id: string, output: unknown) {
  return {
    plugin: PLUGIN,
    surface: 'terminal' as const,
    component: 'ToolResult' as const,
    requestId: id,
    props: { tool_use_id: id, tool, output, isErrored: false },
    viewport: WIDE,
  }
}

describe('/ic draws', () => {
  test('the picture as half blocks under the command, with a caption', async ($, on) => {
    stubFixture(on)

    const text = await runIc($, FIXTURE_PATH)
    expect(text).toContain(FIXTURE_PATH)
    expect(text).toContain('4×4')

    const ui = await $.ui.mount(commandRow(FIXTURE_PATH, text))
    const raster = await ui.find({ type: 'Raster' })
    expect(raster?.props.columns).toBe(4)
    expect(raster?.props.rows).toBe(2)
    expect(cellsOf(String(raster?.props.cells))).toEqual([
      [UPPER_HALF, RED, RED],
      [UPPER_HALF, RED, RED],
      [UPPER_HALF, RED, RED],
      [LOWER_HALF, RED, TERMINAL_DEFAULT],
      [UPPER_HALF, BLUE, BLUE],
      [UPPER_HALF, BLUE, BLUE],
      [UPPER_HALF, BLUE, BLUE],
      [UPPER_HALF, BLUE, BLUE],
    ])
    expect((await ui.find({ type: 'Text', text: /fixture\.png/ }))?.text).toContain('4×4')
  })

  test('the picture again without a second run of ic', async ($, on) => {
    const { calls } = stubFixture(on)

    const text = await runIc($, FIXTURE_PATH)
    await $.ui.mount(commandRow(FIXTURE_PATH, text))
    await $.ui.mount(commandRow(FIXTURE_PATH, text))

    expect(calls).toHaveLength(1)
  })

  test('the picture after a hot reload, when ic runs in the drawing', async ($, on) => {
    const { calls } = stubFixture(on)

    const ui = await $.ui.mount(commandRow(FIXTURE_PATH, 'Showed fixture.png'))

    expect(await ui.find({ type: 'Raster' })).toBeDefined()
    expect(calls).toHaveLength(1)
  })

  test('a wide picture at the width of the transcript, less the indent', async ($, on) => {
    stubIc(on, { 'wide.png': printed(grayJson(400, 100)) })

    const text = await runIc($, 'wide.png')
    const ui = await $.ui.mount(commandRow('wide.png', text, 'terminal', { columns: 40, rows: 50 }))
    const raster = await ui.find({ type: 'Raster' })

    expect(raster?.props.columns).toBe(36)
    expect(raster?.props.rows).toBe(5)
  })

  test('a tall picture no taller than maxRows', { options: { maxRows: 6 } }, async ($, on) => {
    stubIc(on, { 'tall.png': printed(grayJson(100, 400)) })

    const text = await runIc($, 'tall.png')
    const ui = await $.ui.mount(commandRow('tall.png', text))
    const raster = await ui.find({ type: 'Raster' })

    expect(raster?.props.rows).toBe(6)
    expect(raster?.props.columns).toBe(3)
  })

  test("the engine's own row on a surface that has no Raster", async ($, on) => {
    stubFixture(on)

    const text = await runIc($, FIXTURE_PATH)
    const ui = await $.ui.mount(commandRow(FIXTURE_PATH, text, 'desktop'))

    expect(await ui.find({ type: 'Text', text: ENGINE_ROW })).toBeDefined()
  })

  test("the engine's own row for an errored run", async ($, on) => {
    stubFixture(on)

    const ui = await $.ui.mount({
      ...commandRow(FIXTURE_PATH, 'boom'),
      props: { command: COMMAND, args: FIXTURE_PATH, text: 'boom', isErrored: true },
    })

    expect(await ui.find({ type: 'Text', text: ENGINE_ROW })).toBeDefined()
  })
})

describe('/ic reads its argument', () => {
  test('and gives ic the box and the path, with ~ expanded', async ($, on) => {
    mock.env(on, { HOME: '/home/tim' })
    const { calls } = stubIc(on, { '/home/tim/pics/a.png': printed(FIXTURE_JSON) })

    await runIc($, '~/pics/a.png')

    expect(calls).toEqual([['ic', '--emit-rgba', '720x720', '--', '/home/tim/pics/a.png']])
  })

  test('as a path after --, also from show_image, when the path starts with -', async ($, on) => {
    const { calls } = stubIc(on, { '-chart.png': printed(FIXTURE_JSON), '--help': printed(FIXTURE_JSON) })

    await runIc($, '-chart.png')
    await $.tool.call({ tool: TOOL, path: '--help' })

    expect(calls).toEqual([
      ['ic', '--emit-rgba', '720x720', '--', '-chart.png'],
      ['ic', '--emit-rgba', '720x720', '--', '--help'],
    ])
  })

  test('as one path, spaces and quotes included', async ($, on) => {
    const { calls } = stubIc(on, { 'my photo.png': printed(FIXTURE_JSON) })

    await runIc($, '"my photo.png"')
    await runIc($, '  my photo.png  ')

    expect(calls.map(argv => argv[argv.length - 1])).toEqual(['my photo.png', 'my photo.png'])
  })

  test('through the icPath option', { options: { icPath: '/opt/ic' } }, async ($, on) => {
    const { calls } = stubFixture(on)

    await runIc($, FIXTURE_PATH)

    expect(calls[0]?.[0]).toBe('/opt/ic')
  })

  test('and shows its usage when there is none', async ($, on) => {
    const { calls } = stubFixture(on)

    const text = await runIc($, '   ')

    expect(text).toContain('Usage: /ic <image file>')
    expect(text).toContain('/ic diff [--mode boxes|highlight|mask] <first> <second>')
    expect(calls).toHaveLength(0)
  })
})

describe('/ic reports', () => {
  test('what ic said when the file holds no picture, and draws no picture', async ($, on) => {
    stubIc(on, {
      'notes.png': {
        exitCode: 1,
        stdout: '',
        stderr: 'Error: Failed to open image file: notes.png\n\nCaused by:\n    The image format could not be determined\n',
      },
    })

    const text = await runIc($, 'notes.png')
    expect(text).toContain('Failed to open image file: notes.png')
    expect(text).toContain('The image format could not be determined')

    const ui = await $.ui.mount(commandRow('notes.png', text))
    expect(await ui.find({ type: 'Text', text: ENGINE_ROW })).toBeDefined()
  })

  test('an ic that does not know --emit-rgba, and how to update it', async ($, on) => {
    stubIc(on, { [FIXTURE_PATH]: { exitCode: 2, stdout: '', stderr: OLD_IC_STDERR } })

    const text = await runIc($, FIXTURE_PATH)

    expect(text).toContain('does not know --emit-rgba')
    expect(text).toContain('cargo install --git https://github.com/timmattison/tools ic')
  })

  test('an ic that does not run', async ($, on) => {
    stubEngine(on)
    on('process.run', () => ({ deny: 'spawn ic ENOENT' }))

    const text = await runIc($, FIXTURE_PATH)

    expect(text).toContain('could not run ic')
    expect(text).toContain('ENOENT')
  })

  test('output that is not the JSON of --emit-rgba', async ($, on) => {
    stubIc(on, { [FIXTURE_PATH]: printed('{"width":2}\n') })

    const text = await runIc($, FIXTURE_PATH)

    expect(text).toContain('could not read the output of ic')
  })

  /** The JSON of grayJson(2, 2), with `fields` in place of its own. */
  const grayWith = (fields: object) => `${JSON.stringify({ ...JSON.parse(grayJson(2, 2)), ...fields })}\n`

  for (const [wrong, stdout] of Object.entries({
    'a side above 720': grayJson(721, 1),
    'a side of 0': grayJson(0, 1),
    'a side that is not whole': grayJson(1.5, 2),
    'a source side of 0': grayWith({ source_width: 0 }),
    'an rgba that is not a string': grayWith({ rgba: 16 }),
    'an rgba that is not base64': grayWith({ rgba: '@@@@' }),
    'pixels that the size does not give': grayWith({ height: 3 }),
  })) {
    test(`output with ${wrong}`, async ($, on) => {
      stubIc(on, { [FIXTURE_PATH]: printed(stdout) })

      expect(await runIc($, FIXTURE_PATH)).toContain('could not read the output of ic')
    })
  }
})

describe('/ic diff reads its arguments', () => {
  test('and gives ic --diff, the box, and the two paths after --', async ($, on) => {
    const { calls } = stubDiff(on)

    await runIc($, `diff ${FIRST} ${SECOND}`)

    expect(calls).toEqual([['ic', '--diff', '--emit-rgba', '720x720', '--', FIRST, SECOND]])
  })

  for (const mode of DIFF_MODES) {
    test(`and gives ic --diff-mode ${mode} for --mode ${mode}`, async ($, on) => {
      const { calls } = stubDiff(on)

      await runIc($, `diff --mode ${mode} ${FIRST} ${SECOND}`)

      expect(calls).toEqual([['ic', '--diff', '--diff-mode', mode, '--emit-rgba', '720x720', '--', FIRST, SECOND]])
    })
  }

  test('with --mode before, between, or after the paths, and as --mode=MODE', async ($, on) => {
    const { calls } = stubDiff(on)

    await runIc($, `diff ${FIRST} --mode mask ${SECOND}`)
    await runIc($, `diff ${FIRST} ${SECOND} --mode mask`)
    await runIc($, `diff --mode=mask ${FIRST} ${SECOND}`)

    const masked = ['ic', '--diff', '--diff-mode', 'mask', '--emit-rgba', '720x720', '--', FIRST, SECOND]
    expect(calls).toEqual([masked, masked, masked])
  })

  test('as paths in double quotes, spaces included', async ($, on) => {
    const { calls } = stubIc(on, { 'my before.png vs my after.png': printed(FIXTURE_JSON) })

    await runIc($, '  diff "my before.png"   "my after.png"  ')

    expect(calls.map(argv => argv.slice(-2))).toEqual([['my before.png', 'my after.png']])
  })

  test('with ~ expanded in each path, quoted or not', async ($, on) => {
    mock.env(on, { HOME: '/home/tim' })
    const { calls } = stubIc(on, { '/home/tim/before.png vs /home/tim/my after.png': printed(FIXTURE_JSON) })

    await runIc($, 'diff ~/before.png "~/my after.png"')

    expect(calls.map(argv => argv.slice(-2))).toEqual([['/home/tim/before.png', '/home/tim/my after.png']])
  })

  test('as a path every word after --, also --mode and a word that starts with -', async ($, on) => {
    const { calls } = stubIc(on, { '--mode vs -after.png': printed(FIXTURE_JSON) })

    await runIc($, 'diff --mode mask -- --mode -after.png')

    expect(calls).toEqual([['ic', '--diff', '--diff-mode', 'mask', '--emit-rgba', '720x720', '--', '--mode', '-after.png']])
  })

  test('as one image for diff.png, and for diff in quotes', async ($, on) => {
    const { calls } = stubIc(on, { 'diff.png': printed(FIXTURE_JSON), diff: printed(FIXTURE_JSON) })

    await runIc($, 'diff.png')
    await runIc($, '"diff"')

    expect(calls).toEqual([
      ['ic', '--emit-rgba', '720x720', '--', 'diff.png'],
      ['ic', '--emit-rgba', '720x720', '--', 'diff'],
    ])
  })
})

describe('/ic diff draws', () => {
  test('the differences under the command, with the two names in the caption', async ($, on) => {
    stubDiff(on)
    const args = `diff ${FIRST} ${SECOND}`

    const text = await runIc($, args)
    expect(text).toContain(FIRST)
    expect(text).toContain(SECOND)
    expect(text).toContain('4×4')

    const ui = await $.ui.mount(commandRow(args, text))
    const raster = await ui.find({ type: 'Raster' })
    expect(raster?.props.columns).toBe(4)
    expect(raster?.props.rows).toBe(2)
    expect((await ui.find({ type: 'Text', text: /4×4/ }))?.text).toBe(`${FIRST} vs ${SECOND} · 4×4`)
  })

  test('only the two file names in the caption when the paths have folders', async ($, on) => {
    stubIc(on, { 'old/before.png vs new/after.png': printed(FIXTURE_JSON) })
    const args = 'diff old/before.png new/after.png'

    const ui = await $.ui.mount(commandRow(args, await runIc($, args)))

    expect((await ui.find({ type: 'Raster' }))?.props.columns).toBe(4)
    expect((await ui.find({ type: 'Text', text: /4×4/ }))?.text).toBe('before.png vs after.png · 4×4')
  })

  test('the two paths in the caption when the two files have the same name', async ($, on) => {
    stubIc(on, { 'old/shot.png vs new/shot.png': printed(FIXTURE_JSON) })
    const args = 'diff old/shot.png new/shot.png'

    const ui = await $.ui.mount(commandRow(args, await runIc($, args)))

    expect((await ui.find({ type: 'Raster' }))?.props.columns).toBe(4)
    expect((await ui.find({ type: 'Text', text: /4×4/ }))?.text).toBe('old/shot.png vs new/shot.png · 4×4')
  })

  test('the differences again without a second run of ic', async ($, on) => {
    const { calls } = stubDiff(on)
    const args = `diff ${FIRST} ${SECOND}`

    const text = await runIc($, args)
    await $.ui.mount(commandRow(args, text))
    await $.ui.mount(commandRow(args, text))

    expect(calls).toHaveLength(1)
  })

  test('the differences after a hot reload, in the mode of the command', async ($, on) => {
    const { calls } = stubDiff(on)

    const ui = await $.ui.mount(commandRow(`diff --mode highlight ${FIRST} ${SECOND}`, 'Showed'))

    expect(await ui.find({ type: 'Raster' })).toBeDefined()
    expect(calls).toEqual([['ic', '--diff', '--diff-mode', 'highlight', '--emit-rgba', '720x720', '--', FIRST, SECOND]])
  })
})

describe('/ic diff refuses', () => {
  for (const { cause, args, reason } of [
    { cause: 'no path', args: 'diff', reason: 'takes exactly two paths, and it got 0' },
    { cause: 'one path', args: 'diff a.png', reason: 'takes exactly two paths, and it got 1' },
    { cause: 'three paths', args: 'diff a.png b.png c.png', reason: 'takes exactly two paths, and it got 3' },
    {
      cause: 'a mode that ic does not have',
      args: 'diff --mode red a.png b.png',
      reason: '--mode takes boxes, highlight or mask, and it got red',
    },
    { cause: '--mode with no mode', args: 'diff a.png b.png --mode', reason: '--mode needs a mode: boxes, highlight or mask' },
    { cause: '--mode= with no mode', args: 'diff --mode= a.png b.png', reason: '--mode needs a mode: boxes, highlight or mask' },
    { cause: '--mode two times', args: 'diff --mode mask --mode boxes a.png b.png', reason: '--mode occurs two times' },
    { cause: 'an option that it does not know', args: 'diff --fast a.png b.png', reason: 'does not know the option --fast' },
    { cause: 'a double quote that is not closed', args: 'diff "a.png b.png', reason: 'a double quote is not closed' },
    { cause: 'an empty path', args: 'diff "" b.png', reason: 'a path is empty' },
  ]) {
    test(`${cause}, with the cause and the usage, and runs no ic`, async ($, on) => {
      const { calls } = stubDiff(on)

      const text = await runIc($, args)
      expect(text).toContain(reason)
      expect(text).toContain('Usage: /ic diff [--mode boxes|highlight|mask] <first> <second>')
      expect(calls).toHaveLength(0)

      const ui = await $.ui.mount(commandRow(args, text))
      expect(await ui.find({ type: 'Text', text: ENGINE_ROW })).toBeDefined()
    })
  }
})

describe('/ic diff reports', () => {
  test('what ic said when a file does not open, and draws no picture', async ($, on) => {
    stubDiff(on)
    const args = `diff ${FIRST} missing.png`

    const text = await runIc($, args)
    expect(text).toContain(`Could not compare ${FIRST} and missing.png`)
    expect(text).toContain('File not found')

    const ui = await $.ui.mount(commandRow(args, text))
    expect(await ui.find({ type: 'Text', text: ENGINE_ROW })).toBeDefined()
  })

  test('an ic that does not know --diff, and how to update it', async ($, on) => {
    stubIc(on, { [`${FIRST} vs ${SECOND}`]: { exitCode: 2, stdout: '', stderr: OLD_DIFF_STDERR } })

    const text = await runIc($, `diff ${FIRST} ${SECOND}`)

    expect(text).toContain('does not know --diff')
    expect(text).toContain('cargo install --git https://github.com/timmattison/tools ic')
  })
})

describe('show_image', () => {
  test('shows the picture in the result row of the call', async ($, on) => {
    stubFixture(on)
    const ids = recordShown(on)

    const called = await $.tool.call({ tool: TOOL, path: FIXTURE_PATH })
    expect(called.deny).toBeUndefined()
    expect(String(called.result)).toContain('Showed fixture.png to the user')
    expect(String(called.result)).toContain('4×4')

    const id = ids[0] ?? ''
    expect(id).not.toBe('')
    const ui = await $.ui.mount({
      plugin: PLUGIN,
      surface: 'terminal',
      component: 'ToolResult',
      requestId: id,
      props: { tool_use_id: id, tool: TOOL, output: called.result, isErrored: false },
      viewport: WIDE,
    })
    expect(await ui.find({ type: 'Raster' })).toBeDefined()
  })

  test('refuses a file that ic cannot read, with the reason', async ($, on) => {
    stubFixture(on)

    const called = await $.tool.call({ tool: TOOL, path: 'missing.png' })

    expect(called.deny).toContain('File not found: missing.png')
  })

  test('refuses a call with no path', async ($, on) => {
    stubFixture(on)

    const called = await $.tool.call({ tool: TOOL })

    expect(called.deny).toContain('path')
  })

  test("leaves the result row of another call to the engine", async ($, on) => {
    stubFixture(on)

    const ui = await $.ui.mount({
      plugin: PLUGIN,
      surface: 'terminal',
      component: 'ToolResult',
      requestId: 'toolu_unknown',
      props: { tool_use_id: 'toolu_unknown', tool: TOOL, output: 'Showed', isErrored: false },
      viewport: WIDE,
    })

    expect(await ui.find({ type: 'Text', text: ENGINE_ROW })).toBeDefined()
  })
})

describe('show_diff', () => {
  test('gives ic --diff, the box, and the two paths after --, with no mode', async ($, on) => {
    const { calls } = stubDiff(on)

    await $.tool.call({ tool: DIFF_TOOL, first: FIRST, second: SECOND })

    expect(calls).toEqual([['ic', '--diff', '--emit-rgba', '720x720', '--', FIRST, SECOND]])
  })

  for (const mode of DIFF_MODES) {
    test(`gives ic --diff-mode ${mode} for the mode ${mode}`, async ($, on) => {
      const { calls } = stubDiff(on)

      await $.tool.call({ tool: DIFF_TOOL, first: FIRST, second: SECOND, mode })

      expect(calls).toEqual([['ic', '--diff', '--diff-mode', mode, '--emit-rgba', '720x720', '--', FIRST, SECOND]])
    })
  }

  test('reads ~ and a path that starts with - as show_image does', async ($, on) => {
    mock.env(on, { HOME: '/home/tim' })
    const { calls } = stubIc(on, { '/home/tim/before.png vs --help': printed(FIXTURE_JSON) })

    await $.tool.call({ tool: DIFF_TOOL, first: '~/before.png', second: '--help' })

    expect(calls.map(argv => argv.slice(-3))).toEqual([['--', '/home/tim/before.png', '--help']])
  })

  test('shows the differences in the result row of the call', async ($, on) => {
    stubDiff(on)
    const ids = recordShown(on, 'diffed')

    const called = await $.tool.call({ tool: DIFF_TOOL, first: FIRST, second: SECOND, mode: 'mask' })
    expect(called.deny).toBeUndefined()
    expect(String(called.result)).toContain(`Showed the differences between ${FIRST} and ${SECOND} to the user`)
    expect(String(called.result)).toContain('4×4')

    const id = ids[0] ?? ''
    expect(id).not.toBe('')
    const ui = await $.ui.mount(resultRow(DIFF_TOOL, id, called.result))
    expect(await ui.find({ type: 'Raster' })).toBeDefined()
    expect((await ui.find({ type: 'Text', text: /before\.png vs after\.png/ }))?.text).toBe(`${FIRST} vs ${SECOND} · 4×4`)
  })

  test('draws the result row in the mode of the call, from the decode of the call', async ($, on) => {
    const { calls } = stubDiff(on)
    const ids = recordShown(on, 'diffed')

    const called = await $.tool.call({ tool: DIFF_TOOL, first: FIRST, second: SECOND, mode: 'highlight' })
    const ui = await $.ui.mount(resultRow(DIFF_TOOL, ids[0] ?? '', called.result))

    // A row that lost the mode would ask for a decode of another argument vector, and run ic again.
    expect(await ui.find({ type: 'Raster' })).toBeDefined()
    expect(calls).toEqual([['ic', '--diff', '--diff-mode', 'highlight', '--emit-rgba', '720x720', '--', FIRST, SECOND]])
  })

  for (const { cause, input } of [
    { cause: 'no second path', input: { first: FIRST } },
    { cause: 'a path that is not text', input: { first: FIRST, second: 7 } },
    { cause: 'an empty path', input: { first: '  ', second: SECOND } },
  ]) {
    test(`refuses ${cause}, and runs no ic`, async ($, on) => {
      const { calls } = stubDiff(on)

      const called = await $.tool.call({ tool: DIFF_TOOL, ...input })

      expect(called.deny).toContain('show_diff needs two paths: first and second')
      expect(calls).toHaveLength(0)
    })
  }

  for (const { cause, mode, got } of [
    { cause: 'a mode that ic does not have', mode: 'red', got: 'red' },
    { cause: 'a mode that is not text', mode: 3, got: '3' },
  ]) {
    test(`refuses ${cause}, with the modes, and runs no ic`, async ($, on) => {
      const { calls } = stubDiff(on)

      const called = await $.tool.call({ tool: DIFF_TOOL, first: FIRST, second: SECOND, mode })

      expect(called.deny).toContain(`show_diff takes a mode of boxes, highlight or mask, and it got ${got}`)
      expect(calls).toHaveLength(0)
    })
  }

  test('refuses a file that ic cannot read, with the reason', async ($, on) => {
    stubDiff(on)

    const called = await $.tool.call({ tool: DIFF_TOOL, first: FIRST, second: 'missing.png' })

    expect(called.deny).toContain(`Could not compare ${FIRST} and missing.png`)
    expect(called.deny).toContain('File not found')
  })

  test('leaves the result row of another call to the engine', async ($, on) => {
    stubDiff(on)

    const ui = await $.ui.mount(resultRow(DIFF_TOOL, 'toolu_unknown', 'Showed'))

    expect(await ui.find({ type: 'Text', text: ENGINE_ROW })).toBeDefined()
  })
})

describe('pixels', () => {
  test('draw as an Image once the terminal takes one', async ($, on) => {
    const clock = mock.clock(on)
    stubFixture(on)
    const blits: unknown[] = []
    on('ui.blit', ($, e) => {
      blits.push(e)
      return { value: {} }
    })

    const prompt = await $.ui.mount(band())
    expect(await prompt.find({ type: 'Image' })).toBeDefined()
    await clock.advance(PROBE_TIME_MS)
    await clock.settle()

    expect(blits.length).toBeGreaterThan(0)

    const text = await runIc($, FIXTURE_PATH)
    const ui = await $.ui.mount(commandRow(FIXTURE_PATH, text))
    const image = await ui.find({ type: 'Image' })
    expect(image?.props.source).toEqual({
      rgba: '/wAA//8AAP//AAD/AAAAAP8AAP//AAD//wAA//8AAP8AAP//AAD//wAA//8AAP//AAD//wAA//8AAP//AAD//w==',
      width: 4,
      height: 4,
    })
    expect(image?.props.columns).toBe(1)
    expect(image?.props.rows).toBe(1)
    expect(await ui.find({ type: 'Raster' })).toBeUndefined()
  })

  test('stay half blocks where the terminal draws the alt text', async ($, on) => {
    const clock = mock.clock(on)
    stubFixture(on)
    on('ui.blit', () => ({ value: { deny: 'the Image draws its alt there: no placeholder images' } }))

    await $.ui.mount(band())
    await clock.advance(PROBE_TIME_MS)
    await clock.settle()

    const text = await runIc($, FIXTURE_PATH)
    const ui = await $.ui.mount(commandRow(FIXTURE_PATH, text))
    expect(await ui.find({ type: 'Raster' })).toBeDefined()
    expect(await ui.find({ type: 'Image' })).toBeUndefined()
  })

  test('draw as an Image with no probe under renderer image', { options: { renderer: 'image' } }, async ($, on) => {
    stubFixture(on)
    const blits: unknown[] = []
    on('ui.blit', ($, e) => {
      blits.push(e)
      return { value: {} }
    })

    const prompt = await $.ui.mount(band())
    expect(await prompt.find({ type: 'Image' })).toBeUndefined()

    const text = await runIc($, FIXTURE_PATH)
    const ui = await $.ui.mount(commandRow(FIXTURE_PATH, text))
    expect(await ui.find({ type: 'Image' })).toBeDefined()
    expect(blits).toHaveLength(0)
  })

  test('never probe under renderer blocks', { options: { renderer: 'blocks' } }, async ($, on) => {
    stubFixture(on)

    const prompt = await $.ui.mount(band())

    expect(await prompt.find({ type: 'Image' })).toBeUndefined()
    expect(await prompt.find({ type: 'Text', text: ENGINE_ROW })).toBeDefined()
  })
})

describe('the session', () => {
  test('registers /ic, show_image and show_diff', async ($, on) => {
    const commands: string[] = []
    const tools: string[] = []
    on('command.register', ($, e) => {
      commands.push(e.name)
      return { value: { command: e.name } }
    })
    on('tool.register', ($, e) => {
      tools.push(e.name)
      return { value: { tool: TOOL } }
    })
    on('session.start', ($, e) => ({ cwd: e.cwd }))

    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: true })

    expect(commands).toEqual([COMMAND])
    expect(tools).toEqual(['show_image', 'show_diff'])
  })
})

/**
 * The bytes of a picture from a table of rows. Each row is a list of pixels,
 * and each pixel is `[red, green, blue, alpha]`.
 */
function rgbaOf(rows: number[][][]): Uint8Array {
  return Uint8Array.from(rows.flat(2))
}

describe('half blocks', () => {
  test('take the mean of the pixels that each half covers, weighted by alpha, and a space where both halves are transparent', () => {
    const opaqueRed = [255, 0, 0, 255]
    const clearBlue = [0, 0, 255, 0]
    const clear = [0, 0, 0, 0]
    const clearWhite = [255, 255, 255, 0]
    const green = [0, 255, 0, 255]
    const pixels = rgbaOf([
      [opaqueRed, opaqueRed, [200, 0, 0, 255], [0, 200, 0, 255], clearWhite, clearWhite],
      [opaqueRed, clearBlue, [0, 0, 200, 255], [100, 100, 100, 255], clearWhite, clearWhite],
      [clear, clear, green, green, clearWhite, clearWhite],
      [clear, clear, green, green, clearWhite, clearWhite],
    ])

    const cells = cellsOf(halfBlockCells(pixels, { width: 6, height: 4 }, { columns: 3, rows: 1 }))

    expect(cells).toEqual([
      [UPPER_HALF, RED, TERMINAL_DEFAULT],
      [UPPER_HALF, MEAN_GRAY, GREEN],
      [SPACE, TERMINAL_DEFAULT, TERMINAL_DEFAULT],
    ])
  })
})
