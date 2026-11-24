// The behavior of ic-inline, through the engine alone: the /ic command, the
// show_image tool, the rows they draw, and the probe that decides between
// pixels and half blocks.
//
// The test kit runs no process, so every test answers `process.run` the way
// `ic --emit-rgba` answers. FIXTURE_JSON is the real output of that flag for
// tests/fixture.png, which holds the contract between the two halves: a change
// to the JSON of `ic` that this mod cannot read fails here.

import type { On, RenderSurface, RenderViewport } from 'claude-code'
import type { Engine } from 'claude-code/testing'
import { describe, expect, mock, test } from 'claude-code/testing'

const PLUGIN = 'ic-inline'
const COMMAND = 'ic'
const TOOL = 'mcp__ic-inline__show_image'

/** The path that the fixture answers to. */
const FIXTURE_PATH = 'fixture.png'

/**
 * `ic --emit-rgba 720x720 tests/fixture.png`, as the release build printed it.
 *
 * The picture is 4 by 4 pixels: the top two rows red, the bottom two rows
 * blue, and the top right pixel transparent.
 */
const FIXTURE_JSON =
  '{"width":4,"height":4,"source_width":4,"source_height":4,"rgba":"/wAA//8AAP//AAD/AAAAAP8AAP//AAD//wAA//8AAP8AAP//AAD//wAA//8AAP//AAD//wAA//8AAP//AAD//w=="}\n'

/** What an `ic` of a release before --emit-rgba writes for the flag. */
const OLD_IC_STDERR =
  "error: unexpected argument '--emit-rgba' found\n\n  tip: to pass '--emit-rgba' as a value, use '-- --emit-rgba'\n\nUsage: ic [OPTIONS] [FILE]...\n\nFor more information, try '--help'.\n"

const UPPER_HALF = 0x2580
const LOWER_HALF = 0x2584
const RED = 0xff0000
const BLUE = 0x0000ff
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
 * Answer `process.run` as `ic` would, from `answers` by the path argument, and
 * record every argument vector. A path with no answer is a missing file.
 */
function stubIc(on: On, answers: Record<string, IcAnswer>): { calls: string[][] } {
  stubEngine(on)
  const calls: string[][] = []
  on('process.run', async ($, e) => {
    calls.push([...e.argv])
    const path = e.argv[e.argv.length - 1] ?? ''
    const answer = answers[path] ?? {
      exitCode: 1,
      stdout: '',
      stderr: `Error: File not found: ${path}\n`,
    }
    return { value: { ...answer, isStdoutTruncated: false, isStderrTruncated: false } }
  })
  return { calls }
}

/** The fixture at FIXTURE_PATH, and nothing else. */
const stubFixture = (on: On) => stubIc(on, { [FIXTURE_PATH]: printed(FIXTURE_JSON) })

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

/** Record the `tool_use_id` of every picture that show_image keeps. */
function recordShown(on: On): string[] {
  const ids: string[] = []
  on('state.set', { plugin: PLUGIN, key: 'shown' }, ($, e, next) => {
    if (e.id !== undefined) ids.push(e.id)
    return next(e)
  })
  return ids
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

    expect(calls).toEqual([['ic', '--emit-rgba', '720x720', '/home/tim/pics/a.png']])
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
  test('registers /ic and show_image', async ($, on) => {
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
    expect(tools).toEqual(['show_image'])
  })
})

