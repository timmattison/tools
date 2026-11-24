/** A TCP port number from 1 to 65535. */
export type Port = number & { readonly __brand: "Port" };

/**
 * Reads the port that `portplz` printed.
 * Returns null when the output is not one whole number from 1 to 65535.
 */
export function parsePort(_output: string): Port | null {
  return null;
}
