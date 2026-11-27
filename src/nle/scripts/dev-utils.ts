/**
 * Pure logic of the NLE dev script. This module has no side effects.
 */

/** A TCP port number from 1 to 65535. */
export type Port = number & { readonly __brand: "Port" };

/** The highest port a socket can bind. */
const MAX_PORT = 65535;

/**
 * Read the port that `portplz` printed.
 *
 * @param output - the raw standard output of `portplz`.
 * @returns the port, or null when the output is not a number from 1 to 65535.
 */
export function parsePort(output: string): Port | null {
  const trimmed = output.trim();
  if (!/^\d{1,5}$/.test(trimmed)) {
    return null;
  }
  const port = Number(trimmed);
  if (port < 1 || port > MAX_PORT) {
    return null;
  }
  return port as Port;
}

/**
 * Build the `dx` arguments that start the desktop dev server on a given port.
 *
 * dx binds its dev server to this port. portplz picks a free port, so that two
 * worktrees or two accounts do not collide.
 *
 * @param port - the port that dx binds.
 * @returns the arguments, without the `dx` command itself.
 */
export function buildDxServeArgs(port: Port): string[] {
  return ["serve", "--desktop", "--port", String(port)];
}
