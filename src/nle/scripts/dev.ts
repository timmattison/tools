#!/usr/bin/env -S pnpx tsx
/**
 * Run the NLE (Non-Linear Editor) Tauri application in development mode.
 *
 * Usage:
 *   ./src/nle/scripts/dev.ts
 *
 * The script checks that cargo, the Tauri CLI and portplz exist, asks portplz
 * for a free port, checks that the checkout is writable, installs the frontend
 * dependencies, and then starts `cargo tauri dev` with Vite on that port.
 * The checks and the port come before the install on purpose: each is fast, and
 * a missing tool must stop the script before the slow work.
 *
 * The Rust toolchain, the Tauri CLI, portplz and pnpm are per-account installs,
 * and the checkout is writable only by the account that owns it. Each message
 * names the account for that reason.
 */
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  FRONTEND_DIR,
  buildTauriDevArgs,
  parsePort,
  type Port,
} from "./dev-utils.ts";

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

const cargoVersion = spawnSync("cargo", ["tauri", "--version"], {
  stdio: "ignore",
});
if (isMissing(cargoVersion.error)) {
  fail([
    `cargo is not on the PATH of the account '${user}'.`,
    "  Rust installs per account under ~/.cargo, and one account cannot",
    "  read another's. Run this from the account that owns the checkout,",
    `  or install Rust for '${user}' from https://rustup.rs`,
  ]);
}
if (cargoVersion.status !== 0) {
  fail([
    `the Tauri CLI is not installed for the account '${user}'.`,
    "  Install it with:",
    "    cargo install tauri-cli",
    "  Or run this from the account that already has it.",
  ]);
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

try {
  fs.accessSync(nleDir, fs.constants.W_OK);
} catch {
  fail([
    `the account '${user}' cannot write to ${nleDir}.`,
    "  pnpm writes frontend/node_modules/, and the Tauri build writes gen/.",
    "  Run this from the account that owns the checkout.",
  ]);
}

const frontendDir = path.join(nleDir, FRONTEND_DIR);
const install = spawnSync("pnpm", ["install"], {
  cwd: frontendDir,
  stdio: "inherit",
});
if (isMissing(install.error)) {
  fail([
    `pnpm is not on the PATH of the account '${user}'.`,
    "  pnpm installs the frontend dependencies. Install it from",
    "  https://pnpm.io/installation, or run this from the account that has it.",
  ]);
}
if (install.status !== 0) {
  fail([`pnpm install failed in ${frontendDir}.`], install.status ?? 1);
}

console.log(
  `Starting the dev server on port ${String(port)} (chosen by portplz)`,
);
const tauri = spawn("cargo", buildTauriDevArgs(port), {
  cwd: nleDir,
  stdio: "inherit",
});
tauri.on("error", (error) => {
  fail([`cannot start cargo tauri dev: ${error.message}`]);
});
tauri.on("exit", (code) => {
  process.exit(code ?? 1);
});
