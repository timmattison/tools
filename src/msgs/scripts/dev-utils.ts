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
