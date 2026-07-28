// Baseline tests for the swt green-check plan builder, the git argv boundary,
// and the parent merge lock.
//
// Every fixture lives under a process-unique temp root so two concurrent copies
// of this suite (parallel agents, a manual run racing ./test.sh) never clobber
// each other's directories.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, describe, test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

import { git, gitMust, validateWorktreeName, worktreeDirt } from "./git.ts";
import { buildCheckPlan, pkgScripts } from "./green-check.ts";
import { withParentLock } from "./swt.ts";

/** Process-unique root for this suite's fixtures; removed in the `after` hook. */
const FIXTURE_ROOT = join(tmpdir(), `swt-test-${process.pid}-${process.hrtime.bigint()}`);

let fixtureCounter = 0;

/**
 * Materializes a fixture directory containing the given files.
 *
 * @param files - Map of repo-relative path to file contents; parent directories are created.
 * @param label - Prefix for the directory name; a counter is always appended so two
 *   fixtures with the same label never share a path.
 * @returns Absolute path to the fixture directory.
 */
function makeFixture(files: Record<string, string>, label = "case"): string {
  const dir = join(FIXTURE_ROOT, `${label}-${fixtureCounter++}`);
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
      name: "a package.json with no check scripts yields no plan, install alone is not green",
      files: {
        "package.json": pkgJson("build"),
        "pnpm-lock.yaml": "lockfileVersion: '9.0'\n",
      },
      expected: null,
    },
    {
      name: "a package.json with no scripts block at all yields no plan",
      files: {
        "package.json": JSON.stringify({ name: "fixture" }),
        "pnpm-lock.yaml": "lockfileVersion: '9.0'\n",
      },
      expected: null,
    },
    {
      name: "a checkless package.json adds no install to an otherwise cargo-only plan",
      files: {
        "package.json": pkgJson("build"),
        "pnpm-lock.yaml": "lockfileVersion: '9.0'\n",
        "Cargo.toml": "[package]\nname = \"fixture\"\n",
      },
      expected: ["cargo check", "cargo test", "cargo clippy -- -D warnings"],
    },
    {
      name: "a lone test script still gets its install",
      files: {
        "package.json": pkgJson("build", "test"),
        "pnpm-lock.yaml": "lockfileVersion: '9.0'\n",
      },
      expected: ["pnpm install --frozen-lockfile", "pnpm test --run"],
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

  // `pnpm install --frozen-lockfile` verifies nothing on its own — it exists only
  // to make the js checks runnable in a fresh worktree. A plan that is just an
  // install would report green having checked nothing, so the install rides along
  // with the js checks or not at all.
  test("no js check means no install, even when the plan is non-empty", () => {
    const dir = makeFixture({
      "package.json": pkgJson("build", "dev", "start"),
      "pnpm-lock.yaml": "lockfileVersion: '9.0'\n",
      "Cargo.toml": "[package]\nname = \"fixture\"\n",
    });
    const plan = buildCheckPlan(dir) ?? [];
    assert.ok(
      !plan.some((cmd) => cmd.includes("pnpm install")),
      `plan must not install without a js check to run: ${JSON.stringify(plan)}`,
    );
  });

  test("the install is first when js checks do exist", () => {
    const dir = makeFixture({
      "package.json": pkgJson("typecheck", "lint", "test"),
      "pnpm-lock.yaml": "lockfileVersion: '9.0'\n",
    });
    assert.deepEqual(buildCheckPlan(dir), [
      "pnpm install --frozen-lockfile",
      "pnpm typecheck",
      "pnpm lint",
      "pnpm test --run",
    ]);
  });
});

