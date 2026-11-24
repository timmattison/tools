/**
 * Pure logic of the NLE dev script. This module has no side effects.
 */

/** A TCP port number from 1 to 65535. */
export type Port = number & { readonly __brand: "Port" };

/** The directory of the frontend package, relative to `src/nle`. */
export const FRONTEND_DIR = "frontend";

/**
 * Read the port that `portplz` printed.
 *
 * @param output - the raw standard output of `portplz`.
 * @returns the port, or null when the output is not a number from 1 to 65535.
 */
export function parsePort(_output: string): Port | null {
  return null;
}

/**
 * Build the `pnpm` arguments that start Vite on a given port.
 *
 * @param port - the port Vite must bind.
 * @returns the arguments, without the `pnpm` command itself.
 */
export function buildViteArgs(_port: Port): string[] {
  return [];
}

/**
 * Build the `cargo` arguments that start `cargo tauri dev` with Vite on a given port.
 *
 * @param port - the port that Vite binds and that Tauri loads.
 * @returns the arguments, without the `cargo` command itself.
 */
export function buildTauriDevArgs(_port: Port): string[] {
  return [];
}
