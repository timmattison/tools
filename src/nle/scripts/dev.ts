#!/usr/bin/env -S pnpx tsx
/**
 * Run the NLE (Non-Linear Editor) Dioxus desktop application in development mode.
 *
 * Usage:
 *   ./src/nle/scripts/dev.ts
 *
 * The script checks that dx (the Dioxus CLI) and portplz exist, asks portplz
 * for a free port, and then starts `dx serve --desktop` on that port.
 * The checks and the port come first on purpose: each is fast, and a missing
 * tool must stop the script before the slow build.
 *
 * dx and portplz are per-account installs, and one account cannot read
 * another's. Each message names the account for that reason.
 */
import { spawn, spawnSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { buildDxServeArgs, parsePort, type Port } from "./dev-utils.ts";

/** The first words of each message. */
const PREFIX = "nle dev: ";

/** The `src/nle` directory, found from the location of this file. */
const nleDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

/** The name of the account that runs the script. */
const user = os.userInfo().username;

/**
 * Print lines to standard error and exit.
 *
 * @param lines - the message, one entry for each line. The first line gets the prefix.
 * @param code - the exit status.
 */
function fail(lines: string[], code = 1): never {
  const [first = "", ...rest] = lines;
  console.error([`${PREFIX}${first}`, ...rest].join("\n"));
  process.exit(code);
}

/**
 * Tell whether a failed spawn found no such command.
 *
 * @param error - the `error` field of a `spawnSync` result.
 */
function isMissing(error: Error | undefined): boolean {
  return (error as { code?: unknown } | undefined)?.code === "ENOENT";
}

const dxVersion = spawnSync("dx", ["--version"], { stdio: "ignore" });
if (isMissing(dxVersion.error)) {
  fail([
    `dx is not on the PATH of the account '${user}'.`,
    "  dx is the Dioxus CLI. It installs per account under ~/.cargo/bin, and one",
    "  account cannot read another's. Install it with:",
    "    cargo install dioxus-cli",
    "  Or run this from the account that already has it.",
  ]);
}
if (dxVersion.status !== 0) {
  fail([`dx --version failed with exit status ${String(dxVersion.status)}.`]);
}

const portplz = spawnSync("portplz", [], {
  encoding: "utf8",
  stdio: ["ignore", "pipe", "inherit"],
});
if (isMissing(portplz.error)) {
  fail([
    `portplz is not on the PATH of the account '${user}'.`,
    "  It picks the dev server port, so two branches, two worktrees or",
    "  two accounts never collide. Install it from this repository:",
    `    cargo install --path ${path.resolve(nleDir, "..", "portplz")}`,
  ]);
}
if (portplz.status !== 0) {
  fail([`portplz failed with exit status ${String(portplz.status)}.`]);
}
const port: Port | null = parsePort(portplz.stdout);
if (port === null) {
  fail([
    `portplz gave '${portplz.stdout.trim()}', which is not a port number.`,
  ]);
}

console.log(`Starting dx serve on port ${String(port)} (chosen by portplz)`);
const dx = spawn("dx", buildDxServeArgs(port), {
  cwd: nleDir,
  stdio: "inherit",
});
dx.on("error", (error) => {
  fail([`cannot start dx serve: ${error.message}`]);
});
dx.on("exit", (code) => {
  process.exit(code ?? 1);
});