// The `.swt-check` escape hatch is documented as a file you *drop* at the repo
// root — i.e. uncommitted, and therefore absent from the fresh checkout of HEAD
// the green check now runs in. So the override has to be resolved against the
// parent repo root while the commands still run in the worktree being checked.
describe("buildCheckPlan config root", () => {
  /** Trivial always-green override script. */
  const SWT_CHECK = "#!/bin/sh\nexit 0\n";
  /** Minimal manifest that makes a directory auto-detect as a cargo repo. */
  const CARGO_TOML = '[package]\nname = "fixture"\n';
  /** The plan `CARGO_TOML` alone produces — what must NOT win over an override. */
  const CARGO_PLAN = ["cargo check", "cargo test", "cargo clippy -- -D warnings"];

  test(".swt-check in the target is the whole plan, as an absolute quoted path", () => {
    const dir = makeFixture({ ".swt-check": SWT_CHECK });
    assert.deepEqual(buildCheckPlan(dir), [`'${join(dir, ".swt-check")}'`]);
  });

  test(".swt-check wins alone over package.json and Cargo.toml", () => {
    const dir = makeFixture({
      ".swt-check": SWT_CHECK,
      "package.json": pkgJson("typecheck", "lint", "test"),
      "pnpm-lock.yaml": "lockfileVersion: '9.0'\n",
      "Cargo.toml": CARGO_TOML,
    });
    assert.deepEqual(buildCheckPlan(dir), [`'${join(dir, ".swt-check")}'`]);
  });

  test(".swt-check at the config root is used when the target has none", () => {
    const configRoot = makeFixture({ ".swt-check": SWT_CHECK });
    const target = makeFixture({ "Cargo.toml": CARGO_TOML });
    assert.deepEqual(buildCheckPlan(target, configRoot), [`'${join(configRoot, ".swt-check")}'`]);
  });

  test(".swt-check at the config root beats auto-detection in the target", () => {
    const configRoot = makeFixture({ ".swt-check": SWT_CHECK });
    const target = makeFixture({
      "package.json": pkgJson("typecheck", "lint", "test"),
      "pnpm-lock.yaml": "lockfileVersion: '9.0'\n",
      "Cargo.toml": CARGO_TOML,
    });
    assert.deepEqual(buildCheckPlan(target, configRoot), [`'${join(configRoot, ".swt-check")}'`]);
  });

  test("no .swt-check at the config root leaves the target's auto-detected plan alone", () => {
    const configRoot = makeFixture({});
    const target = makeFixture({ "Cargo.toml": CARGO_TOML });
    assert.deepEqual(buildCheckPlan(target, configRoot), CARGO_PLAN);
  });

  test("no .swt-check anywhere still yields no plan", () => {
    assert.equal(buildCheckPlan(makeFixture({}), makeFixture({})), null);
  });

  // The emitted command is handed to `sh -c`, so a repo root containing a space,
  // a quote or a `$` has to survive that round trip intact.
  test("a config root path with a space and a quote is shell-quoted for sh -c", () => {
    const configRoot = makeFixture({ ".swt-check": "#!/bin/sh\nexit 7\n" }, "wei'rd $config root");
    chmodSync(join(configRoot, ".swt-check"), 0o755);
    const target = makeFixture({ "Cargo.toml": CARGO_TOML });

    const plan = buildCheckPlan(target, configRoot);
    const quoted = `'${join(configRoot, ".swt-check").replaceAll("'", "'\\''")}'`;
    assert.deepEqual(plan, [quoted]);

    // Not tautological: this actually runs the emitted string the way swt does.
    const r = spawnSync("sh", ["-c", plan![0]], { cwd: target, encoding: "utf8" });
    assert.equal(r.status, 7, `sh could not run the quoted override: ${r.stderr}`);
  });
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

/** Path of the one tracked file `makeCommittedGitRepo` leaves behind. */
const TRACKED_FILE = "tracked.txt";

/**
 * Creates a git repository with a single committed file under the process-unique
 * fixture root. Identity and excludes are pinned locally so the fixture behaves
 * the same regardless of the developer's global git config.
 *
 * @returns Absolute path to the initialized repository.
 */
function makeCommittedGitRepo(): string {
  const dir = makeFixture({ [TRACKED_FILE]: "original\n" });
  const init = git(["init", "-b", "main", "--quiet"], dir);
  assert.ok(init.ok, `git init failed: ${init.out}`);
  for (const [key, value] of [
    ["user.email", "swt-test@example.com"],
    ["user.name", "swt test"],
    ["commit.gpgsign", "false"],
    ["core.excludesFile", "/dev/null"],
  ]) {
    const set = git(["config", "--local", key, value], dir);
    assert.ok(set.ok, `git config ${key} failed: ${set.out}`);
  }
  const add = git(["add", "--", TRACKED_FILE], dir);
  assert.ok(add.ok, `git add failed: ${add.out}`);
  const commit = git(["commit", "--quiet", "-m", "fixture"], dir);
  assert.ok(commit.ok, `git commit failed: ${commit.out}`);
  return dir;
}

// The parent guard and the subagent guard need different scopes, so dirt
// detection is a parameter rather than a fixed `git status --porcelain`.
describe("worktreeDirt", () => {
  test("a freshly committed repo is clean under either scope", () => {
    const dir = makeCommittedGitRepo();
    assert.equal(worktreeDirt(dir, { includeUntracked: false }), "");
    assert.equal(worktreeDirt(dir, { includeUntracked: true }), "");
  });

  test("an untracked file is dirt only when untracked files are included", () => {
    const dir = makeCommittedGitRepo();
    writeFileSync(join(dir, "scratch.txt"), "scratch\n");
    assert.equal(worktreeDirt(dir, { includeUntracked: false }), "");
    assert.match(worktreeDirt(dir, { includeUntracked: true }), /scratch\.txt/);
  });

  // Finding A in one test: the documented escape hatch is an uncommitted file at
  // the repo root, so an untracked-sensitive parent guard hard-blocks every merge
  // for anyone following the documented workflow.
  test("an uncommitted .swt-check escape hatch is not parent dirt", () => {
    const dir = makeCommittedGitRepo();
    writeFileSync(join(dir, ".swt-check"), "#!/bin/sh\nexit 0\n");
    assert.equal(worktreeDirt(dir, { includeUntracked: false }), "");
  });

  test("a modified tracked file is dirt even when untracked files are excluded", () => {
    const dir = makeCommittedGitRepo();
    writeFileSync(join(dir, TRACKED_FILE), "changed\n");
    assert.match(worktreeDirt(dir, { includeUntracked: false }), /tracked\.txt/);
  });

  test("a staged addition is dirt even when untracked files are excluded", () => {
    const dir = makeCommittedGitRepo();
    writeFileSync(join(dir, "added.txt"), "added\n");
    const add = git(["add", "--", "added.txt"], dir);
    assert.ok(add.ok, `git add failed: ${add.out}`);
    assert.match(worktreeDirt(dir, { includeUntracked: false }), /added\.txt/);
  });

  test("a deleted tracked file is dirt even when untracked files are excluded", () => {
    const dir = makeCommittedGitRepo();
    rmSync(join(dir, TRACKED_FILE));
    assert.match(worktreeDirt(dir, { includeUntracked: false }), /tracked\.txt/);
  });
});

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

/**
 * Adds a linked worktree to a fixture repository. A linked worktree's `.git` is
 * a regular *file* holding `gitdir: …`, which is the whole point of these
 * fixtures: it is the shape `swt merge` actually runs in, since the workflow
 * this tool serves never works in the main repo.
 *
 * @param repo - Repository to add the worktree to; must already have a commit.
 * @returns Absolute path to the new linked worktree.
 */
function addLinkedWorktree(repo: string): string {
  const id = fixtureCounter++;
  const path = join(FIXTURE_ROOT, `linked-worktree-${id}`);
  const add = git(["worktree", "add", "--quiet", "-b", `linked-${id}`, path, "HEAD"], repo);
  assert.ok(add.ok, `git worktree add failed: ${add.out}`);
  return path;
}

// The lock that serializes concurrent `swt merge` runs against one parent repo.
// Two things are being pinned here: *where* the lock file lives (the git dir
// shared by every worktree of the repo — not `<worktree>/.git`, which is a file
// in a linked worktree), and that it is always released, including on the exit
// paths that skip `finally` entirely.
describe("withParentLock", () => {
  /** Basename of the lock file, relative to the repo's shared git dir. */
  const LOCK_NAME = "swt.lock";

  /** This module's sibling: the entry point child-process fixtures import. */
  const SWT_MODULE = fileURLToPath(new URL("./swt.ts", import.meta.url));

  /** Child fixture exit status meaning "exited from inside the locked region". */
  const EXIT_INSIDE_LOCK = 17;

  /** Child fixture exit status meaning "the lock file was never created". */
  const EXIT_NO_LOCK = 18;

  test("creates the lock while fn runs, removes it after, and returns fn's value", () => {
    const repo = makeCommittedGitRepo();
    const lock = join(repo, ".git", LOCK_NAME);

    const returned = withParentLock(repo, () => {
      assert.ok(existsSync(lock), `expected a lock at ${lock} while fn runs`);
      return "fn result";
    });

    assert.equal(returned, "fn result");
    assert.ok(!existsSync(lock), `lock at ${lock} outlived the locked region`);
  });

  // Bug A: `.git` is only a directory in the main worktree. In a linked worktree
  // it is a regular file, so `join(repoRoot, ".git", "swt.lock")` is an ENOTDIR.
  test("locks from a linked worktree, whose .git is a file rather than a directory", () => {
    const repo = makeCommittedGitRepo();
    const wt = addLinkedWorktree(repo);
    assert.ok(
      statSync(join(wt, ".git")).isFile(),
      "fixture precondition: a linked worktree's .git must be a file",
    );
    const lock = join(repo, ".git", LOCK_NAME);

    let ran = false;
    withParentLock(wt, () => {
      ran = true;
      assert.ok(existsSync(lock), `expected the lock in the shared git dir at ${lock}`);
    });

    assert.ok(ran, "fn never ran");
    assert.ok(!existsSync(lock), `lock at ${lock} outlived the locked region`);
  });

  // Serialization scope: a merge launched from any worktree of a repo must
  // contend for the *same* file, so a lock written by one is seen by the other.
  // Aging it past the staleness horizon keeps the assertion instant instead of
  // parking the test on the retry backoff.
  test("sees a stale lock left in the shared git dir by another worktree, and reaps it", () => {
    const repo = makeCommittedGitRepo();
    const wt = addLinkedWorktree(repo);
    const lock = join(repo, ".git", LOCK_NAME);
    writeFileSync(lock, "");
    const longAgo = new Date(Date.now() - 2 * 60 * 60 * 1000);
    utimesSync(lock, longAgo, longAgo);

    let ran = false;
    withParentLock(wt, () => {
      ran = true;
    });

    assert.ok(ran, `a stale lock at ${lock} was never reaped from the linked worktree`);
    assert.ok(!existsSync(lock), `lock at ${lock} outlived the locked region`);
  });

  test("releases the lock when fn throws", () => {
    const repo = makeCommittedGitRepo();
    const lock = join(repo, ".git", LOCK_NAME);

    assert.throws(
      () =>
        withParentLock(repo, () => {
          throw new Error("boom");
        }),
      /boom/,
    );

    assert.ok(!existsSync(lock), `a throwing fn left ${lock} behind`);
  });

  // Bug B, and the only assertion that can actually observe it: `process.exit`
  // skips `finally`, so this has to be watched from outside the process. The
  // rebase-conflict path inside the locked region is exactly this shape, and a
  // leaked lock blocks every later merge until the one-hour stale reap.
  test("releases the lock when fn exits the process outright", () => {
    const repo = makeCommittedGitRepo();
    const lock = join(repo, ".git", LOCK_NAME);
    const script = join(FIXTURE_ROOT, `exit-inside-lock-${fixtureCounter++}.ts`);
    writeFileSync(
      script,
      [
        `import { existsSync } from "node:fs";`,
        `import { withParentLock } from ${JSON.stringify(pathToFileURL(SWT_MODULE).href)};`,
        ``,
        `withParentLock(${JSON.stringify(repo)}, () => {`,
        `  if (!existsSync(${JSON.stringify(lock)})) process.exit(${EXIT_NO_LOCK});`,
        `  process.exit(${EXIT_INSIDE_LOCK});`,
        `});`,
        ``,
      ].join("\n"),
    );

    const r = spawnSync("npx", ["tsx", script], { cwd: dirname(SWT_MODULE), encoding: "utf8" });
    assert.equal(
      r.status,
      EXIT_INSIDE_LOCK,
      `child did not exit from inside the locked region: ${r.stdout}${r.stderr}`,
    );
    assert.ok(!existsSync(lock), `process.exit inside the locked region left ${lock} behind`);
  });
});
