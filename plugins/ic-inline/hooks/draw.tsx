// The tree that shows one picture in a row of the transcript: the picture,
// as an Image or as half blocks, and a dim caption under it. Pure: the
// elements come from `$.ui.resolve(e)` in register.tsx.

import type { Elements, RenderElement, RenderViewport } from 'claude-code'

import { fileName } from './argument'
import { fitCells, HALF_BLOCK_CELL, halfBlockCells, TERMINAL_CELL } from './cells'
import type { Picture, Subject } from './picture'

/** The elements of the terminal that a picture draws with. */
export type PictureElements = Pick<Elements['terminal'], 'Box' | 'Text' | 'Raster' | 'Image'>

/** How a picture draws: pixels in an Image, or colored half blocks. */
export type Drawing = 'pixels' | 'blocks'

/** The key of the picture in its row. */
const PICTURE_KEY = 'ic-inline-picture'

/**
 * The columns that the picture leaves free of the transcript's width, for the
 * margin that the engine draws in front of a row.
 */
const MARGIN_COLUMNS = 4

/** The width of a transcript that has not measured itself. */
const UNMEASURED_COLUMNS = 80

/** The widest and tallest Raster that the engine draws. */
const RASTER_LIMIT = { columns: 512, rows: 256 }

/** The widest and tallest Image that the engine draws. */
const IMAGE_LIMIT = { columns: 255, rows: 255 }

/**
 * The tree that shows `picture`, which ic decoded from `subject`.
 *
 * @param elements the terminal's elements, from `$.ui.resolve(e)`
 * @param viewport what the transcript measured, from `e.viewport`
 * @param picture the picture that ic decoded
 * @param subject the file, or the two files of a diff, for the caption
 * @param drawing pixels or half blocks
 * @param maxRows the tallest that the picture can be, in rows
 * @returns the picture over its caption
 */
export function pictureTree(
  elements: PictureElements,
  viewport: RenderViewport | undefined,
  picture: Picture,
  subject: Subject,
  drawing: Drawing,
  maxRows: number,
): RenderElement {
  const { Box, Text, Raster, Image } = elements
  const columns = Math.max(1, (viewport?.columns ?? UNMEASURED_COLUMNS) - MARGIN_COLUMNS)
  const caption = `${titleOf(subject)} · ${picture.sourceWidth}×${picture.sourceHeight}`

  if (drawing === 'pixels') {
    const size = fitCells(
      picture,
      { columns: Math.min(columns, IMAGE_LIMIT.columns), rows: Math.min(maxRows, IMAGE_LIMIT.rows) },
      TERMINAL_CELL,
    )
    return (
      <Box flexDirection="column">
        <Image
          key={PICTURE_KEY}
          source={{ rgba: picture.rgba, width: picture.width, height: picture.height }}
          columns={size.columns}
          rows={size.rows}
          alt={caption}
        />
        <Text dimColor>{caption}</Text>
      </Box>
    )
  }

  const size = fitCells(
    picture,
    { columns: Math.min(columns, RASTER_LIMIT.columns), rows: Math.min(maxRows, RASTER_LIMIT.rows) },
    HALF_BLOCK_CELL,
  )
  return (
    <Box flexDirection="column">
      <Raster
        key={PICTURE_KEY}
        columns={size.columns}
        rows={size.rows}
        cells={halfBlockCells(picture.pixels, picture, size)}
      />
      <Text dimColor>{caption}</Text>
    </Box>
  )
}

/**
 * The names in the caption: the file name, or `first vs second` for a diff,
 * joined as ic joins them. Two equal file names show as the two paths.
 */
function titleOf(subject: Subject): string {
  if (subject.kind === 'image') return fileName(subject.path)
  const [first, second] = [fileName(subject.first), fileName(subject.second)]
  return first === second ? `${subject.first} vs ${subject.second}` : `${first} vs ${second}`
}
