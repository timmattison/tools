// green-check — what "green" means for a repo, and how to verify it.
//
// This module owns the whole definition of the green check: detecting which
// toolchains a worktree uses (pnpm / cargo / Tauri), assembling the command
// plan, and running it. Callers see only `isGreen(target, configRoot)` (and
// `buildCheckPlan` for inspection/testing) — the pnpm/cargo/Tauri detection
// stays hidden here.
//
// Green check (always runs inside the worktree being checked, never the parent):
//   - .swt-check at `configRoot` — the parent repo root, because the escape hatch
//     is an uncommitted file and so is absent from a fresh checkout of HEAD. Used
//     alone if present, as an absolute shell-quoted path, still run in `target`.
//   Otherwise, detected from `target` and run there, whichever apply, additively
//   (Tauri repos have both):
//   - package.json declaring at least one of typecheck/tsc/lint/test:
//     `pnpm install --frozen-lockfile` (only if pnpm-lock.yaml exists *and*
//     node_modules does not — see below), then those checks. A package.json with
//     none of those scripts contributes nothing — the install alone verifies
//     nothing and must never stand in for a check.
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
 * Wraps a string so `sh -c` sees exactly one literal argument.
 *
 * Check commands are shell strings by design, so the one piece swt splices into
 * them — the absolute path of the `.swt-check` override — has to be quoted: a
 * repo root containing a space, a quote or a `$` would otherwise word-split or
 * expand. Single quotes suppress every expansion; the embedded-quote case is
 * handled by closing, escaping, and reopening (`'` → `'\''`).
 *
 * @param s - Raw string to embed in a shell command.
 * @returns The single-quoted form, safe to concatenate into a command line.
 */
const shellQuote = (s: string): string => `'${s.replaceAll("'", `'\\''`)}'`;

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
  // Resolved against configRoot, run in target. The escape hatch is documented as
  // a file you *drop* at the repo root — uncommitted, and so absent from the fresh
  // checkout of HEAD that `create` checks. Looking it up in the parent keeps that
  // per-developer override working; running it in the target keeps the check
  // honest about what it is verifying.
  const override = join(configRoot, ".swt-check");
  if (existsSync(override)) return [shellQuote(override)];

  const cwd = target;
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
    //
    // And it only rides along into a tree that is actually fresh. `isGreen` also
    // runs against the parent worktree the user is living in, where an install is
    // not a read-only step: `--frozen-lockfile` prunes extraneous packages and
    // undoes local `pnpm link`s. An existing node_modules is the tell that the
    // dependencies are already there — nothing to set up, and something to lose —
    // so verification inspects that tree without touching it.
    if (jsChecks.length > 0) {
      const needsInstall =
        existsSync(join(cwd, "pnpm-lock.yaml")) && !existsSync(join(cwd, "node_modules"));
      if (needsInstall) cmds.push("pnpm install --frozen-lockfile");
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
  const plan = buildCheckPlan(target, configRoot);
  if (!plan) {
    return {
      ok: false,
      out: `No green-check defined. Drop a '.swt-check' executable at ${configRoot}.\n`,
    };
  }
  process.stderr.write(`Running green check in ${target}…`);
  for (const cmd of plan) {
    if (!streamCheck(cmd, target)) return { ok: false, out: `failed: ${cmd}\n` };
  }
  return { ok: true, out: "" };
}
