#!/usr/bin/env -S npx tsx
// swt — subagent worktree helper for parallel TDD.
//
//   swt create <name>          → verify HEAD green, create worktree on a new branch, print path
//   swt merge <worktree-path>  → verify subagent green, ff-merge (rebase if parent advanced), cleanup
//
// Invariants enforced:
//   1. The green check runs INSIDE the new worktree (a clean checkout of HEAD), not the
//      parent — so uncommitted changes in the parent can't trick the check. The worktree
//      and branch are torn down on failure.
//   2. At merge time, BOTH worktrees must be clean (no uncommitted/untracked changes)
//      AND both must pass the green check — so no in-progress red is silently advanced
//      past, and no uncommitted subagent work is lost when the worktree is removed.
//   3. If parent advanced during the subagent's work, rebase + re-verify green before ff-merging.
//   4. Concurrent `swt merge` runs against the same parent are serialized via .git/swt.lock.
//
// The green check itself lives in ./green-check.ts — see that module for what
// counts as green and how pnpm/cargo/Tauri repos are detected.

import { spawnSync } from "node:child_process";
import { closeSync, existsSync, openSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { isGreen, type Result } from "./green-check.ts";

/**
 * Runs a shell command and captures its combined output.
 *
 * @param cmd - Shell command to run.
 * @param cwd - Directory to run it in; defaults to the current working directory.
 * @returns The command's success flag and combined stdout/stderr.
 */
const sh = (cmd: string, cwd?: string): Result => {
  const r = spawnSync("sh", ["-c", cmd], { cwd, encoding: "utf8" });
  return { ok: r.status === 0, out: (r.stdout ?? "") + (r.stderr ?? "") };
};

/**
 * Runs a shell command, aborting the process with its output on failure.
 *
 * @param cmd - Shell command to run.
 * @param cwd - Directory to run it in; defaults to the current working directory.
 * @returns The command's trimmed combined output.
 */
const must = (cmd: string, cwd?: string): string => {
  const r = sh(cmd, cwd);
  if (!r.ok) {
    process.stderr.write(r.out);
    process.exit(1);
  }
  return r.out.trim();
};

/**
 * Runs `fn` while holding the parent repo's merge lock, so concurrent
 * `swt merge` runs are serialized. O_EXCL-based lock with bounded retry;
 * stale locks older than 1h are reaped.
 *
 * @param repoRoot - Parent repository root containing the .git directory.
 * @param fn - Work to perform under the lock.
 * @returns Whatever `fn` returns.
 */
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

/**
 * Creates a subagent worktree on a fresh branch, verifying HEAD is green inside
 * it first. Prints the worktree path on stdout. Tears the worktree and branch
 * down and exits non-zero if the check fails.
 *
 * @param name - Base name for the worktree directory and branch.
 */
function create(name: string): void {
  const root = must("git rev-parse --show-toplevel");
  const branch = `swt/${name}-${Date.now().toString(36)}`;
  const path = resolve(root, "..", `${name}.swt`);

  // Create the worktree first, then run the green check INSIDE it. A fresh
  // worktree is a clean checkout of HEAD with no parent dirty state — so
  // uncommitted changes in the parent can't trick the check.
  must(`git worktree add -b ${branch} ${path} HEAD`, root);

  const cleanup = (): void => {
    sh(`git worktree remove --force ${path}`, root);
    sh(`git branch -D ${branch}`, root);
  };

  let green: Result;
  try {
    green = isGreen(path);
  } catch (e) {
    cleanup();
    throw e;
  }

  if (!green.ok) {
    cleanup();
    process.stderr.write(`HEAD not green: ${green.out}`);
    process.stderr.write(`Cleaned up worktree ${path} and branch ${branch}.\n`);
    process.exit(1);
  }

  // Print only the path on stdout — callers can capture cleanly.
  process.stdout.write(path + "\n");
}

/**
 * Merges a subagent worktree back into the parent: both worktrees must be clean
 * and green, the subagent is rebased if the parent advanced, then fast-forwarded
 * in and torn down. Exits non-zero on any violated invariant.
 *
 * @param wtPath - Path to the subagent worktree to merge.
 */
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

  // Refuse if either worktree is dirty. Parent dirt = in-progress work that
  // shouldn't be silently fast-forwarded over. Subagent dirt = uncommitted work
  // that would vanish when the worktree is removed. `git status --porcelain`
  // catches both modified-tracked and untracked files.
  const checkClean = (cwd: string, label: string): void => {
    const r = sh("git status --porcelain", cwd);
    if (!r.ok) {
      process.stderr.write(r.out);
      process.exit(1);
    }
    if (r.out.trim().length > 0) {
      process.stderr.write(`${label} has uncommitted/untracked changes:\n${r.out}`);
      process.stderr.write("Commit or stash before merging.\n");
      process.exit(1);
    }
  };
  checkClean(root, "Parent worktree");
  checkClean(wt, "Subagent worktree");

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
