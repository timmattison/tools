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
import type { SpawnOptions, SpawnSyncOptionsWithStringEncoding } from "node:child_process";
import type { Result } from "./green-check.ts";

/**
 * `spawnSync`'s options with `detached` added back.
 *
 * Node honors `detached` for `spawnSync` — it reaches the same spawn path as the
 * asynchronous `spawn` — but says so nowhere: the option is documented only for
 * `spawn`, and `@types/node` likewise declares it on `SpawnOptions` while
 * omitting it from `SpawnSyncOptions`. Borrowing the field from `SpawnOptions`
 * rather than restating `detached?: boolean` keeps this pinned to Node's own
 * declaration, and lets the extension collapse into a harmless no-op on the day
 * `@types/node` declares the option where it is actually accepted.
 */
type ShieldableSpawnSyncOptions = SpawnSyncOptionsWithStringEncoding &
  Pick<SpawnOptions, "detached">;

/**
 * Runs git and captures its combined output.
 *
 * Exported only so the process-group guard in the test suite can exercise this
 * exact call rather than a hand-rolled imitation of it; production callers want
 * {@link git}, {@link gitMust} or {@link removeWorktree}, which fix `shielded`
 * to the value their situation calls for.
 *
 * @param args - Arguments to git, one array element per argv entry.
 * @param cwd - Directory to run git in; defaults to the current working directory.
 * @param shielded - Whether to put git in a process group of its own, out of
 *   reach of a signal aimed at swt's. See {@link removeWorktree} for why that is
 *   the right call for teardown and the wrong one for everything else.
 * @returns Git's success flag and its combined stdout/stderr.
 */
export const runGit = (args: string[], cwd: string | undefined, shielded: boolean): Result => {
  // `detached` is the whole shield, and it rests on undocumented behavior:
  // verified to work on Node 26, but absent from `spawnSync`'s documented
  // options, so a future release could drop the pass-through and turn this line
  // into a silent no-op. Nothing would throw — teardown's git would simply move
  // back into swt's process group, where a second Ctrl-C kills it mid-`worktree
  // remove` and orphans both the worktree and the branch it still claims (see
  // {@link removeWorktree}). The "a shielded git runs outside swt's process
  // group" test in swt.test.ts is what makes that regression a test failure
  // instead of a bug that quietly comes back.
  const options: ShieldableSpawnSyncOptions = { cwd, encoding: "utf8", detached: shielded };
  const r = spawnSync("git", args, options);
  return { ok: r.status === 0, out: (r.stdout ?? "") + (r.stderr ?? "") };
};

/**
 * Runs a git command, capturing its combined output. Arguments are passed to
 * git directly rather than through a shell, so spaces, `;`, `$(…)` and every
 * other metacharacter in `args` are always literal argument text.
 *
 * Interruptible: a Ctrl-C reaches this git the same way it reaches swt, which is
 * what you want for work the user is waiting on and can abandon.
 *
 * @param args - Arguments to git, one array element per argv entry.
 * @param cwd - Directory to run git in; defaults to the current working directory.
 * @returns Git's success flag and its combined stdout/stderr.
 */
export const git = (args: string[], cwd?: string): Result => runGit(args, cwd, false);

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
 * Best-effort is not the same as abandonable, though, so unlike every other git
 * call in swt these two run in a process group of their own. Teardown is most
 * often what a Ctrl-C *asked for*, and a terminal sends Ctrl-C to the whole
 * foreground process group — so an impatient second one would kill the very
 * command that is carrying out the first. Cut between these two calls, that
 * leaves the worst possible state: a worktree that survived and a branch that
 * cannot be deleted while it does. Out of the group, teardown finishes on its
 * own terms, and finishes even if swt itself is killed once it has started.
 *
 * @param root - Repository worktree to run git from; never the one being removed.
 * @param path - Worktree directory to delete.
 * @param branch - Branch checked out in that worktree.
 * @returns Ok only when both commands succeeded; `out` is their combined output.
 */
export function removeWorktree(root: string, path: string, branch: string): Result {
  const removed = runGit(["worktree", "remove", "--force", path], root, true);
  const deleted = runGit(["branch", "-D", branch], root, true);
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
