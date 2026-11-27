// What ic gives: the arguments of `ic --emit-rgba` for one image or for the
// differences of two, the reading of what it printed, and the decodes kept for
// the rows that draw a picture again.
//
// Pure: register.tsx runs ic through the engine and hands the result here.
// The engine follows `$` into no module but the hooks module, so every call
// on `$` stays there.

/** A picture that ic decoded. */
export type Picture = {
  /** The width of the decoded picture, in pixels. */
  width: number
  /** The height of the decoded picture, in pixels. */
  height: number
  /** The width of the picture in the file, in pixels. */
  sourceWidth: number
  /** The height of the picture in the file, in pixels. */
  sourceHeight: number
  /** The pixels as ic printed them: 4 bytes each (RGBA), row by row, base64. */
  rgba: string
  /** The same pixels, decoded. */
  pixels: Uint8Array
}

/** What a request for a picture gave: the picture, or why there is none. */
export type Loaded = { isLoaded: true; picture: Picture } | { isLoaded: false; reason: string }

/** How one run of ic ended, as `$.process.run` resolves it. */
export type IcRun = { exitCode: number; stdout: string; stderr: string }

/** How the picture of `ic --diff` shows the differences: `--diff-mode`. */
export type DiffMode = 'boxes' | 'highlight' | 'mask'

/** The modes of `ic --diff-mode`, in the order that a message names them. */
export const DIFF_MODES: readonly DiffMode[] = ['boxes', 'highlight', 'mask']

/** The modes as a sentence names them: `boxes, highlight or mask`. */
export const DIFF_MODE_NAMES = `${DIFF_MODES.slice(0, -1).join(', ')} or ${DIFF_MODES.at(-1) ?? ''}`

/** Whether `value` is one of the modes of `ic --diff-mode`. */
export function isDiffMode(value: unknown): value is DiffMode {
  return DIFF_MODES.some(mode => mode === value)
}

/**
 * What one run of ic decodes: one image file, or the picture of the
 * differences of two. A diff with no mode takes the default mode of ic.
 */
export type Subject =
  | { kind: 'image'; path: string }
  | { kind: 'diff'; first: string; second: string; mode: DiffMode | undefined }

/**
 * The box that ic fits every picture inside, in pixels.
 *
 * One decode serves both ways to draw: an Image takes these pixels as its
 * source (the engine takes at most 2 MiB of them, and 720 by 720 by 4 bytes is
 * just under), and the half blocks of a Raster sample them down to a few
 * hundred columns at most.
 */
const DECODE_BOX = '720x720'

/** The widest or tallest picture that ic can print inside DECODE_BOX. */
const DECODE_SIDE = 720

/** How long one run of ic can take. A photograph decodes in well under it. */
export const IC_TIMEOUT_MS = 30_000

/** The most decodes kept. The oldest one goes first. */
const KEPT_DECODES = 16

/** The bytes of one RGBA pixel. */
const BYTES_PER_PIXEL = 4

/** How to put an ic that knows --emit-rgba and --diff on this machine. */
const UPDATE_IC = 'cargo install --git https://github.com/timmattison/tools ic'

/** What clap prints when ic does not know a flag. The flag is the group. */
const UNKNOWN_FLAG = /unexpected argument '(--[a-z-]+)'/

/**
 * The argument vector that decodes `subject` with the ic at `icPath`.
 *
 * Each path goes after `--`, so a path that starts with `-` stays a path and
 * clap does not read it as options. A diff gives `--diff-mode` only when it
 * names a mode, so ic applies its own default.
 *
 * @param icPath the ic executable
 * @param subject the file, or the two files to compare, each absolute or
 *   relative to the session's directory
 */
export function icArguments(icPath: string, subject: Subject): string[] {
  if (subject.kind === 'image') return [icPath, '--emit-rgba', DECODE_BOX, '--', subject.path]

  const mode = subject.mode === undefined ? [] : ['--diff-mode', subject.mode]
  return [icPath, '--diff', ...mode, '--emit-rgba', DECODE_BOX, '--', subject.first, subject.second]
}

/**
 * The picture that a finished run of ic printed, or why there is none.
 *
 * @param run how the run ended
 * @param icPath the ic that ran, for the reason of a failure
 */
