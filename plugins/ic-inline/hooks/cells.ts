// The arithmetic of a picture on a grid of terminal cells: how many cells it
// takes, and the half-block cells of a Raster that draws it.
//
// Nothing here calls the engine, so every function is pure.

/** A size in terminal cells. */
export type CellSize = { columns: number; rows: number }

/** A size in pixels. */
export type PixelSize = { width: number; height: number }

/**
 * The pixels that one cell shows of a half-block Raster: one across, and two
 * down, because each cell draws `▀` with a color above and a color below.
 *
 * A terminal cell is about twice as tall as it is wide, so these two pixels
 * are about square on the screen and the picture keeps its shape.
 */
export const HALF_BLOCK_CELL: PixelSize = { width: 1, height: 2 }

/**
 * The pixels that one cell takes of an Image, as most terminals draw a cell.
 *
 * The engine scales an Image to fill its box of cells, so this size decides
 * only how large a small picture shows: at about its own size, not stretched
 * to the width of the transcript.
 */
export const TERMINAL_CELL: PixelSize = { width: 8, height: 16 }

/** The glyph of a cell whose upper half holds a color. */
const UPPER_HALF = 0x2580

/** The glyph of a cell whose lower half alone holds a color. */
const LOWER_HALF = 0x2584

/** The glyph of a cell that holds no color. */
const SPACE = 0x20

/** The color value that a Raster reads as the terminal's own color. */
const TERMINAL_DEFAULT = 0x01000000

/** The bytes of one RGBA pixel. */
const BYTES_PER_PIXEL = 4

/** The bytes of one Raster cell: three little-endian u32 values. */
const BYTES_PER_CELL = 12

/** The alpha below which a pixel draws as no color. */
const OPAQUE_FROM = 128

/**
 * The cells that a picture of `picture` pixels takes inside `room`, when one
 * cell shows `cell` pixels.
 *
 * A picture that fits keeps its own size, because a larger copy holds no more
 * detail. A picture that does not fit takes the whole of the side that binds
 * it, and the other side keeps the shape of the picture, rounded to the
 * nearest cell. Each side keeps one cell at least.
 *
 * @param picture the size of the picture, each side 1 or more
 * @param room the most cells the picture can take, each side 1 or more
 * @param cell the pixels that one cell shows
 * @returns the cells of the picture, inside `room`
 */
export function fitCells(picture: PixelSize, room: CellSize, cell: PixelSize): CellSize {
  const native = {
    columns: Math.max(1, Math.ceil(picture.width / cell.width)),
    rows: Math.max(1, Math.ceil(picture.height / cell.height)),
  }
  if (native.columns <= room.columns && native.rows <= room.rows) {
    return native
  }

  // The picture is wider than the room, in shape, when its width in cells
  // over its height in cells is at least the columns of the room over its
  // rows. The width then binds. Both sides of the comparison are multiplied
  // out, so no division rounds it.
  const isWidthBound =
    picture.width * cell.height * room.rows >= picture.height * cell.width * room.columns
  if (isWidthBound) {
    const rows = Math.round((room.columns * picture.height * cell.width) / (picture.width * cell.height))
    return { columns: room.columns, rows: clamp(rows, 1, room.rows) }
  }
  const columns = Math.round((room.rows * picture.width * cell.height) / (picture.height * cell.width))
  return { columns: clamp(columns, 1, room.columns), rows: room.rows }
}

/**
 * The `cells` of a Raster that draws `pixels` over `size`, two pixels to a
 * cell: standard base64 of `[codePoint, foreground, background]` u32 triplets,
 * little-endian, row by row.
 *
 * Each cell is `▀`, its foreground the pixel above and its background the
 * pixel below. A transparent pixel takes the terminal's own color, so a
 * picture with a transparent background sits on the terminal's background: a
 * transparent upper half draws `▄` with the lower color, and a cell with no
 * color draws a space.
 *
 * @param pixels the picture, 4 bytes for each pixel (RGBA), row by row
 * @param picture the size of the picture in pixels
 * @param size the cells to draw; the picture is sampled to `columns` by
 *   `rows * 2` pixels
 * @returns the cells, ready for `RasterProps.cells`
 */
