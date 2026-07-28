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
