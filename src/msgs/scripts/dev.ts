#!/usr/bin/env npx tsx

import { execSync, spawn, type ChildProcess } from "child_process";
import { readFileSync, writeFileSync } from "fs";
import { join } from "path";
import { fileURLToPath } from "url";
import { dirname } from "path";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const PACKAGE_DIR = join(SCRIPT_DIR, "..");
const WORKSPACE_ROOT = join(PACKAGE_DIR, "..", "..");
const FRONTEND_DIR = join(PACKAGE_DIR, "frontend");
const TAURI_CONF_PATH = join(PACKAGE_DIR, "tauri.conf.json");

function getPort(): number {
  try {
    const result = execSync("portplz", { encoding: "utf8" }).trim();
    const port = parseInt(result, 10);
    if (isNaN(port) || port < 1024 || port > 65535) {
      throw new Error(`Invalid port from portplz: ${result}`);
    }
    return port;
  } catch {
    console.error("portplz not found or failed, using default port 1420");
    return 1420;
  }
}

function updateTauriConfig(port: number): void {
  const config = JSON.parse(readFileSync(TAURI_CONF_PATH, "utf8"));
  config.build.devUrl = `http://localhost:${port}`;
  writeFileSync(TAURI_CONF_PATH, JSON.stringify(config, null, 2) + "\n");
}

function installDeps(): void {
  console.log("Installing frontend dependencies...");
  execSync("pnpm install", { cwd: FRONTEND_DIR, stdio: "inherit" });
}

function startVite(port: number): ChildProcess {
  console.log(`Starting Vite dev server on port ${port}...`);
  return spawn("pnpm", ["dev"], {
    cwd: FRONTEND_DIR,
    stdio: "inherit",
    shell: true,
    env: {
      ...process.env,
      VITE_PORT: String(port),
      VITE_HMR_PORT: String(port + 1),
    },
  });
}

function startTauri(): ChildProcess {
  const extraArgs = process.argv.slice(2);
  const args = ["tauri", "dev", "-p", "msgs"];
  if (extraArgs.length > 0) {
    args.push("--", ...extraArgs);
    console.log(`Starting Tauri with extra args: ${extraArgs.join(" ")}`);
  } else {
    console.log("Starting Tauri...");
  }

  return spawn("cargo", args, {
    cwd: WORKSPACE_ROOT,
    stdio: "inherit",
    shell: true,
  });
}

function cleanup(vite: ChildProcess, tauri: ChildProcess): void {
  console.log("\nShutting down...");
  tauri.kill("SIGTERM");
  vite.kill("SIGTERM");
}

function main(): void {
  const port = getPort();
  console.log(`Using port ${port} (HMR: ${port + 1})`);

  installDeps();
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
