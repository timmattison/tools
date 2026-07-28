// Baseline tests for the swt green-check plan builder.
//
// Every fixture lives under a process-unique temp root so two concurrent copies
// of this suite (parallel agents, a manual run racing ./test.sh) never clobber
// each other's directories.

import assert from "node:assert/strict";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, describe, test } from "node:test";

import { buildCheckPlan, pkgScripts } from "./green-check.ts";

/** Process-unique root for this suite's fixtures; removed in the `after` hook. */
const FIXTURE_ROOT = join(tmpdir(), `swt-test-${process.pid}-${process.hrtime.bigint()}`);

let fixtureCounter = 0;

/**
 * Materializes a fixture directory containing the given files.
 *
 * @param files - Map of repo-relative path to file contents; parent directories are created.
 * @returns Absolute path to the fixture directory.
 */
function makeFixture(files: Record<string, string>): string {
  const dir = join(FIXTURE_ROOT, `case-${fixtureCounter++}`);
  mkdirSync(dir, { recursive: true });
  for (const [relPath, contents] of Object.entries(files)) {
    const full = join(dir, relPath);
    mkdirSync(dirname(full), { recursive: true });
    writeFileSync(full, contents);
  }
  return dir;
}

/** Serializes a package.json with the given script names (bodies are irrelevant to the plan). */
const pkgJson = (...scripts: string[]): string =>
  JSON.stringify({
    name: "fixture",
    scripts: Object.fromEntries(scripts.map((s) => [s, "true"])),
  });

after(() => {
  rmSync(FIXTURE_ROOT, { recursive: true, force: true });
});

describe("buildCheckPlan", () => {
  const cases: { name: string; files: Record<string, string>; expected: string[] | null }[] = [
    {
      name: "empty directory has no plan",
      files: {},
      expected: null,
    },
    {
      name: ".swt-check escape hatch is the whole plan",
      files: { ".swt-check": "#!/bin/sh\nexit 0\n" },
      expected: ["./.swt-check"],
    },
    {
      name: ".swt-check wins alone over package.json and Cargo.toml",
      files: {
        ".swt-check": "#!/bin/sh\nexit 0\n",
        "package.json": pkgJson("typecheck", "lint", "test"),
        "pnpm-lock.yaml": "lockfileVersion: '9.0'\n",
        "Cargo.toml": "[package]\nname = \"fixture\"\n",
      },
      expected: ["./.swt-check"],
    },
    {
      name: "root Cargo.toml only uses no --manifest-path",
      files: { "Cargo.toml": "[package]\nname = \"fixture\"\n" },
      expected: ["cargo check", "cargo test", "cargo clippy -- -D warnings"],
    },
    {
      name: "root plus src-tauri manifests check root first",
      files: {
        "Cargo.toml": "[package]\nname = \"fixture\"\n",
        "src-tauri/Cargo.toml": "[package]\nname = \"fixture-tauri\"\n",
      },
      expected: [
        "cargo check",
        "cargo test",
        "cargo clippy -- -D warnings",
        "cargo check --manifest-path src-tauri/Cargo.toml",
        "cargo test --manifest-path src-tauri/Cargo.toml",
        "cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings",
      ],
    },
    {
      name: "pnpm lockfile adds a frozen install before the js checks",
      files: {
        "package.json": pkgJson("typecheck", "lint", "test"),
        "pnpm-lock.yaml": "lockfileVersion: '9.0'\n",
      },
      expected: [
        "pnpm install --frozen-lockfile",
        "pnpm typecheck",
        "pnpm lint",
        "pnpm test --run",
      ],
    },
    {
      name: "a tsc script substitutes for a missing typecheck script",
      files: {
        "package.json": pkgJson("tsc", "lint", "test"),
        "pnpm-lock.yaml": "lockfileVersion: '9.0'\n",
      },
      expected: [
        "pnpm install --frozen-lockfile",
        "pnpm exec tsc --noEmit",
        "pnpm lint",
        "pnpm test --run",
      ],
    },
    {
      name: "typecheck wins over tsc when both scripts exist",
      files: { "package.json": pkgJson("typecheck", "tsc") },
      expected: ["pnpm typecheck"],
    },
    {
      name: "no pnpm lockfile means no install command",
      files: { "package.json": pkgJson("typecheck", "lint", "test") },
      expected: ["pnpm typecheck", "pnpm lint", "pnpm test --run"],
    },
    {
      name: "tauri-shaped repo runs js checks then both cargo manifests",
      files: {
        "package.json": pkgJson("typecheck", "lint", "test"),
        "pnpm-lock.yaml": "lockfileVersion: '9.0'\n",
        "Cargo.toml": "[package]\nname = \"fixture\"\n",
        "src-tauri/Cargo.toml": "[package]\nname = \"fixture-tauri\"\n",
      },
      expected: [
        "pnpm install --frozen-lockfile",
        "pnpm typecheck",
        "pnpm lint",
        "pnpm test --run",
        "cargo check",
        "cargo test",
        "cargo clippy -- -D warnings",
        "cargo check --manifest-path src-tauri/Cargo.toml",
        "cargo test --manifest-path src-tauri/Cargo.toml",
        "cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings",
      ],
    },
  ];

  for (const { name, files, expected } of cases) {
    test(name, () => {
      assert.deepEqual(buildCheckPlan(makeFixture(files)), expected);
    });
  }
});

describe("pkgScripts", () => {
  test("returns an empty set when there is no package.json", () => {
    assert.deepEqual(pkgScripts(makeFixture({})), new Set());
  });

  test("returns the declared script names", () => {
    const dir = makeFixture({ "package.json": pkgJson("typecheck", "lint") });
    assert.deepEqual(pkgScripts(dir), new Set(["typecheck", "lint"]));
  });

  test("returns an empty set for a package.json with no scripts block", () => {
    const dir = makeFixture({ "package.json": JSON.stringify({ name: "fixture" }) });
    assert.deepEqual(pkgScripts(dir), new Set());
  });

  test("returns an empty set for a malformed package.json instead of throwing", () => {
    const dir = makeFixture({ "package.json": "{ this is not json" });
    assert.deepEqual(pkgScripts(dir), new Set());
  });
});
