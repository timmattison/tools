#!/usr/bin/env -S pnpx tsx

// Starts the app in development. Tauri starts Vite through `beforeDevCommand`,
// both on a port from portplz. The script never writes tauri.conf.json.
// It finds its paths from its own location, so the current directory is free.

import { spawn, spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { buildTauriDevArgs, parsePort, superviseChild } from "./dev-utils.ts";

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const PACKAGE_DIR = path.dirname(SCRIPT_DIR);
const REPO_ROOT = path.dirname(path.dirname(PACKAGE_DIR));
const FRONTEND_DIR = path.join(PACKAGE_DIR, "frontend");

function fail(message: string): never {
  console.error(`✖ ${message}`);
  process.exit(1);
}

const install = spawnSync("pnpm", ["install"], { cwd: FRONTEND_DIR, stdio: "inherit" });
if (install.status !== 0) fail("pnpm install failed");

const portplz = spawnSync("portplz", [], { encoding: "utf8" });
if (portplz.error || portplz.status !== 0) {
  fail("Could not run portplz. Install it with: cargo install portplz");
}

const port = parsePort(portplz.stdout);
if (port === null) fail(`portplz returned an invalid port: ${JSON.stringify(portplz.stdout)}`);

console.log(`Using port ${port}`);

// Run from the repository root: Tauri runs beforeDevCommand there.
const tauri = spawn("cargo", buildTauriDevArgs(port, process.argv.slice(2)), {
  cwd: REPO_ROOT,
  stdio: "inherit",
});
superviseChild(tauri, "cargo tauri dev", (code) => process.exit(code), fail);

// Pass a stop signal to cargo. Tauri then stops Vite, which it started itself.
for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => tauri.kill(signal));
}
