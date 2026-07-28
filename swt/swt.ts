#!/usr/bin/env -S npx tsx
// swt — subagent worktree helper for parallel TDD.
//
//   swt create <name>          → verify HEAD green, create worktree on a new branch, print path
//   swt merge <worktree-path>  → verify subagent green, ff-merge (rebase if parent advanced), cleanup
//
// Invariants enforced — each states the guarantee first, then the mechanism:
//   1. Worktrees are only ever created from a green commit. The check runs INSIDE the
//      new worktree — a clean checkout of HEAD, so uncommitted changes in the parent
//      cannot trick it — and both the worktree and its branch are torn down again if the
//      check fails or the run is interrupted. A failed check reports what that teardown
//      actually did, plus the command to finish it by hand, rather than assuming it worked.
//   2. A name from the command line can only ever name the thing it spells. It becomes
//      both a branch and a filesystem path, so it is validated against a restricted
//      character set (see WORKTREE_NAME_RULE) before anything at all is created.
//   3. A merge neither loses work nor advances the parent past an in-progress red. BOTH
//      worktrees must be clean AND both must pass the green check before anything moves.
//      "Clean" is scoped per side: tracked changes only in the parent (a ff-merge cannot
//      discard untracked files, and the `.swt-check` escape hatch is untracked by design),
//      untracked included in the subagent (whose whole directory is deleted).
//   4. What lands in the parent is green as merged, not merely green in isolation. If the
//      parent advanced during the subagent's work, the subagent is rebased onto it and
//      re-verified green before the fast-forward.
//   5. Merges into one repository never interleave. They serialize on a swt.lock in the
//      repo's *shared* git dir, so runs launched from two different worktrees of one repo
//      still block each other, and that lock is released when the merge returns, when it
//      throws, and when the process exits from inside the locked region.
//
// The green check itself lives in ./green-check.ts — see that module for what
// counts as green, how pnpm/cargo/Tauri repos are detected, and where the
// `.swt-check` override is looked up versus where it runs.
//
// Every git command runs through ./git.ts as an argv array, never a shell
// string, so caller-supplied names cannot word-split or inject.

import { closeSync, existsSync, openSync, realpathSync, rmSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  git,
  gitMust,
  removeWorktree,
  validateWorktreeName,
  worktreeDirt,
  WORKTREE_NAME_RULE,
} from "./git.ts";
import { isGreen, shellQuote, type Result } from "./green-check.ts";

/** Basename of the lock file, inside the repository's shared git directory. */
const LOCK_FILE = "swt.lock";

/** Signals swt turns into an ordinary exit so its `exit` hooks still run. */
const TERMINATION_SIGNALS = ["SIGINT", "SIGTERM"] as const;

/** Conventional shell exit status for a death by signal: 128 + signal number. */
const SIGNAL_EXIT_STATUS: Record<(typeof TERMINATION_SIGNALS)[number], number> = {
  SIGINT: 130,
  SIGTERM: 143,
};

/**
 * Turns a terminating signal into a normal exit. Without a listener the signal's
 * default disposition kills the process outright — no `exit` hook runs — which
 * is precisely how a lock, or a worktree that never passed its check, outlives
 * its owner. This listener deliberately does no cleanup of its own: every
 * teardown decision lives in an `exit` hook, so two responsibilities armed at
 * the same time cannot fight over ordering.
 *
 * @param signal - Signal that arrived.
 */
function exitOnSignal(signal: (typeof TERMINATION_SIGNALS)[number]): void {
  process.exit(SIGNAL_EXIT_STATUS[signal]);
}

/** How many teardown responsibilities currently need signals to reach `exit`. */
let signalGuards = 0;

/**
 * Arms signal-to-exit conversion on behalf of one teardown responsibility, so
 * swt only alters the process's signal behaviour while it actually owns
 * something that would otherwise be orphaned.
 */
function armSignalExit(): void {
  if (signalGuards++ === 0) {
    for (const signal of TERMINATION_SIGNALS) process.on(signal, exitOnSignal);
  }
}

/** Drops one responsibility's claim, restoring default signal handling at zero. */
function disarmSignalExit(): void {
  if (signalGuards === 0) return;
  if (--signalGuards === 0) {
    for (const signal of TERMINATION_SIGNALS) process.removeListener(signal, exitOnSignal);
  }
}

/** Locks this process created and is still responsible for removing. */
const heldLocks = new Set<string>();

/**
 * Removes every lock this process is holding. Registered as the `exit` hook, so
 * it also covers `process.exit` calls that unwind no `finally` at all — never
 * touching a lock file this process did not create.
 */
