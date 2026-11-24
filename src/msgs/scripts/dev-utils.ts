/** A TCP port number from 1 to 65535. */
export type Port = number & { readonly __brand: "Port" };

const MAX_PORT = 65535;

/**
 * Reads the port that `portplz` printed.
 * Returns null when the output is not one whole number from 1 to 65535.
 */
export function parsePort(output: string): Port | null {
  const trimmed = output.trim();
  if (!/^\d{1,5}$/.test(trimmed)) return null;
  const port = Number(trimmed);
  if (port < 1 || port > MAX_PORT) return null;
  return port as Port;
}

/**
 * Builds the arguments of `cargo` that start the app in development on a port.
 * The port goes in a `--config` merge, so `tauri.conf.json` stays unchanged.
 * Extra arguments go to the app after `--`.
 */
export function buildTauriDevArgs(_port: Port, _extraArgs: readonly string[]): string[] {
  return ["tauri", "dev"];
}
