#!/usr/bin/env -S npx tsx
// install-bin — install a locally built binary without tripping macOS's
// code-signature cache.
//
//   install-bin <source-binary> [name]   → install as ~/.local/bin/<name>
//                                          (name defaults to the source basename)
//   --dest <dir>                         → destination directory (default ~/.local/bin)
//   --verify-arg <arg>                   → arg for the post-install exec check (default --version)
//   --no-verify                          → skip the exec check
//
// Why this exists: on Apple Silicon macOS, `cp` over an EXISTING binary reuses
// the destination inode, and the kernel caches code signatures per vnode. The
// cache still holds the old build's signature, so every exec of the new bytes
// dies with SIGKILL (Code Signature Invalid) — the shell prints "Killed"
// (exit 137) even though `codesign -vv` says the file on disk is valid.
//
// Invariants enforced:
//   1. The destination is unlinked before copying, so the installed file always
//      lands on a fresh inode the kernel has never cached.
//   2. The installed copy is exec'd once after install; a signal death fails
//      the install loudly instead of leaving a booby-trapped binary on PATH.
//      On a SIGKILL the tool re-signs ad-hoc (codesign -f -s -) and retries
//      once before giving up.

import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  realpathSync,
  rmSync,
  statSync,
} from "node:fs";
import { homedir, platform } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

export class InstallError extends Error {}

export type InstallResult = { dest: string; replacedExisting: boolean };

export type ExecVerdict =
  | { ok: true; exitCode: number }
  | { ok: false; signal: string | null; hint: string };

const SIGKILL_DARWIN_HINT =
  "SIGKILL at exec on macOS usually means the kernel rejected the code " +
  "signature (stale per-vnode signature cache from an in-place overwrite, or " +
  "an unsigned/modified binary). Reinstalling onto a fresh inode or " +
  "`codesign -f -s - <path>` fixes it.";

// Copy `source` to `dest` such that `dest` always ends up on a fresh inode:
// the destination is unlinked first, never overwritten in place. Creates the
// destination directory if needed and carries over the source's file mode.
export function installBinary(source: string, dest: string): InstallResult {
  const sourceStat = (() => {
    try {
      return statSync(source);
    } catch {
      throw new InstallError(`source binary does not exist: ${source}`);
    }
  })();
  if (!sourceStat.isFile()) {
    throw new InstallError(`source is not a regular file: ${source}`);
  }

  const replacedExisting = existsSync(dest);
  if (replacedExisting && realpathSync(source) === realpathSync(dest)) {
    throw new InstallError(
      `source and destination are the same file: ${realpathSync(source)}`,
    );
  }

  mkdirSync(dirname(dest), { recursive: true });
  rmSync(dest, { force: true });
  copyFileSync(source, dest);
  chmodSync(dest, sourceStat.mode & 0o7777);
  return { dest, replacedExisting };
}

// Exec the installed binary once to prove the kernel will actually run it.
// A normal exit (any code) means exec succeeded — the signature check already
// passed — so only signal deaths and spawn failures are verdicts against it.
export function verifyExec(bin: string, arg: string): ExecVerdict {
  const run = spawnSync(bin, [arg], { encoding: "utf8", timeout: 15_000 });
  if (run.error) {
    return { ok: false, signal: null, hint: `exec failed: ${run.error.message}` };
  }
  if (run.signal) {
    const hint =
      run.signal === "SIGKILL" && platform() === "darwin"
        ? SIGKILL_DARWIN_HINT
        : `process died from ${run.signal}`;
    return { ok: false, signal: run.signal, hint };
  }
  return { ok: true, exitCode: run.status ?? 0 };
}

const usage = (): never => {
  console.error(
    "usage: install-bin <source-binary> [name] [--dest <dir>] [--verify-arg <arg>] [--no-verify]",
  );
  process.exit(2);
};

export function main(argv: string[]): number {
  const positional: string[] = [];
  let destDir = join(homedir(), ".local", "bin");
  let verifyArg = "--version";
  let verify = true;

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--dest") destDir = argv[++i] ?? usage();
    else if (arg === "--verify-arg") verifyArg = argv[++i] ?? usage();
    else if (arg === "--no-verify") verify = false;
    else if (arg === "--help" || arg === "-h") usage();
    else if (arg.startsWith("--")) usage();
    else positional.push(arg);
  }
  if (positional.length < 1 || positional.length > 2) usage();

  const source = resolve(positional[0]);
  const dest = join(destDir, positional[1] ?? basename(source));

  const { replacedExisting } = installBinary(source, dest);
  console.log(
    `installed ${source} → ${dest}${replacedExisting ? " (replaced existing, fresh inode)" : ""}`,
  );
  if (!verify) return 0;

  let verdict = verifyExec(dest, verifyArg);
  if (!verdict.ok && verdict.signal === "SIGKILL" && platform() === "darwin") {
    console.error("exec check got SIGKILL; re-signing ad-hoc and retrying once…");
    spawnSync("codesign", ["-f", "-s", "-", dest], { encoding: "utf8" });
    verdict = verifyExec(dest, verifyArg);
  }

  if (!verdict.ok) {
    console.error(`FAILED: ${dest} does not survive exec (${verdict.signal ?? "spawn error"})`);
    console.error(verdict.hint);
    return 1;
  }
  console.log(
    `verified: \`${basename(dest)} ${verifyArg}\` execs cleanly (exit ${verdict.exitCode})`,
  );
  if (verdict.exitCode !== 0) {
    console.log(
      "note: nonzero exit from the verify arg — exec itself worked, so the binary is not signature-blocked",
    );
  }
  return 0;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    process.exit(main(process.argv.slice(2)));
  } catch (err) {
    if (err instanceof InstallError) {
      console.error(`install-bin: ${err.message}`);
      process.exit(1);
    }
    throw err;
  }
}
