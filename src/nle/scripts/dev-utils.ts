/**
 * Pure logic of the NLE dev script. This module has no side effects.
 */

/** A TCP port number from 1 to 65535. */
export type Port = number & { readonly __brand: "Port" };

/** The directory of the frontend package, relative to `src/nle`. */
export const FRONTEND_DIR = "frontend";

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
 * Build the `pnpm` arguments that start Vite on a given port.
 *
 * `--strictPort` makes Vite fail instead of drifting to another port, which
 * Tauri would not know about.
 *
 * @param port - the port Vite must bind.
 * @returns the arguments, without the `pnpm` command itself.
 */
export function buildViteArgs(port: Port): string[] {
  return ["exec", "vite", "--port", String(port), "--strictPort"];
}

/**
 * Build the `cargo` arguments that start `cargo tauri dev` with Vite on a given port.
 *
 * The `--config` value is a JSON merge patch over `tauri.conf.json`. It sets
 * the URL that the window loads and the command that starts Vite, so the two
 * always name the same port.
 *
 * @param port - the port that Vite binds and that Tauri loads.
 * @returns the arguments, without the `cargo` command itself.
 */
export function buildTauriDevArgs(port: Port): string[] {
  const config = {
    build: {
      devUrl: `http://localhost:${String(port)}`,
      beforeDevCommand: {
        script: ["pnpm", ...buildViteArgs(port)].join(" "),
        cwd: FRONTEND_DIR,
      },
    },
  };
  return ["tauri", "dev", "--config", JSON.stringify(config)];
}