function releaseHeldLocks(): void {
  for (const path of heldLocks) {
    try {
      rmSync(path, { force: true });
    } catch {
      /* best effort: nothing useful to do while the process is going down */
    }
  }
  heldLocks.clear();
}

let lockExitHookInstalled = false;

/**
 * Records a freshly created lock as this process's responsibility and arms the
 * teardown hooks.
 *
 * @param path - Lock file this process just created with O_EXCL.
 */
function holdLock(path: string): void {
  if (heldLocks.size === 0) {
    if (!lockExitHookInstalled) {
      process.on("exit", releaseHeldLocks);
      lockExitHookInstalled = true;
    }
    armSignalExit();
  }
  heldLocks.add(path);
}

/**
 * Removes a lock this process holds and disarms its signal claim once the last
 * one is gone.
 *
 * @param path - Lock file previously passed to {@link holdLock}.
 */
function releaseLock(path: string): void {
  heldLocks.delete(path);
  rmSync(path, { force: true });
  if (heldLocks.size === 0) disarmSignalExit();
}

/** A worktree that exists but has not passed its green check yet. */
type UnverifiedWorktree = { root: string; path: string; branch: string };

/** The unverified worktree this process is on the hook for, if any. */
let unverifiedWorktree: UnverifiedWorktree | null = null;

let worktreeExitHookInstalled = false;

/**
 * Takes responsibility for a worktree that exists but is not verified yet, so a
 * Ctrl-C during a long green check does not leave the user with a half-built
 * worktree and branch they never asked to keep. Mirrors {@link holdLock}: the
 * hooks are armed only for the window where something would actually be
 * orphaned, and dropped by {@link keepUnverifiedWorktree} or
 * {@link removeUnverifiedWorktree}.
 *
 * @param worktree - Worktree just created by `git worktree add`.
 */
function holdUnverifiedWorktree(worktree: UnverifiedWorktree): void {
  if (!worktreeExitHookInstalled) {
    process.on("exit", () => {
      removeUnverifiedWorktree();
    });
    worktreeExitHookInstalled = true;
  }
  unverifiedWorktree = worktree;
  armSignalExit();
}

/** Releases the hold without removing anything: the check passed, so it stays. */
function keepUnverifiedWorktree(): void {
  if (unverifiedWorktree === null) return;
  unverifiedWorktree = null;
  disarmSignalExit();
}

/**
 * Tears down the held worktree, if one is still held. Idempotent, because it is
 * both the `exit` hook and what `create` calls explicitly when the check fails.
 *
 * @returns The teardown outcome, or null when there was nothing left to remove.
 */
function removeUnverifiedWorktree(): Result | null {
  const worktree = unverifiedWorktree;
  if (worktree === null) return null;
  // Clearing the hold first makes a second call a no-op; disarming waits until
  // the git commands are done. An interrupted run is often *still* being
  // signalled while it cleans up — the `npx tsx` launcher relays SIGINT again
  // 30ms later — and dropping the listener mid-teardown would restore the
  // default disposition and kill swt between the two commands.
  unverifiedWorktree = null;
  try {
    return removeWorktree(worktree.root, worktree.path, worktree.branch);
  } finally {
    disarmSignalExit();
  }
}

/**
 * Resolves the lock file that serializes merges for a repository.
 *
 * `.git` is a directory only in the main worktree; in a linked worktree it is a
 * regular *file* holding `gitdir: …`, so joining `.git/swt.lock` onto a worktree
 * root is an ENOTDIR — and the workflow swt serves never merges from the main
 * repo. `--git-common-dir` names the git directory shared by every worktree of
 * the repository, which is also exactly the serialization scope wanted: two
 * `swt merge` runs launched from two different worktrees of one repo must
 * contend for the same lock.
 *
 * @param repoRoot - Any worktree root of the repository.
 * @returns Absolute path of that repository's merge lock file.
 */
function parentLockPath(repoRoot: string): string {
  // Run in the main worktree, git answers with a path relative to its cwd ('.git').
  const commonDir = gitMust(["rev-parse", "--git-common-dir"], repoRoot);
  return join(resolve(repoRoot, commonDir), LOCK_FILE);
}

/**
 * Runs `fn` while holding the parent repo's merge lock, so concurrent
 * `swt merge` runs are serialized. O_EXCL-based lock with bounded retry;
 * stale locks older than 1h are reaped.
 *
 * The lock is released when `fn` returns, when it throws, and when it — or
 * anything it calls — exits the process outright.
 *
 * @param repoRoot - Root of any worktree of the parent repository.
 * @param fn - Work to perform under the lock.
 * @returns Whatever `fn` returns.
 */
