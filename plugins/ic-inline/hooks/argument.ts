// The text that a person types after /ic, or the paths that the model passes
// to show_image and show_diff, made into the paths that ic reads. Pure:
// register.tsx reads HOME through the engine when a path starts at the home
// directory.

import { DIFF_MODE_NAMES, type DiffMode, isDiffMode, type Subject } from './picture'

/** A path in matching single or double quotes. */
const QUOTED = /^(["'])(.*)\1$/s

/** The first word of the diff form of /ic. */
const DIFF_WORD = 'diff'

/** The option of the diff form that names the mode. */
const MODE_OPTION = '--mode'

/** The same option, with its mode in the same word: `--mode=mask`. */
const MODE_ASSIGNMENT = `${MODE_OPTION}=`

/** The word after which each word of the diff form is a path. */
const END_OF_OPTIONS = '--'

/** The start of an option. A path that starts with it goes after `--`. */
const OPTION_START = '--'

/** The quote that holds a path with white space in the diff form. */
const DOUBLE_QUOTE = '"'

/** One character of white space. */
const WHITE_SPACE = /\s/

/**
 * What the text after /ic asks for: a picture to show, nothing (the usage),
 * or a diff form that cannot run, with the cause.
 */
export type Asked = Subject | { kind: 'empty' } | { kind: 'refused'; reason: string }

/**
 * What the text after /ic asks for.
 *
 * When the first word is `diff`, the text is the diff form:
 * `diff [--mode MODE] FIRST SECOND`. Else the whole text is one path, as
 * `parsePath` reads it. Thus `diff.png` and `"diff"` stay one image.
 *
 * @param text what the person typed after /ic
 */
export function parseCommand(text: string): Asked {
  const rest = afterDiffWord(text)
  if (rest !== undefined) return parseDiff(rest)

  const path = parsePath(text)
  return path === undefined ? { kind: 'empty' } : { kind: 'image', path }
}

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

/** The text after the first word when that word is `diff`, else `undefined`. */
function afterDiffWord(text: string): string | undefined {
  const trimmed = text.trimStart()
  if (!trimmed.startsWith(DIFF_WORD)) return undefined
  const rest = trimmed.slice(DIFF_WORD.length)
  return rest === '' || WHITE_SPACE.test(rest.charAt(0)) ? rest : undefined
}

/**
 * The diff form after its word `diff`: `[--mode MODE] FIRST SECOND`.
 *
 * `--mode MODE` and `--mode=MODE` can occur before, between, or after the
 * paths. After the word `--`, each word is a path. Any other word that starts
 * with `--` is an option that the form does not know.
 */
function parseDiff(text: string): Asked {
  const words = wordsOf(text)
  if (words === undefined) return { kind: 'refused', reason: 'a double quote is not closed' }

  let mode: DiffMode | undefined
  let isPathsOnly = false
  const paths: string[] = []
  let word: string | undefined
  while ((word = words.shift()) !== undefined) {
    if (isPathsOnly) {
      paths.push(word)
    } else if (word === END_OF_OPTIONS) {
      isPathsOnly = true
    } else if (word === MODE_OPTION || word.startsWith(MODE_ASSIGNMENT)) {
      if (mode !== undefined) {
        return { kind: 'refused', reason: `${MODE_OPTION} occurs two times. Give it one time` }
      }
      const value = word === MODE_OPTION ? words.shift() : word.slice(MODE_ASSIGNMENT.length)
      if (value === undefined || value === '') {
        return { kind: 'refused', reason: `${MODE_OPTION} needs a mode: ${DIFF_MODE_NAMES}` }
      }
      if (!isDiffMode(value)) {
        return { kind: 'refused', reason: `${MODE_OPTION} takes ${DIFF_MODE_NAMES}, and it got ${value}` }
      }
      mode = value
    } else if (word.startsWith(OPTION_START)) {
      return {
        kind: 'refused',
        reason: `/ic diff does not know the option ${word}. Put ${END_OF_OPTIONS} before a path that starts with ${OPTION_START}`,
      }
    } else {
      paths.push(word)
    }
  }

  const [first, second] = paths
  if (paths.length !== 2 || first === undefined || second === undefined) {
    return {
      kind: 'refused',
      reason: `/ic diff compares two images, thus it takes exactly two paths, and it got ${paths.length}`,
    }
  }
  if (first === '' || second === '') return { kind: 'refused', reason: 'a path is empty' }
  return { kind: 'diff', first, second, mode }
}

/**
 * The words of `text`, split at white space, or `undefined` when a double
 * quote is not closed.
 *
 * White space between double quotes is part of the word, and the quotes are
 * not, so `"my photo".png` is the one word `my photo.png` and `""` is one
 * empty word. A single quote is a character of the word, so a name such as
 * `Tim's.png` needs no quotes.
 */
function wordsOf(text: string): string[] | undefined {
  const words: string[] = []
  let word: string | undefined
  let isQuoted = false
  for (const character of text) {
    if (character === DOUBLE_QUOTE) {
      isQuoted = !isQuoted
      word ??= ''
    } else if (!isQuoted && WHITE_SPACE.test(character)) {
      if (word !== undefined) words.push(word)
      word = undefined
    } else {
      word = (word ?? '') + character
    }
  }
  if (isQuoted) return undefined
  if (word !== undefined) words.push(word)
  return words
}
