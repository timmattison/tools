// Behavioral tests for install-bin. The core regression under test: installing
// over an existing destination MUST give the destination a new inode. A naive
// in-place copy (cp semantics) keeps the inode, and on Apple Silicon macOS the
// kernel's per-vnode code-signature cache then kills every exec of the new
// bytes with SIGKILL (Code Signature Invalid) — the "Killed" / exit 137 trap.
//
// Run: npx tsx --test install-bin/install-bin.test.ts
//
// Parallel-safety: every test gets its own mkdtempSync sandbox (unique per
// call), so concurrent runs of this suite never share a path.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  chmodSync,
  constants,
  accessSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { InstallError, installBinary, verifyExec } from "./install-bin.ts";

const sandbox = (): string => mkdtempSync(join(tmpdir(), "install-bin-test-"));

const writeExecutable = (path: string, content: string): void => {
  writeFileSync(path, content);
  chmodSync(path, 0o755);
};

test("installing over an existing destination allocates a new inode", (t) => {
  const dir = sandbox();
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  const source = join(dir, "source-bin");
  const dest = join(dir, "dest-bin");
  writeExecutable(source, "#!/bin/sh\necho new-build\n");
  writeExecutable(dest, "#!/bin/sh\necho old-build\n");
  const oldInode = statSync(dest).ino;

  const result = installBinary(source, dest);

  assert.notEqual(
    statSync(dest).ino,
    oldInode,
    "destination kept its inode — this is the exact macOS signature-cache SIGKILL bug",
  );
  assert.equal(result.replacedExisting, true);
});

test("installed file matches source content and is executable", (t) => {
  const dir = sandbox();
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  const source = join(dir, "source-bin");
  const dest = join(dir, "sub", "dir", "dest-bin");
  writeExecutable(source, "#!/bin/sh\necho payload\n");

  const result = installBinary(source, dest);

  assert.equal(readFileSync(dest, "utf8"), "#!/bin/sh\necho payload\n");
  assert.doesNotThrow(() => accessSync(dest, constants.X_OK));
  assert.equal(result.replacedExisting, false, "nothing existed at dest yet");
});

test("refuses to install a file onto itself", (t) => {
  const dir = sandbox();
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  const source = join(dir, "the-bin");
  writeExecutable(source, "#!/bin/sh\necho hi\n");

  assert.throws(
    () => installBinary(source, source),
    (err: unknown) =>
      err instanceof InstallError && /same file/i.test(err.message),
  );
  assert.equal(
    readFileSync(source, "utf8"),
    "#!/bin/sh\necho hi\n",
    "refusal must not destroy the file",
  );
});

test("refuses a missing source with a clear error", (t) => {
  const dir = sandbox();
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  assert.throws(
    () => installBinary(join(dir, "nope"), join(dir, "dest")),
    (err: unknown) =>
      err instanceof InstallError && /does not exist/i.test(err.message),
  );
});

test("verifyExec reports a binary the kernel SIGKILLs as not ok", (t) => {
  const dir = sandbox();
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  const suicidal = join(dir, "gets-killed");
  writeExecutable(suicidal, "#!/bin/sh\nkill -KILL $$\n");

  const verdict = verifyExec(suicidal, "--version");

  assert.equal(verdict.ok, false);
  assert.ok(!verdict.ok && verdict.signal === "SIGKILL");
  assert.ok(
    !verdict.ok && verdict.hint.length > 0,
    "a SIGKILL verdict must carry a diagnostic hint",
  );
});

test("verifyExec reports a binary that execs normally as ok", (t) => {
  const dir = sandbox();
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  const healthy = join(dir, "healthy");
  writeExecutable(healthy, "#!/bin/sh\necho v1.2.3\nexit 0\n");

  const verdict = verifyExec(healthy, "--version");

  assert.ok(verdict.ok && verdict.exitCode === 0);
});