export function withParentLock<T>(repoRoot: string, fn: () => T): T {
  const lockPath = parentLockPath(repoRoot);
  const STALE_MS = 60 * 60 * 1000;
  const start = Date.now();
  while (true) {
    try {
      const fd = openSync(lockPath, "wx");
      holdLock(lockPath);
      try {
        return fn();
      } finally {
        closeSync(fd);
        releaseLock(lockPath);
      }
    } catch (e) {
      const err = e as NodeJS.ErrnoException;
      if (err.code !== "EEXIST") throw err;
      // Reap stale locks.
      try {
        const stat = statSync(lockPath);
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
 * down and exits non-zero if the check fails — or if the run is interrupted
 * before it passes — reporting honestly when that teardown does not work.
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
  // Nothing has verified this worktree yet, and the check about to run can take
  // minutes. Until it passes, swt owns removing it — including when the user
  // gives up and hits Ctrl-C.
  holdUnverifiedWorktree({ root, path, branch });

  /** Tears the unverified worktree down and says what actually happened to it. */
  const cleanup = (): void => {
    const torn = removeUnverifiedWorktree();
    if (torn === null || torn.ok) {
      process.stderr.write(`Cleaned up worktree ${path} and branch ${branch}.\n`);
      return;
    }
    // Teardown is best-effort, so claiming it worked would strand the user with
    // an orphaned worktree *and* branch they were told did not exist. Report
    // git's own account, then the command that finishes the job by hand.
    process.stderr.write(torn.out);
    process.stderr.write(
      `Could not clean up ${path}. Remove it by hand:\n` +
        `  git worktree remove --force ${shellQuote(path)} && git branch -D ${branch}\n`,
    );
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
    process.stderr.write(`HEAD not green: ${green.out}`);
    cleanup();
    process.exit(1);
  }

  // Verified: it is the caller's worktree now, not swt's to tear down.
  keepUnverifiedWorktree();

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

  // Nothing inside the locked region may exit the process: `process.exit` skips
  // the `finally` that releases the lock, and a rebase conflict — the very case
  // this path exists to handle — would then block every later merge until the
  // stale reap. So the region *returns* its outcome and the exit happens out
  // here, after the lock is released. `gitMust` is banned in there for the same
  // reason: it exits on failure.
  const outcome = withParentLock(root, (): Result => {
    const ff = git(["merge", "--ff-only", branch], root);
    if (!ff.ok) {
      process.stderr.write("Parent advanced; rebasing subagent onto parent…\n");
      const rebase = git(["rebase", parentBranch], wt);
      if (!rebase.ok) {
        return {
          ok: false,
          out: `${rebase.out}\nResolve conflicts in ${wt}, then re-run: swt merge ${wt}\n`,
        };
      }
      const reGreen = isGreen(wt, root);
      if (!reGreen.ok) return { ok: false, out: `Not green after rebase: ${reGreen.out}` };
      const ffAfterRebase = git(["merge", "--ff-only", branch], root);
      if (!ffAfterRebase.ok) return { ok: false, out: ffAfterRebase.out };
    }
    const removed = git(["worktree", "remove", wt], root);
    if (!removed.ok) return { ok: false, out: removed.out };
    const deleted = git(["branch", "-d", branch], root);
    if (!deleted.ok) return { ok: false, out: deleted.out };
    return { ok: true, out: `merged ${branch}, removed ${wt}\n` };
  });

  if (!outcome.ok) {
    process.stderr.write(outcome.out);
    process.exit(1);
  }
  process.stdout.write(outcome.out);
}

/**
 * Reports whether this module is the program being run rather than a module
 * someone imported. Symlinked installs (`~/.local/bin/swt` → `swt/swt.ts`) mean
 * argv[1] and this module's own URL can name the same file by different paths,
 * so both sides are resolved through the filesystem before comparing.
 *
 * @returns True when swt was invoked as a command line program.
 */
function isProgramEntry(): boolean {
  const entry = process.argv[1];
  if (entry === undefined) return false;
  try {
    return realpathSync(entry) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

// Guarded so importing this module — which the tests do, for `withParentLock` —
// neither parses argv nor exits the importing process.
if (isProgramEntry()) {
  const [cmd, ...args] = process.argv.slice(2);
  if (cmd === "create" && args[0]) create(args[0]);
  else if (cmd === "merge" && args[0]) merge(args[0]);
  else {
    process.stderr.write("usage: swt {create <name>|merge <worktree-path>}\n");
    process.exit(2);
  }
}
