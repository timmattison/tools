// green-check — what "green" means for a repo, and how to verify it.
//
// This module owns the whole definition of the green check: detecting which
// toolchains a worktree uses (pnpm / cargo / Tauri), assembling the command
// plan, and running it. Callers see only `isGreen(cwd)` (and `buildCheckPlan`
// for inspection/testing) — the pnpm/cargo/Tauri detection stays hidden here.
//
// Green check (always runs inside the worktree being checked, never the parent):
//   - ./.swt-check at repo root (escape hatch — used alone if present)
//   Otherwise, runs whichever apply, additively (Tauri repos have both):
//   - package.json declaring at least one of typecheck/tsc/lint/test:
//     `pnpm install --frozen-lockfile` (if pnpm-lock.yaml), then those checks.
//     A package.json with none of those scripts contributes nothing — the install
//     alone verifies nothing and must never stand in for a check.
//   - Cargo.toml at repo root and/or src-tauri/Cargo.toml: cargo check + test + clippy per manifest
//   If nothing applies: error (drop a .swt-check).

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

/** Outcome of a shell command or check: success flag plus captured output. */
export type Result = { ok: boolean; out: string };

/**
 * Runs a single check command, streaming stdout/stderr live so the user sees
 * progress on long checks.
 *
 * @param cmd - Shell command to run.
 * @param cwd - Directory to run it in.
 * @returns True if the command exited 0.
 */
const streamCheck = (cmd: string, cwd: string): boolean => {
  process.stderr.write(`\n  $ ${cmd}\n`);
  const r = spawnSync("sh", ["-c", cmd], { cwd, stdio: "inherit" });
  return r.status === 0;
};

/**
 * Reads the script names declared in a directory's package.json.
 *
 * @param cwd - Directory that may contain a package.json.
 * @returns The set of script names; empty if there is no package.json or it is unparseable.
 */
export function pkgScripts(cwd: string): Set<string> {
  const p = join(cwd, "package.json");
  if (!existsSync(p)) return new Set();
  try {
    const json = JSON.parse(readFileSync(p, "utf8"));
    return new Set(Object.keys(json.scripts ?? {}));
  } catch {
    return new Set();
  }
}

/**
 * Determines the ordered list of shell commands that constitute the green check
 * for a worktree, based on the files present at its root.
 *
 * @param target - Worktree root to inspect and to run the check in.
 * @param configRoot - Directory the `.swt-check` override is looked up in;
 *   defaults to `target`.
 * @returns The commands to run in order, or null if no check applies.
 */
export function buildCheckPlan(target: string, configRoot: string = target): string[] | null {
  const cwd = target;
  if (existsSync(join(cwd, ".swt-check"))) return ["./.swt-check"];

  const cmds: string[] = [];

  if (existsSync(join(cwd, "package.json"))) {
    const scripts = pkgScripts(cwd);
    const jsChecks: string[] = [];
    if (scripts.has("typecheck")) jsChecks.push("pnpm typecheck");
    else if (scripts.has("tsc")) jsChecks.push("pnpm exec tsc --noEmit");
    if (scripts.has("lint")) jsChecks.push("pnpm lint");
    if (scripts.has("test")) jsChecks.push("pnpm test --run");

    // The install verifies nothing on its own — it exists only so the js checks
    // can run in a fresh worktree, which has no node_modules. A plan of just an
    // install would report green having checked nothing, so it rides along with
    // the js checks or not at all.
    if (jsChecks.length > 0) {
      if (existsSync(join(cwd, "pnpm-lock.yaml"))) {
        cmds.push("pnpm install --frozen-lockfile");
      }
      cmds.push(...jsChecks);
    }
  }

  // Rust checks run alongside package.json checks — Tauri repos have both.
  // "" = root Cargo.toml (no --manifest-path needed); otherwise point at the manifest.
  const cargoManifests: string[] = [];
  if (existsSync(join(cwd, "Cargo.toml"))) cargoManifests.push("");
  if (existsSync(join(cwd, "src-tauri", "Cargo.toml"))) cargoManifests.push("src-tauri/Cargo.toml");
  for (const manifest of cargoManifests) {
    const flag = manifest ? ` --manifest-path ${manifest}` : "";
    cmds.push(`cargo check${flag}`, `cargo test${flag}`, `cargo clippy${flag} -- -D warnings`);
  }

  return cmds.length > 0 ? cmds : null;
}

/**
 * Runs the green check for a worktree, stopping at the first failing command.
 *
 * @param target - Worktree root to check; every command runs in this directory.
 * @param configRoot - Directory the `.swt-check` override is looked up in;
 *   defaults to `target`.
 * @returns Ok result when every command passed, otherwise the first failure.
 */
export function isGreen(target: string, configRoot: string = target): Result {
  const cwd = target;
  const plan = buildCheckPlan(cwd);
  if (!plan) {
    return {
      ok: false,
      out: `No green-check defined. Drop a './.swt-check' executable at the repo root.\n`,
    };
  }
  process.stderr.write(`Running green check in ${cwd}…`);
  for (const cmd of plan) {
    if (!streamCheck(cmd, cwd)) return { ok: false, out: `failed: ${cmd}\n` };
  }
  return { ok: true, out: "" };
}
