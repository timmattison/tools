#!/usr/bin/env -S pnpx tsx

import { spawn, spawnSync, type ChildProcess } from "child_process";
import { readFileSync, writeFileSync } from "fs";
import { join } from "path";
import { fileURLToPath } from "url";
import { dirname } from "path";
import { parsePort, type Port } from "./dev-utils.ts";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const PACKAGE_DIR = join(SCRIPT_DIR, "..");
const FRONTEND_DIR = join(PACKAGE_DIR, "frontend");
const TAURI_CONF_PATH = join(PACKAGE_DIR, "tauri.conf.json");

function fail(message: string): never {
  console.error(`✖ ${message}`);
  process.exit(1);
}

function getPort(): Port {
  const result = spawnSync("portplz", [], { encoding: "utf8" });
  if (result.error || result.status !== 0) {
    fail("Could not run portplz. Install it with: cargo install portplz");
  }
  const port = parsePort(result.stdout);
  if (port === null) {
    fail(`portplz returned an invalid port: ${JSON.stringify(result.stdout)}`);
  }
  return port;
}

function updateTauriConfig(port: Port): void {
  const config = JSON.parse(readFileSync(TAURI_CONF_PATH, "utf8"));
  config.build.devUrl = `http://localhost:${port}`;
  writeFileSync(TAURI_CONF_PATH, JSON.stringify(config, null, 2) + "\n");
}

function installDeps(): void {
  console.log("Installing frontend dependencies...");
  const install = spawnSync("pnpm", ["install"], { cwd: FRONTEND_DIR, stdio: "inherit" });
  if (install.status !== 0) fail("pnpm install failed");
}

function startVite(port: Port): ChildProcess {
  console.log(`Starting Vite dev server on port ${port}...`);
  return spawn("pnpm", ["dev"], {
    cwd: FRONTEND_DIR,
    stdio: "inherit",
    env: {
      ...process.env,
      VITE_PORT: String(port),
      VITE_HMR_PORT: String(port + 1),
    },
  });
}

function startTauri(): ChildProcess {
  const extraArgs = process.argv.slice(2);
  const args = ["tauri", "dev"];
  if (extraArgs.length > 0) {
    args.push("--", ...extraArgs);
    console.log(`Starting Tauri with extra args: ${extraArgs.join(" ")}`);
  } else {
    console.log("Starting Tauri...");
  }

  // Run from the package directory so cargo tauri finds tauri.conf.json
  return spawn("cargo", args, {
    cwd: PACKAGE_DIR,
    stdio: "inherit",
  });
}

function cleanup(vite: ChildProcess, tauri: ChildProcess): void {
  console.log("\nShutting down...");
  tauri.kill("SIGTERM");
  vite.kill("SIGTERM");
}

function main(): void {
  installDeps();
  const port = getPort();
  console.log(`Using port ${port} (HMR: ${port + 1})`);

  updateTauriConfig(port);

  const vite = startVite(port);
  // Give vite a moment to start before launching tauri
  const tauri = startTauri();

  const onExit = () => cleanup(vite, tauri);
  process.on("SIGINT", onExit);
  process.on("SIGTERM", onExit);

  tauri.on("close", (code) => {
    vite.kill("SIGTERM");
    process.exit(code ?? 0);
  });

  vite.on("close", (code) => {
    if (code !== null && code !== 0) {
      console.error(`Vite exited with code ${code}`);
      tauri.kill("SIGTERM");
      process.exit(code);
    }
  });
}

main();