export function halfBlockCells(pixels: Uint8Array, picture: PixelSize, size: CellSize): string {
  const sampled = sample(pixels, picture, { width: size.columns, height: size.rows * HALF_BLOCK_CELL.height })
  const cells = new DataView(new ArrayBuffer(size.columns * size.rows * BYTES_PER_CELL))

  for (let row = 0; row < size.rows; row += 1) {
    for (let column = 0; column < size.columns; column += 1) {
      const upper = colorAt(sampled, size.columns, column, row * 2)
      const lower = colorAt(sampled, size.columns, column, row * 2 + 1)
      const [glyph, foreground, background] =
        upper === undefined
          ? lower === undefined
            ? [SPACE, TERMINAL_DEFAULT, TERMINAL_DEFAULT]
            : [LOWER_HALF, lower, TERMINAL_DEFAULT]
          : [UPPER_HALF, upper, lower ?? TERMINAL_DEFAULT]
      const at = (row * size.columns + column) * BYTES_PER_CELL
      cells.setUint32(at, glyph, true)
      cells.setUint32(at + 4, foreground, true)
      cells.setUint32(at + 8, background, true)
    }
  }

  return new Uint8Array(cells.buffer).toBase64()
}

/**
 * The color of the pixel at `x`, `y` of a sampled picture `width` pixels
 * wide, as `0x00RRGGBB`, or `undefined` for a pixel that draws as no color.
 */
function colorAt(sampled: Uint8Array, width: number, x: number, y: number): number | undefined {
  const at = (y * width + x) * BYTES_PER_PIXEL
  if ((sampled[at + 3] ?? 0) < OPAQUE_FROM) {
    return undefined
  }
  return ((sampled[at] ?? 0) << 16) | ((sampled[at + 1] ?? 0) << 8) | (sampled[at + 2] ?? 0)
}

/**
 * `pixels` resampled to `target`: each target pixel is the average of the
 * source pixels it covers, at least one, weighted by their alpha so that the
 * color of a transparent pixel never bleeds into its neighbors.
 *
 * @param pixels the source, 4 bytes for each pixel (RGBA), row by row
 * @param source the size of the source in pixels
 * @param target the size to sample to, in pixels
 * @returns the target pixels, 4 bytes each (RGBA), row by row
 */
function sample(pixels: Uint8Array, source: PixelSize, target: PixelSize): Uint8Array {
  const sampled = new Uint8Array(target.width * target.height * BYTES_PER_PIXEL)

  for (let ty = 0; ty < target.height; ty += 1) {
    const top = Math.floor((ty * source.height) / target.height)
    const bottom = Math.max(top + 1, Math.floor(((ty + 1) * source.height) / target.height))
    for (let tx = 0; tx < target.width; tx += 1) {
      const left = Math.floor((tx * source.width) / target.width)
      const right = Math.max(left + 1, Math.floor(((tx + 1) * source.width) / target.width))

      let red = 0
      let green = 0
      let blue = 0
      let alpha = 0
      for (let y = top; y < bottom; y += 1) {
        for (let x = left; x < right; x += 1) {
          const at = (y * source.width + x) * BYTES_PER_PIXEL
          const weight = pixels[at + 3] ?? 0
          red += (pixels[at] ?? 0) * weight
          green += (pixels[at + 1] ?? 0) * weight
          blue += (pixels[at + 2] ?? 0) * weight
          alpha += weight
        }
      }

      const at = (ty * target.width + tx) * BYTES_PER_PIXEL
      const count = (bottom - top) * (right - left)
      if (alpha > 0) {
        sampled[at] = Math.round(red / alpha)
        sampled[at + 1] = Math.round(green / alpha)
        sampled[at + 2] = Math.round(blue / alpha)
        sampled[at + 3] = Math.round(alpha / count)
      }
    }
  }

  return sampled
}

/** `value` held inside `low` and `high`. */
function clamp(value: number, low: number, high: number): number {
  return Math.min(high, Math.max(low, value))
}
