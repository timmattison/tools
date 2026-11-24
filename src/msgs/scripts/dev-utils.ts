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
export function buildTauriDevArgs(port: Port, extraArgs: readonly string[]): string[] {
  const config = {
    build: {
      devUrl: `http://localhost:${port}`,
      beforeDevCommand: `cd src/msgs/frontend && pnpm exec vite --port ${port} --strictPort`,
    },
  };
  const args = ["tauri", "dev", "--config", JSON.stringify(config)];
  if (extraArgs.length > 0) args.push("--", ...extraArgs);
  return args;
}
