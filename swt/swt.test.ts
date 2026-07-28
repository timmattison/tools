// Baseline tests for the swt green-check plan builder and the git argv boundary.
//
// Every fixture lives under a process-unique temp root so two concurrent copies
// of this suite (parallel agents, a manual run racing ./test.sh) never clobber
// each other's directories.

import assert from "node:assert/strict";
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, describe, test } from "node:test";

import { git, gitMust, validateWorktreeName } from "./git.ts";
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

/**
 * Creates an empty git repository under the process-unique fixture root.
 *
 * @returns Absolute path to the initialized repository.
 */
function makeGitRepo(): string {
  const dir = makeFixture({});
  const init = git(["init", "--quiet"], dir);
  assert.ok(init.ok, `git init failed: ${init.out}`);
  return dir;
}

// These tests exist to prove there is no shell between swt and git. Each one
// feeds git an argument that a shell would mangle — a space (word splitting),
// a `;` (command separator), a `$(…)` (command substitution) — and asserts git
// saw one literal argument and that nothing was executed.
describe("git argv boundary", () => {
  /** Argument that only survives intact if no shell ever parses it. */
  const HOSTILE = "weird ; $(touch pwned) && echo no | tee bad.txt";

  test("an argument containing spaces is one argument, not several", () => {
    const dir = makeGitRepo();
    const set = git(["config", "--local", "swt.testvalue", "one two three"], dir);
    assert.ok(set.ok, `git config failed: ${set.out}`);
    // Under `sh -c`, this would have reached git as four argv entries and failed.
    assert.equal(gitMust(["config", "--local", "--get", "swt.testvalue"], dir), "one two three");
  });

  test("shell metacharacters are stored verbatim, never interpreted", () => {
    const dir = makeGitRepo();
    const set = git(["config", "--local", "swt.testvalue", HOSTILE], dir);
    assert.ok(set.ok, `git config failed: ${set.out}`);
    assert.equal(gitMust(["config", "--local", "--get", "swt.testvalue"], dir), HOSTILE);
    assert.ok(!existsSync(join(dir, "pwned")), "command substitution was executed");
    assert.ok(!existsSync(join(dir, "bad.txt")), "pipeline was executed");
  });

  test("a path argument full of metacharacters round-trips through the index", () => {
    const dir = makeGitRepo();
    const evil = `${HOSTILE}.txt`;
    writeFileSync(join(dir, evil), "contents\n");

    const add = git(["add", "--", evil], dir);
    assert.ok(add.ok, `git add failed: ${add.out}`);

    // ls-files -z emits raw, unquoted paths — an exact-match assertion.
    const listed = git(["ls-files", "-z"], dir);
    assert.ok(listed.ok, `git ls-files failed: ${listed.out}`);
    assert.deepEqual(
      listed.out.split("\0").filter((p) => p.length > 0),
      [evil],
    );
    assert.ok(!existsSync(join(dir, "pwned")), "command substitution was executed");
  });

  test("a leading-dash argument is not swallowed as an option by a shell", () => {
    const dir = makeGitRepo();
    const set = git(["config", "--local", "swt.testvalue", "--not-an-option"], dir);
    assert.ok(set.ok, `git config failed: ${set.out}`);
    assert.equal(gitMust(["config", "--local", "--get", "swt.testvalue"], dir), "--not-an-option");
  });

  test("a failing git command reports ok=false with git's own output", () => {
    const dir = makeGitRepo();
    const r = git(["rev-parse", "--verify", "definitely-not-a-ref"], dir);
    assert.equal(r.ok, false);
    assert.ok(r.out.length > 0, "expected git's stderr to be captured");
  });

  test("gitMust returns trimmed combined output on success", () => {
    const dir = makeGitRepo();
    const out = gitMust(["config", "--local", "swt.testvalue", "trimmed"], dir);
    assert.equal(out, "");
    assert.equal(gitMust(["config", "--local", "--get", "swt.testvalue"], dir), "trimmed");
  });
});

describe("validateWorktreeName", () => {
  const rejected: { name: string; why: string }[] = [
    { name: "fix parser", why: "a space splits the branch name from the path" },
    { name: "a;rm -rf /", why: "a semicolon is a command separator" },
    { name: "$(touch pwned)", why: "command substitution" },
    { name: "feat/foo", why: "a slash silently nests the branch and the path" },
    { name: "..", why: "escapes the worktree parent directory" },
    { name: ".", why: "resolves to the parent directory itself" },
    { name: "../escape", why: "path traversal" },
    { name: "-b", why: "a leading dash is read as a git option" },
    { name: "-force", why: "a leading dash is read as a git option" },
    { name: "", why: "an empty name yields an empty path component" },
    { name: "café", why: "non-ASCII is outside the allowed set" },
    { name: "with\nnewline", why: "a newline breaks ref parsing" },
  ];

  for (const { name, why } of rejected) {
    test(`rejects ${JSON.stringify(name)} — ${why}`, () => {
      assert.equal(validateWorktreeName(name), null);
    });
  }

  const accepted = ["fix-parser", "fix_parser", "issue42", "v1.2.3", "A-Z_a-z.0-9", "x"];

  for (const name of accepted) {
    test(`accepts ${JSON.stringify(name)}`, () => {
      assert.equal(validateWorktreeName(name), name);
    });
  }
});
