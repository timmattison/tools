/**
 * Whether this terminal draws an `Image` element as pixels.
 *
 * `unknown` until the probe answers. The engine draws an `Image` as pixels in
 * kitty and Ghostty, and as its alt text elsewhere (zellij, tmux, iTerm2, mosh),
 * and it says which through `$.ui.blit`. A picture draws as half blocks until
 * the answer is `yes`.
 */
export type IcInlinePixels = 'unknown' | 'yes' | 'no'

/**
 * The picture that one `show_image` call showed, kept by its `tool_use_id`, so
 * the result row of that call can draw it again after a hot reload.
 */
export type IcInlineShown = {
  /** The path as `ic` reads it: the model's path with a leading `~` expanded. */
  path: string
}

/**
 * The two images that one `show_diff` call compared, kept by its
 * `tool_use_id`, so the result row of that call can draw the picture of their
 * differences again after a hot reload.
 */
export type IcInlineDiffed = {
  /** The first image as `ic` reads it: the model's path with a leading `~` expanded. */
  first: string
  /** The second image, read the same way. */
  second: string
  /** The `--diff-mode` of the call, or absent for the default mode of `ic`. */
  mode?: 'boxes' | 'highlight' | 'mask'
}

declare module 'claude-code' {
  interface PluginState {
    'ic-inline': {
      pixels: IcInlinePixels
      shown: StateFamily<IcInlineShown>
      diffed: StateFamily<IcInlineDiffed>
    }
  }
}
