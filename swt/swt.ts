#!/usr/bin/env -S npx tsx
// swt — subagent worktree helper for parallel TDD.
//
//   swt create <name>          → verify HEAD green, create worktree on a new branch, print path
//   swt merge <worktree-path>  → verify subagent green, ff-merge (rebase if parent advanced), cleanup
//
// Invariants enforced:
//   1. Worktrees are only created from a green commit (parent HEAD green at create time).
//   2. Merges are only accepted from a green subagent commit AND a green parent HEAD —
//      so a `swt merge` mid-TDD-cycle in the parent is refused, not silently fast-forwarded past.
//   3. If parent advanced during the subagent's work, rebase + re-verify green before ff-merging.
//   4. Concurrent `swt merge` runs against the same parent are serialized via .git/swt.lock.
//
// Green check:
//   - ./.swt-check at repo root (escape hatch — used alone if present)
//   Otherwise, runs whichever apply, additively (Tauri repos have both):
//   - package.json scripts: typecheck/lint/test (only the ones that exist)
//   - Cargo.toml at repo root and/or src-tauri/Cargo.toml: cargo check + test + clippy per manifest
//   If nothing applies: error (drop a .swt-check).

import { spawnSync } from "node:child_process";
import { closeSync, existsSync, openSync, readFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";

type Result = { ok: boolean; out: string };

const sh = (cmd: string, cwd?: string): Result => {
  const r = spawnSync("sh", ["-c", cmd], { cwd, encoding: "utf8" });
  return { ok: r.status === 0, out: (r.stdout ?? "") + (r.stderr ?? "") };
};

const must = (cmd: string, cwd?: string): string => {
  const r = sh(cmd, cwd);
  if (!r.ok) {
    process.stderr.write(r.out);
    process.exit(1);
  }
  return r.out.trim();
};

// Stream stdout/stderr live so the user sees progress on long checks.
const streamCheck = (cmd: string, cwd: string): boolean => {
  process.stderr.write(`\n  $ ${cmd}\n`);
  const r = spawnSync("sh", ["-c", cmd], { cwd, stdio: "inherit" });
  return r.status === 0;
};

function pkgScripts(cwd: string): Set<string> {
  const p = join(cwd, "package.json");
  if (!existsSync(p)) return new Set();
  try {
    const json = JSON.parse(readFileSync(p, "utf8"));
    return new Set(Object.keys(json.scripts ?? {}));
  } catch {
    return new Set();
  }
}

function buildCheckPlan(cwd: string): string[] | null {
  if (existsSync(join(cwd, ".swt-check"))) return ["./.swt-check"];

  const cmds: string[] = [];

  const scripts = pkgScripts(cwd);
  if (scripts.has("typecheck")) cmds.push("pnpm typecheck");
  else if (scripts.has("tsc")) cmds.push("pnpm exec tsc --noEmit");
  if (scripts.has("lint")) cmds.push("pnpm lint");
  if (scripts.has("test")) cmds.push("pnpm test --run");

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

function isGreen(cwd: string): Result {
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

// O_EXCL-based lock with bounded retry. Stale locks > 1h are reaped.
function withParentLock<T>(repoRoot: string, fn: () => T): T {
  const lockPath = join(repoRoot, ".git", "swt.lock");
  const STALE_MS = 60 * 60 * 1000;
  const start = Date.now();
  while (true) {
    try {
      const fd = openSync(lockPath, "wx");
      try {
        return fn();
      } finally {
        closeSync(fd);
        rmSync(lockPath, { force: true });
      }
    } catch (e) {
      const err = e as NodeJS.ErrnoException;
      if (err.code !== "EEXIST") throw err;
      // Reap stale locks.
      try {
        const stat = require("node:fs").statSync(lockPath);
        if (Date.now() - stat.mtimeMs > STALE_MS) {
          rmSync(lockPath, { force: true });
          continue;
        }
      } catch {
        /* race: lock vanished, retry */
      }
      if (Date.now() - start > 10 * 60 * 1000) {
        process.stderr.write("Timed out waiting for parent repo lock.\n");
        process.exit(1);
      }
      spawnSync("sh", ["-c", "sleep 1"]);
    }
  }
}

function create(name: string): void {
  const root = must("git rev-parse --show-toplevel");
  const green = isGreen(root);
  if (!green.ok) {
    process.stderr.write(`HEAD not green: ${green.out}`);
    process.exit(1);
  }
  const branch = `swt/${name}-${Date.now().toString(36)}`;
  const path = resolve(root, "..", `${name}.swt`);
  must(`git worktree add -b ${branch} ${path} HEAD`, root);
  // Print only the path on stdout — callers can capture cleanly.
  process.stdout.write(path + "\n");
}

function merge(wtPath: string): void {
  const wt = resolve(wtPath);
  const root = must("git rev-parse --show-toplevel");
  if (resolve(root) === wt) {
    process.stderr.write("Refusing: that's the parent worktree.\n");
    process.exit(1);
  }
  if (!existsSync(wt)) {
    process.stderr.write(`No such worktree: ${wt}\n`);
    process.exit(1);
  }

  // Parent HEAD must be green: refusing to silently advance past an in-progress
  // red commit in the parent worktree (mirrors the create-time invariant).
  const parentGreen = isGreen(root);
  if (!parentGreen.ok) {
    process.stderr.write(
      `Parent worktree not green: ${parentGreen.out}` +
        `Refusing to merge — finish your red→green cycle in the parent first.\n`,
    );
    process.exit(1);
  }

  const green = isGreen(wt);
  if (!green.ok) {
    process.stderr.write(`Subagent worktree not green: ${green.out}`);
    process.exit(1);
  }

  const branch = must("git rev-parse --abbrev-ref HEAD", wt);
  const parentBranch = must("git rev-parse --abbrev-ref HEAD", root);

  withParentLock(root, () => {
    const ff = sh(`git merge --ff-only ${branch}`, root);
    if (!ff.ok) {
      process.stderr.write("Parent advanced; rebasing subagent onto parent…\n");
      const rebase = sh(`git rebase ${parentBranch}`, wt);
      if (!rebase.ok) {
        process.stderr.write(rebase.out);
        process.stderr.write(`\nResolve conflicts in ${wt}, then re-run: swt merge ${wt}\n`);
        process.exit(1);
      }
      const reGreen = isGreen(wt);
      if (!reGreen.ok) {
        process.stderr.write(`Not green after rebase: ${reGreen.out}`);
        process.exit(1);
      }
      must(`git merge --ff-only ${branch}`, root);
    }
    must(`git worktree remove ${wt}`, root);
    must(`git branch -d ${branch}`, root);
    process.stdout.write(`merged ${branch}, removed ${wt}\n`);
  });
}

const [cmd, ...args] = process.argv.slice(2);
if (cmd === "create" && args[0]) create(args[0]);
else if (cmd === "merge" && args[0]) merge(args[0]);
else {
  process.stderr.write("usage: swt {create <name>|merge <worktree-path>}\n");
  process.exit(2);
}
