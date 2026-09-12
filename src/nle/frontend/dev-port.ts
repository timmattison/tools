/**
 * The port the Vite dev server listens on.
 *
 * This module runs in Node, inside the Vite config. It is never bundled into the
 * application.
 */

/** The port to use when nothing else names one. */
const DEFAULT_DEV_PORT = 1420;

/** The variable `run-nle.sh` sets from `portplz`. */
const PORT_VARIABLE = "NLE_DEV_PORT";

/** The highest port a socket can bind. */
const MAX_PORT = 65535;

/**
 * Give the port the Vite dev server must bind to.
 *
 * `run-nle.sh` sets {@link PORT_VARIABLE} from `portplz`, which derives a port
 * from the repository, the branch and the user. Tauri gets the same number
 * through its `--config` override, so the two always agree.
 *
 * @returns the port from the environment, or 1420 when the variable is absent.
 * @throws Error when the variable holds something that is not a port number.
 */
export function devServerPort(): number {
  const raw = process.env[PORT_VARIABLE];
  if (raw === undefined) {
    return DEFAULT_DEV_PORT;
  }

  const port = Number(raw);
  if (
    raw.trim() === "" ||
    !Number.isInteger(port) ||
    port < 1 ||
    port > MAX_PORT
  ) {
    throw new Error(
      `${PORT_VARIABLE} must hold a port number from 1 to ${String(MAX_PORT)}, but it holds "${raw}".`,
    );
  }
  return port;
}