export function loadedFrom(run: IcRun, icPath: string): Loaded {
  if (run.exitCode !== 0) {
    const flag = UNKNOWN_FLAG.exec(run.stderr)?.[1]
    if (flag !== undefined) {
      return { isLoaded: false, reason: `this ${icPath} does not know ${flag}. Update it: ${UPDATE_IC}` }
    }
    return { isLoaded: false, reason: oneLine(run.stderr) || `${icPath} exited with ${run.exitCode}` }
  }

  const picture = readEmitted(run.stdout)
  return picture === undefined
    ? { isLoaded: false, reason: `could not read the output of ${icPath}: ${oneLine(run.stdout).slice(0, 80)}` }
    : { isLoaded: true, picture }
}

/**
 * Why there is no picture when ic did not run at all.
 *
 * @param icPath the ic that did not run
 * @param error what the engine rejected the run with
 */
export function notRun(icPath: string, error: unknown): Loaded {
  const message = error instanceof Error ? error.message : String(error)
  return { isLoaded: false, reason: `could not run ${icPath}: ${message}` }
}

/**
 * The decodes kept for the rows of the transcript, by the argument vector of
 * the run of ic that made each one: the ic, the mode, and the paths.
 *
 * A row draws again when the width of the window changes and after a hot
 * reload, and each draw asks for the same picture. A request of the person or
 * of the model decodes again, because a file can have changed. A failure is
 * not kept: a file can appear, or the run can have been cut short.
 */
export class Decodes {
  /** The decodes, newest last, by the key of their argument vector. */
  private readonly kept = new Map<string, Promise<Loaded>>()

  /**
   * The kept decode of the run `argv`, now the newest, or `undefined` when
   * none is kept.
   */
  held(argv: readonly string[]): Promise<Loaded> | undefined {
    const key = keyOf(argv)
    const held = this.kept.get(key)
    if (held !== undefined) {
      this.kept.delete(key)
      this.kept.set(key, held)
    }
    return held
  }

  /**
   * Keep `loading` as the decode of the run `argv`, in place of any other.
   * The oldest decode goes when there are too many, and `loading` goes again
   * if it fails.
   */
  keep(argv: readonly string[], loading: Promise<Loaded>): void {
    const key = keyOf(argv)
    this.kept.delete(key)
    this.kept.set(key, loading)
    for (const oldest of this.kept.keys()) {
      if (this.kept.size <= KEPT_DECODES) break
      this.kept.delete(oldest)
    }
    void loading.then(loaded => {
      if (!loaded.isLoaded && this.kept.get(key) === loading) this.kept.delete(key)
    })
  }
}

/** The key of the run `argv`. No argument holds a NUL, so no two runs share one. */
function keyOf(argv: readonly string[]): string {
  return argv.join('\u0000')
}

/**
 * The picture in the JSON of `ic --emit-rgba`, or `undefined` when the text is
 * not that JSON: a field absent, a size out of the box, or pixels of a length
 * that the size does not give.
 */
function readEmitted(stdout: string): Picture | undefined {
  let document: unknown
  try {
    document = JSON.parse(stdout)
  } catch {
    return undefined
  }
  if (typeof document !== 'object' || document === null) return undefined
  if (
    !(
      'width' in document &&
      'height' in document &&
      'source_width' in document &&
      'source_height' in document &&
      'rgba' in document
    )
  ) {
    return undefined
  }

  const width = sideOf(document.width, DECODE_SIDE)
  const height = sideOf(document.height, DECODE_SIDE)
  const sourceWidth = sideOf(document.source_width, Number.MAX_SAFE_INTEGER)
  const sourceHeight = sideOf(document.source_height, Number.MAX_SAFE_INTEGER)
  const rgba = document.rgba
  if (
    width === undefined ||
    height === undefined ||
    sourceWidth === undefined ||
    sourceHeight === undefined ||
    typeof rgba !== 'string'
  ) {
    return undefined
  }

  let pixels: Uint8Array
  try {
    pixels = Uint8Array.fromBase64(rgba)
  } catch {
    return undefined
  }
  if (pixels.length !== width * height * BYTES_PER_PIXEL) return undefined

  return { width, height, sourceWidth, sourceHeight, rgba, pixels }
}

/** `value` when it is a whole number from 1 to `most`, else `undefined`. */
function sideOf(value: unknown, most: number): number | undefined {
  return typeof value === 'number' && Number.isInteger(value) && value >= 1 && value <= most
    ? value
    : undefined
}

/**
 * The lines of `text` on one line: each trimmed, the empty ones dropped, and
 * the `Error: ` that anyhow puts in front of the first one removed.
 */
function oneLine(text: string): string {
  return text
    .split('\n')
    .map(line => line.trim())
    .filter(line => line !== '')
    .join(' ')
    .replace(/^Error: /, '')
}
