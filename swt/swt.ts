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
//   2. At merge time, BOTH worktrees must be clean AND both must pass the green check —
//      so no in-progress red is silently advanced past, and no uncommitted subagent work
//      is lost when the worktree is removed. "Clean" is scoped per side: tracked changes
//      only in the parent (a ff-merge cannot discard untracked files, and the `.swt-check`
//      escape hatch is untracked by design), untracked included in the subagent (whose
//      whole directory is deleted).
//   3. If parent advanced during the subagent's work, rebase + re-verify green before ff-merging.
//   4. Concurrent `swt merge` runs against the same parent are serialized via .git/swt.lock.
//
// The green check itself lives in ./green-check.ts — see that module for what
// counts as green and how pnpm/cargo/Tauri repos are detected.
//
// Every git command runs through ./git.ts as an argv array, never a shell
// string, so caller-supplied names cannot word-split or inject.

import { closeSync, existsSync, openSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { git, gitMust, validateWorktreeName, worktreeDirt, WORKTREE_NAME_RULE } from "./git.ts";
import { isGreen, type Result } from "./green-check.ts";

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
      // Synchronous 1s backoff without spawning a shell to sleep.
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 1000);
    }
  }
}

/**
 * Creates a subagent worktree on a fresh branch, verifying HEAD is green inside
 * it first. Prints the worktree path on stdout. Tears the worktree and branch
 * down and exits non-zero if the check fails.
 *
 * @param rawName - Base name for the worktree directory and branch; rejected up
 *   front unless it satisfies {@link WORKTREE_NAME_RULE}.
 */
function create(rawName: string): void {
  // Validate before touching git: the name becomes both a branch and a path, so
  // `..` or a leading `-` would otherwise create the wrong thing somewhere else.
  const name = validateWorktreeName(rawName);
  if (name === null) {
    process.stderr.write(
      `Invalid worktree name ${JSON.stringify(rawName)} — ${WORKTREE_NAME_RULE}.\n`,
    );
    process.exit(1);
  }

  const root = gitMust(["rev-parse", "--show-toplevel"]);
  const branch = `swt/${name}-${Date.now().toString(36)}`;
  const path = resolve(root, "..", `${name}.swt`);

  // Create the worktree first, then run the green check INSIDE it. A fresh
  // worktree is a clean checkout of HEAD with no parent dirty state — so
  // uncommitted changes in the parent can't trick the check.
  gitMust(["worktree", "add", "-b", branch, path, "HEAD"], root);

  const cleanup = (): void => {
    git(["worktree", "remove", "--force", path], root);
    git(["branch", "-D", branch], root);
  };

  let green: Result;
  try {
    // Checked in the new worktree, but configured from the parent: the
    // `.swt-check` override is uncommitted, so it only exists in `root`.
    green = isGreen(path, root);
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
  const root = gitMust(["rev-parse", "--show-toplevel"]);
  if (resolve(root) === wt) {
    process.stderr.write("Refusing: that's the parent worktree.\n");
    process.exit(1);
  }
  if (!existsSync(wt)) {
    process.stderr.write(`No such worktree: ${wt}\n`);
    process.exit(1);
  }

  // Refuse if either worktree is dirty — but the two guards need different scopes.
  //
  // Parent: tracked changes only. What matters there is in-progress work a
  // fast-forward would silently advance past, and a ff-merge can only ever touch
  // tracked files — git itself refuses to overwrite modified tracked ones, and it
  // never discards untracked ones. Including untracked files here would instead
  // hard-block every merge for anyone using the documented `./.swt-check` escape
  // hatch, which is by definition an uncommitted file at the parent repo root.
  //
  // Subagent: untracked included. Uncommitted *or* untracked work there is
  // genuinely destroyed, because `git worktree remove` deletes the whole
  // directory. This is the early, clearer guard; the `git worktree remove` below
  // is invoked without `--force`, so git's own dirty-worktree refusal is the
  // backstop — but it fires after the merge and reports far less usefully.
  const checkClean = (cwd: string, label: string, includeUntracked: boolean): void => {
    let dirt: string;
    try {
      dirt = worktreeDirt(cwd, { includeUntracked });
    } catch (e) {
      process.stderr.write((e as Error).message);
      process.exit(1);
    }
    if (dirt.length > 0) {
      const kind = includeUntracked ? "uncommitted/untracked" : "uncommitted";
      process.stderr.write(`${label} has ${kind} changes:\n${dirt}\n`);
      process.stderr.write("Commit or stash before merging.\n");
      process.exit(1);
    }
  };
  checkClean(root, "Parent worktree", false);
  checkClean(wt, "Subagent worktree", true);

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

  const green = isGreen(wt, root);
  if (!green.ok) {
    process.stderr.write(`Subagent worktree not green: ${green.out}`);
    process.exit(1);
  }

  const branch = gitMust(["rev-parse", "--abbrev-ref", "HEAD"], wt);
  const parentBranch = gitMust(["rev-parse", "--abbrev-ref", "HEAD"], root);

  withParentLock(root, () => {
    const ff = git(["merge", "--ff-only", branch], root);
    if (!ff.ok) {
      process.stderr.write("Parent advanced; rebasing subagent onto parent…\n");
      const rebase = git(["rebase", parentBranch], wt);
      if (!rebase.ok) {
        process.stderr.write(rebase.out);
        process.stderr.write(`\nResolve conflicts in ${wt}, then re-run: swt merge ${wt}\n`);
        process.exit(1);
      }
      const reGreen = isGreen(wt, root);
      if (!reGreen.ok) {
        process.stderr.write(`Not green after rebase: ${reGreen.out}`);
        process.exit(1);
      }
      gitMust(["merge", "--ff-only", branch], root);
    }
    gitMust(["worktree", "remove", wt], root);
    gitMust(["branch", "-d", branch], root);
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
