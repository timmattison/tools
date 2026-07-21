#!/usr/bin/env -S npx tsx
// install-bin — install a locally built binary without tripping macOS's
// code-signature cache.
//
// Stub for the red TDD cycle: naive in-place copy — the exact buggy behavior
// (cp semantics keep the destination inode) the tests exist to forbid.

import { copyFileSync } from "node:fs";

export class InstallError extends Error {}

export type InstallResult = { dest: string; replacedExisting: boolean };

export type ExecVerdict =
  | { ok: true; exitCode: number }
  | { ok: false; signal: string | null; hint: string };

export function installBinary(source: string, dest: string): InstallResult {
  copyFileSync(source, dest);
  return { dest, replacedExisting: false };
}

export function verifyExec(_bin: string, _arg: string): ExecVerdict {
  return { ok: true, exitCode: 0 };
}
