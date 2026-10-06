// A path as a person types it after /ic, or as the model passes it to
// show_image, made into the path that ic reads. Pure: register.tsx reads HOME
// through the engine when a path starts at the home directory.

/** A path in matching single or double quotes. */
const QUOTED = /^(["'])(.*)\1$/s

/**
 * The path in `text`, or `undefined` when `text` names none.
 *
 * The whole text is one path, so a path with spaces needs no quotes. Space
 * around it is dropped, and matching quotes around it are dropped.
 *
 * @param text what the person or the model gave
 * @returns the path, or `undefined` for text that is empty
 */
export function parsePath(text: string): string | undefined {
  const trimmed = text.trim()
  const path = (QUOTED.exec(trimmed)?.[2] ?? trimmed).trim()
  return path === '' ? undefined : path
}

/** Whether `path` starts at the home directory: `~` or `~/...`. */
export function startsAtHome(path: string): boolean {
  return path === '~' || path.startsWith('~/')
}

/**
 * `path` with its leading `~` made into `home`. ic gets no shell to expand it.
 *
 * @param path a path for which `startsAtHome` is true
 * @param home the home directory, or `undefined` when HOME is not set
 * @returns the path from the home directory, or `path` itself with no home
 */
export function fromHome(path: string, home: string | undefined): string {
  return home === undefined || home === '' ? path : home + path.slice(1)
}

/** The last part of `path`: the name that the caption shows. */
export function fileName(path: string): string {
  return path.split('/').filter(part => part !== '').pop() ?? path
}
