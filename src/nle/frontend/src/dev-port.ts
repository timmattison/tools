/**
 * The port the Vite dev server listens on.
 *
 * This module runs in Node, inside the Vite config. It is never bundled into the
 * application.
 */

/** The port to use when nothing else names one. */
const DEFAULT_DEV_PORT = 1420;

/**
 * Give the port the Vite dev server must bind to.
 *
 * @returns the port number.
 */
export function devServerPort(): number {
  return DEFAULT_DEV_PORT;
}
