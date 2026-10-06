// The options of ic-inline, read once from the values that the manifest's
// userConfig declares. The engine fills in the defaults and drops a value
// outside a field's `options`, so this reads what remains and holds each value
// to the range that the drawing can use.

import type { PluginOptions } from 'claude-code'

/**
 * How a picture draws: `auto` asks the terminal, `blocks` draws colored half
 * blocks, `image` draws pixels (kitty and Ghostty show them, every other
 * terminal shows the caption in their place).
 */
export type Renderer = 'auto' | 'blocks' | 'image'

/** The options, typed and in range. */
export type Options = {
  renderer: Renderer
  /** The tallest that a picture can be, in terminal rows. */
  maxRows: number
  /** The ic executable to run. */
  icPath: string
}

const RENDERERS: readonly Renderer[] = ['auto', 'blocks', 'image']

const DEFAULTS: Options = { renderer: 'auto', maxRows: 24, icPath: 'ic' }

/** The tallest picture the option takes: the most rows a Raster draws. */
const MOST_ROWS = 256

/**
 * Read the options that `register` receives.
 *
 * @param raw the plugin's options, as the engine hands them over
 * @returns every option, a missing or unusable value replaced by its default
 */
export function readOptions(raw: PluginOptions): Options {
  const renderer = RENDERERS.find(known => known === raw.renderer) ?? DEFAULTS.renderer
  const rows = raw.maxRows
  const maxRows =
    typeof rows === 'number' && Number.isFinite(rows)
      ? Math.min(MOST_ROWS, Math.max(1, Math.floor(rows)))
      : DEFAULTS.maxRows
  const icPath =
    typeof raw.icPath === 'string' && raw.icPath.trim() !== '' ? raw.icPath.trim() : DEFAULTS.icPath

  return { renderer, maxRows, icPath }
}
