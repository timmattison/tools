// What ic gives: the arguments of `ic --emit-rgba`, the reading of what it
// printed, and the decodes kept for the rows that draw a picture again.
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

/** How to put an ic that knows --emit-rgba on this machine. */
const UPDATE_IC = 'cargo install --git https://github.com/timmattison/tools ic'

/** What clap prints when ic does not know the flag. */
const UNKNOWN_FLAG = /unexpected argument '--emit-rgba'/

/**
 * The argument vector that decodes `path` with the ic at `icPath`.
 *
 * The path goes after `--`, so a path that starts with `-` stays a path and
 * clap does not read it as options.
 *
 * @param icPath the ic executable
 * @param path the file, absolute or relative to the session's directory
 */
export function icArguments(icPath: string, path: string): string[] {
  return [icPath, '--emit-rgba', DECODE_BOX, '--', path]
}

/**
 * The picture that a finished run of ic printed, or why there is none.
 *
 * @param run how the run ended
 * @param icPath the ic that ran, for the reason of a failure
 */
export function loadedFrom(run: IcRun, icPath: string): Loaded {
  if (run.exitCode !== 0) {
    if (UNKNOWN_FLAG.test(run.stderr)) {
      return { isLoaded: false, reason: `this ${icPath} does not know --emit-rgba. Update it: ${UPDATE_IC}` }
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
 * The decodes kept for the rows of the transcript.
 *
 * A row draws again when the width of the window changes and after a hot
 * reload, and each draw asks for the same picture. A request of the person or
 * of the model decodes again, because the file can have changed. A failure is
 * not kept: the file can appear, or the run can have been cut short.
 */
export class Decodes {
  /** The decodes, newest last, by the ic and the path that made them. */
  private readonly kept = new Map<string, Promise<Loaded>>()

  /**
   * The kept decode of `path` by `icPath`, now the newest, or `undefined`
   * when none is kept.
   */
  held(icPath: string, path: string): Promise<Loaded> | undefined {
    const key = keyOf(icPath, path)
    const held = this.kept.get(key)
    if (held !== undefined) {
      this.kept.delete(key)
      this.kept.set(key, held)
    }
    return held
  }

  /**
   * Keep `loading` as the decode of `path` by `icPath`, in place of any
   * other. The oldest decode goes when there are too many, and `loading`
   * goes again if it fails.
   */
  keep(icPath: string, path: string, loading: Promise<Loaded>): void {
    const key = keyOf(icPath, path)
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

/** The key of the decode of `path` by `icPath`. */
function keyOf(icPath: string, path: string): string {
  return `${icPath}\u0000${path}`
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

  const fields = document as Record<string, unknown>
  const width = sideOf(fields.width, DECODE_SIDE)
  const height = sideOf(fields.height, DECODE_SIDE)
  const sourceWidth = sideOf(fields.source_width, Number.MAX_SAFE_INTEGER)
  const sourceHeight = sideOf(fields.source_height, Number.MAX_SAFE_INTEGER)
  const rgba = fields.rgba
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
