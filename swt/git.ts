// git — the single entrance for running git from swt.
//
// Every git command goes through here as an argv array handed straight to
// spawnSync, with no shell in between. That is deliberate and load-bearing:
// branch names and worktree paths are built from caller-supplied argv, so a
// shell in the middle turns `swt create 'a; rm -rf ~'` into arbitrary code
// execution, and even a benign space silently word-splits into the wrong branch
// and the wrong path. There is intentionally NO string-command variant exported
// here — adding a new git call means adding an argv array, which cannot be
// injected into. (Green-check commands are different: those are user-authored
// shell strings by design and live in ./green-check.ts.)

import { spawnSync } from "node:child_process";
import type { Result } from "./green-check.ts";

/**
 * Runs a git command, capturing its combined output. Arguments are passed to
 * git directly rather than through a shell, so spaces, `;`, `$(…)` and every
 * other metacharacter in `args` are always literal argument text.
 *
 * @param args - Arguments to git, one array element per argv entry.
 * @param cwd - Directory to run git in; defaults to the current working directory.
 * @returns Git's success flag and its combined stdout/stderr.
 */
export const git = (args: string[], cwd?: string): Result => {
  const r = spawnSync("git", args, { cwd, encoding: "utf8" });
  return { ok: r.status === 0, out: (r.stdout ?? "") + (r.stderr ?? "") };
};

/**
 * Runs a git command, aborting the process with git's output on failure.
 *
 * @param args - Arguments to git, one array element per argv entry.
 * @param cwd - Directory to run git in; defaults to the current working directory.
 * @returns Git's trimmed combined output.
 */
export const gitMust = (args: string[], cwd?: string): string => {
  const r = git(args, cwd);
  if (!r.ok) {
    process.stderr.write(r.out);
    process.exit(1);
  }
  return r.out.trim();
};

/**
 * Tears down a worktree and the branch checked out in it, forcing both.
 *
 * Teardown is best-effort by nature — git refuses to remove a working tree whose
 * `.git` link has gone missing, and refuses to delete a branch a registered
 * worktree still claims — so the outcome is *reported* rather than assumed. Both
 * commands are attempted even when the first fails, and both outputs come back:
 * a caller shown only the first complaint would not know whether the branch is
 * still lying around too, which is the difference between a usable recovery
 * instruction and a wrong one.
 *
 * @param root - Repository worktree to run git from; never the one being removed.
 * @param path - Worktree directory to delete.
 * @param branch - Branch checked out in that worktree.
 * @returns Ok only when both commands succeeded; `out` is their combined output.
 */
export function removeWorktree(root: string, path: string, branch: string): Result {
  const removed = git(["worktree", "remove", "--force", path], root);
  const deleted = git(["branch", "-D", branch], root);
  return { ok: removed.ok && deleted.ok, out: removed.out + deleted.out };
}

/**
 * Reports a worktree's uncommitted state as git's own porcelain listing.
 *
 * @param cwd - Worktree root to inspect.
 * @param opts - `includeUntracked` decides whether untracked files count as dirt.
 * @returns The trimmed porcelain output; empty string means clean.
 * @throws If git itself fails, carrying git's combined output as the message.
 */
export function worktreeDirt(cwd: string, opts: { includeUntracked: boolean }): string {
  const args = ["status", "--porcelain"];
  if (!opts.includeUntracked) args.push("--untracked-files=no");
  const r = git(args, cwd);
  if (!r.ok) throw new Error(r.out);
  return r.out.trim();
}

declare const worktreeNameBrand: unique symbol;

/**
 * A worktree base name that has passed `validateWorktreeName` and is therefore
 * safe to splice into a branch name and a filesystem path. The brand makes the
 * check unskippable: nothing else in the codebase can produce this type.
 */
export type WorktreeName = string & { readonly [worktreeNameBrand]: "WorktreeName" };

/** Characters a worktree name may be built from. */
const WORKTREE_NAME_PATTERN = /^[A-Za-z0-9._-]+$/;

/** Names that match the pattern but are still meaningless as a path component. */
const RESERVED_WORKTREE_NAMES = new Set([".", ".."]);

/** Human-readable statement of what a worktree name may contain. */
export const WORKTREE_NAME_RULE =
  "allowed: letters, digits, '.', '_' and '-'; must not start with '-', and must not be '.' or '..'";

/**
 * Checks that a caller-supplied worktree base name is safe to turn into a
 * branch name and a worktree path.
 *
 * Passing git argv arrays already removes the injection risk, but an unchecked
 * name still yields nonsense: `../..` escapes the worktree parent directory,
 * a leading `-` is read as an option, and `/` silently nests the branch.
 *
 * @param name - Raw name as supplied on the command line.
 * @returns The branded name, or null if it violates {@link WORKTREE_NAME_RULE}.
 */
export function validateWorktreeName(name: string): WorktreeName | null {
  if (!WORKTREE_NAME_PATTERN.test(name)) return null;
  if (name.startsWith("-")) return null;
  if (RESERVED_WORKTREE_NAMES.has(name)) return null;
  return name as WorktreeName;
}
