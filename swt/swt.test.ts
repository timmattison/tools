// Baseline tests for the swt green-check plan builder, the git argv boundary,
// and the parent merge lock.
//
// Every fixture lives under a process-unique temp root so two concurrent copies
// of this suite (parallel agents, a manual run racing ./test.sh) never clobber
// each other's directories.

import assert from "node:assert/strict";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  realpathSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { after, describe, test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

import { git, gitMust, removeWorktree, runGit, validateWorktreeName, worktreeDirt } from "./git.ts";
import { buildCheckPlan, pkgScripts } from "./green-check.ts";
import { withParentLock } from "./swt.ts";

/** Process-unique root for this suite's fixtures; removed in the `after` hook. */
const FIXTURE_ROOT = join(tmpdir(), `swt-test-${process.pid}-${process.hrtime.bigint()}`);

/** This module's sibling: the entry point child-process fixtures run and import. */
const SWT_MODULE = fileURLToPath(new URL("./swt.ts", import.meta.url));

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

// The install is a *setup* step smuggled into a *verification* step, and it is
// not inert: `pnpm install --frozen-lockfile` prunes extraneous packages and
// undoes local `pnpm link`s. That is acceptable in a fresh worktree, which has
// no dependencies and nothing to lose, and unacceptable in the parent worktree
// the user is living in — which `isGreen` also checks. An already-populated
// node_modules is the tell: the target is not fresh, so the check must inspect
// it without touching it.
describe("buildCheckPlan install gating on node_modules", () => {
  /** Lockfile contents; only its existence matters to the plan. */
  const LOCKFILE = "lockfileVersion: '9.0'\n";
  /** Minimal manifest that makes a directory auto-detect as a cargo repo. */
  const CARGO_TOML = '[package]\nname = "fixture"\n';
  /** A file inside node_modules, so `makeFixture` materializes the directory. */
  const NODE_MODULES_MARKER = "node_modules/.modules.yaml";
  /** The js checks a full package.json produces, in order, without any install. */
  const JS_CHECKS = ["pnpm typecheck", "pnpm lint", "pnpm test --run"];

  /** Every command in a plan that is a pnpm install, for pinpointing failures. */
  const installsIn = (plan: string[] | null): string[] =>
    (plan ?? []).filter((cmd) => cmd.includes("pnpm install"));

  test("an existing node_modules suppresses the install entirely", () => {
    const dir = makeFixture({
      "package.json": pkgJson("typecheck", "lint", "test"),
      "pnpm-lock.yaml": LOCKFILE,
      [NODE_MODULES_MARKER]: "hoistPattern:\n  - '*'\n",
    });
    const plan = buildCheckPlan(dir);
    assert.deepEqual(
      installsIn(plan),
      [],
      `verification must not install into a populated tree: ${JSON.stringify(plan)}`,
    );
    assert.deepEqual(plan, JS_CHECKS);
  });

  test("a fresh worktree with no node_modules still gets the install first", () => {
    const dir = makeFixture({
      "package.json": pkgJson("typecheck", "lint", "test"),
      "pnpm-lock.yaml": LOCKFILE,
    });
    assert.deepEqual(buildCheckPlan(dir), ["pnpm install --frozen-lockfile", ...JS_CHECKS]);
  });

  test("a populated tauri-shaped repo runs js then both cargo manifests, no install", () => {
    const dir = makeFixture({
      "package.json": pkgJson("typecheck", "lint", "test"),
      "pnpm-lock.yaml": LOCKFILE,
      [NODE_MODULES_MARKER]: "hoistPattern:\n  - '*'\n",
      "Cargo.toml": CARGO_TOML,
      "src-tauri/Cargo.toml": '[package]\nname = "fixture-tauri"\n',
    });
    assert.deepEqual(buildCheckPlan(dir), [
      ...JS_CHECKS,
      "cargo check",
      "cargo test",
      "cargo clippy -- -D warnings",
      "cargo check --manifest-path src-tauri/Cargo.toml",
      "cargo test --manifest-path src-tauri/Cargo.toml",
      "cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings",
    ]);
  });

  // Dropping the install is the *only* thing node_modules may change: the checks
  // themselves, their order, and the cargo commands after them stay identical.
  test("node_modules removes the install and nothing else", () => {
    const files = {
      "package.json": pkgJson("tsc", "lint", "test"),
      "pnpm-lock.yaml": LOCKFILE,
      "Cargo.toml": CARGO_TOML,
    };
    const fresh = buildCheckPlan(makeFixture(files)) ?? [];
    const populated = buildCheckPlan(
      makeFixture({ ...files, [NODE_MODULES_MARKER]: "hoistPattern:\n  - '*'\n" }),
    );
    assert.deepEqual(installsIn(fresh), ["pnpm install --frozen-lockfile"]);
    assert.deepEqual(
      populated,
      fresh.filter((cmd) => !cmd.includes("pnpm install")),
    );
  });

  // A tree with no js checks never had an install to drop, so node_modules is a
  // no-op there — it must not add, remove, or reorder anything.
  test("node_modules alone changes nothing when there are no js checks", () => {
    const files = {
      "package.json": pkgJson("build"),
      "pnpm-lock.yaml": LOCKFILE,
      "Cargo.toml": CARGO_TOML,
    };
    const populated = buildCheckPlan(
      makeFixture({ ...files, [NODE_MODULES_MARKER]: "hoistPattern:\n  - '*'\n" }),
    );
    assert.deepEqual(populated, buildCheckPlan(makeFixture(files)));
    assert.deepEqual(populated, ["cargo check", "cargo test", "cargo clippy -- -D warnings"]);
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

// The shield that keeps interrupt teardown alive is a single undocumented
// option: `detached` on `spawnSync`. Node honors it, but documents it only for
// the asynchronous `spawn`, so a release that dropped the pass-through would
// turn it into a silent no-op — teardown's git would slide back into swt's
// process group, and the second-Ctrl-C orphan bug would return without one
// assertion anywhere going red. These two tests are that alarm, and they call
// `runGit` itself: a hand-rolled `spawnSync` here would only prove that Node
// still honors an option the production code might have stopped passing.
describe("runGit process-group shielding", () => {
  /**
   * A git alias that reports the process group git itself is running in.
   *
   * A `!`-prefixed alias body is handed to a shell that git forks, so `$$` is
   * that shell's pid and the group it reports is the one it inherited from git.
   * Supplying the alias with `-c` leaves the fixture repository's own config
   * untouched, and beats any `alias.pg` in the developer's global config.
   */
  const PGID_ALIAS = "!ps -o pgid= -p $$";

  /**
   * Reports the process group a git launched through the production `runGit`
   * ran in.
   *
   * @param cwd - Repository to run git in.
   * @param shielded - Passed straight through to `runGit`.
   * @returns Git's process group id, as decimal digits.
   */
  function gitProcessGroup(cwd: string, shielded: boolean): string {
    const r = runGit(["-c", `alias.pg=${PGID_ALIAS}`, "pg"], cwd, shielded);
    assert.ok(r.ok, `the pgid alias failed to run: ${r.out}`);
    const pgid = r.out.trim();
    assert.match(pgid, /^\d+$/, `expected a process group id, got ${JSON.stringify(r.out)}`);
    return pgid;
  }

  /**
   * Reports this process's own process group — the one a terminal Ctrl-C aims
   * at, and therefore the group teardown has to stay out of.
   *
   * @returns This process's process group id, as decimal digits.
   */
  function ownProcessGroup(): string {
    const r = spawnSync("ps", ["-o", "pgid=", "-p", String(process.pid)], { encoding: "utf8" });
    assert.equal(r.status, 0, `ps failed: ${r.stderr ?? ""}`);
    return (r.stdout ?? "").trim();
  }

  /** Process groups are a POSIX notion; there is nothing to assert on Windows. */
  const posixOnly = { skip: process.platform === "win32" ? "POSIX process groups only" : false };

  test("a shielded git runs outside swt's process group", posixOnly, () => {
    const dir = makeGitRepo();
    const own = ownProcessGroup();
    assert.notEqual(
      gitProcessGroup(dir, true),
      own,
      "shielded git shared swt's process group, so a Ctrl-C aimed at swt would " +
        "kill teardown mid-flight — `detached` is no longer being honored by spawnSync",
    );
  });

  test("an unshielded git runs inside swt's process group", posixOnly, () => {
    const dir = makeGitRepo();
    const own = ownProcessGroup();
    assert.equal(
      gitProcessGroup(dir, false),
      own,
      "unshielded git escaped swt's process group; work the user is waiting on " +
        "must stay interruptible by the Ctrl-C that abandons it",
    );
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

/**
 * Names a path directly under the fixture root, spelled the way git spells it back.
 *
 * `getcwd(2)` always answers with the physical path and on macOS `$TMPDIR` is a
 * symlink, so every path git and swt print is already symlink-resolved — string
 * comparisons against a `FIXTURE_ROOT`-derived path would otherwise miss.
 *
 * @param name - Basename to place under the fixture root; nothing is created.
 * @returns The absolute, symlink-resolved path.
 */
function fixturePath(name: string): string {
  mkdirSync(FIXTURE_ROOT, { recursive: true });
  return join(realpathSync(FIXTURE_ROOT), name);
}

/**
 * Drops an executable `.swt-check` override at a repository root.
 *
 * @param repo - Repository root the override belongs to. It is deliberately left
 *   untracked, which is exactly how the escape hatch is documented.
 * @param body - Shell script contents, shebang included.
 */
function writeSwtCheck(repo: string, body: string): void {
  const path = join(repo, ".swt-check");
  writeFileSync(path, body);
  chmodSync(path, 0o755);
}

/**
 * How long the shimmed teardown command stalls before handing over to real git.
 * Only a floor matters: it has to outlast the poll interval the test notices the
 * shim's sentinel on, so the interrupt cannot arrive after teardown is over.
 */
const TEARDOWN_HOLD_SECONDS = 1;

/**
 * Materializes a PATH-shadowing `git` that announces swt's teardown and holds
 * its first command open.
 *
 * The interrupt under test has to land *inside* teardown, and teardown is two
 * back-to-back git commands that together take a few tens of milliseconds — so
 * timing a signal into that window with a sleep is a coin flip, and a flaky test
 * for a flaky bug proves nothing. Making the first teardown command its own
 * synchronization point removes the guesswork: it touches a sentinel the test
 * waits on, then stalls before handing over to the real git. Every other git
 * invocation is passed straight through, so nothing else about the run changes.
 *
 * @param label - Distinguishes this shim's directory from every other fixture's.
 * @returns The directory to prepend to PATH, and the sentinel it touches.
 */
function makeTeardownShim(label: string): { dir: string; sentinel: string } {
  // Resolved here, before the shim can shadow it, so the shim can hand over.
  const resolved = spawnSync("sh", ["-c", "command -v git"], { encoding: "utf8" });
  assert.equal(resolved.status, 0, `could not resolve the real git: ${resolved.stderr}`);
  const realGit = resolved.stdout.trim();

  const dir = join(FIXTURE_ROOT, `git-shim-${label}`);
  mkdirSync(dir, { recursive: true });
  const sentinel = join(dir, "teardown-started");
  const shim = join(dir, "git");
  writeFileSync(
    shim,
    [
      "#!/bin/sh",
      `if [ "$1" = "worktree" ] && [ "$2" = "remove" ]; then`,
      `  : > ${JSON.stringify(sentinel)}`,
      `  sleep ${TEARDOWN_HOLD_SECONDS}`,
      "fi",
      `exec ${JSON.stringify(realGit)} "$@"`,
      "",
    ].join("\n"),
  );
  chmodSync(shim, 0o755);
  return { dir, sentinel };
}

/**
 * Lists the branches `swt create <name>` could have left behind in a repository.
 *
 * @param repo - Repository to inspect.
 * @param name - Worktree base name that was passed to `swt create`.
 * @returns Matching branch names; empty when the branch was cleaned up.
 */
function swtBranches(repo: string, name: string): string[] {
  const r = git(["branch", "--list", "--format=%(refname:short)", `swt/${name}-*`], repo);
  assert.ok(r.ok, `git branch --list failed: ${r.out}`);
  return r.out
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

/** Ceiling on every wait for a child `swt`, so a hang fails instead of parking the suite. */
const CHILD_DEADLINE_MS = 60_000;

/**
 * Spawns swt as a child in its own process group, with no launcher in between.
 *
 * The launcher is not incidental to any test that signals swt. `npx tsx` is a
 * parent *process*: on SIGINT it relays the signal to the node process actually
 * running swt, waits 30ms for an IPC acknowledgement that swt — blocked in the
 * synchronous `spawnSync` of a green check — can never send, re-sends, and
 * SIGKILLs at ~60ms. A test launched that way measures tsx's kill deadline
 * against swt's teardown rather than swt's own guarantee, and fails whenever a
 * loaded machine pushes teardown past 60ms. Re-using this process's own node
 * binary and `execArgv` runs swt under exactly the same TypeScript loader (tsx's
 * `--import` when the suite itself is run under tsx) with the launcher removed,
 * so the signal lands on swt and nothing else can kill it.
 *
 * `detached` puts swt in its own process group, so a signal sent to `-pid`
 * reaches the children swt spawned too — which is what a terminal Ctrl-C does.
 *
 * @param args - Arguments following the program name, e.g. `["create", name]`.
 * @param cwd - Directory to run swt in.
 * @param env - Environment entries merged over this process's own.
 * @returns The spawned child; stdout is discarded and stderr is piped.
 */
function spawnSwt(args: string[], cwd: string, env: NodeJS.ProcessEnv = {}): ChildProcess {
  return spawn(process.execPath, [...process.execArgv, SWT_MODULE, ...args], {
    cwd,
    detached: true,
    stdio: ["ignore", "ignore", "pipe"],
    env: { ...process.env, ...env },
  });
}

/**
 * Reports whether any process remains in a process group.
 *
 * A signalled swt is not necessarily the last process standing: the green check
 * it spawned, and the git commands it runs while tearing down, are children too.
 * Waiting on the tracked child alone would therefore sample the filesystem
 * mid-cleanup; the group is empty only once every one of them has gone. Signal 0
 * performs the existence check without delivering anything.
 *
 * @param pid - Pid of the group leader, as spawned with `detached: true`.
 * @returns True while at least one member of the group is alive.
 */
function groupAlive(pid: number): boolean {
  try {
    process.kill(-pid, 0);
    return true;
  } catch {
    return false;
  }
}

/**
 * Polls until a condition holds, failing the test rather than hanging forever.
 *
 * @param ready - Condition to poll; must be cheap and side-effect free.
 * @param what - Phrase describing what is awaited, used in the failure message.
 */
async function waitUntil(ready: () => boolean, what: string): Promise<void> {
  const deadline = Date.now() + CHILD_DEADLINE_MS;
  while (!ready()) {
    if (Date.now() > deadline) {
      assert.fail(`timed out after ${CHILD_DEADLINE_MS}ms waiting for ${what}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
}

/**
 * Awaits a promise under the same deadline, so a wedged child cannot park the suite.
 *
 * @param promise - Promise to await.
 * @param what - Phrase describing what is awaited, used in the failure message.
 * @returns Whatever `promise` resolves to.
 */
async function withDeadline<T>(promise: Promise<T>, what: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error(`timed out after ${CHILD_DEADLINE_MS}ms waiting for ${what}`)),
          CHILD_DEADLINE_MS,
        );
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

// Finding #4: tearing an unverified worktree down takes two git commands, and
// both used to have their results dropped on the floor. Teardown is genuinely
// best-effort — a worktree whose `.git` link is gone cannot be removed, and a
// branch cannot be deleted while a registered worktree still claims it — so the
// outcome has to travel back to the caller instead of being assumed.
describe("removeWorktree", () => {
  test("removes the worktree and its branch, and reports success", () => {
    const repo = makeCommittedGitRepo();
    const branch = `swt/teardown-${fixtureCounter++}`;
    const path = fixturePath(`teardown-${fixtureCounter++}.swt`);
    const add = git(["worktree", "add", "--quiet", "-b", branch, path, "HEAD"], repo);
    assert.ok(add.ok, `git worktree add failed: ${add.out}`);
    assert.ok(existsSync(path), "fixture precondition: the worktree must exist first");

    const torn = removeWorktree(repo, path, branch);

    assert.equal(torn.ok, true, `teardown reported failure: ${torn.out}`);
    assert.ok(!existsSync(path), `${path} survived a teardown that reported success`);
    assert.ok(
      !gitMust(["worktree", "list"], repo).includes(path),
      `${path} is still a registered worktree`,
    );
    assert.equal(gitMust(["branch", "--list", "--format=%(refname:short)", branch], repo), "");
  });

  test("reports failure, and git's own output, when neither command can succeed", () => {
    const repo = makeCommittedGitRepo();
    const stranger = fixturePath(`not-a-worktree-${fixtureCounter++}`);
    mkdirSync(stranger, { recursive: true });
    const branch = "swt/never-existed";

    const torn = removeWorktree(repo, stranger, branch);

    assert.equal(
      torn.ok,
      false,
      `teardown claimed success for ${stranger}: ${JSON.stringify(torn)}`,
    );
    // Both commands are attempted and both are reported. A caller shown only the
    // first failure would still not know whether the branch is lying around.
    assert.ok(torn.out.includes(stranger), `git's worktree complaint is missing:\n${torn.out}`);
    assert.ok(torn.out.includes(branch), `git's branch complaint is missing:\n${torn.out}`);
  });
});

// The user-facing half of finding #4, plus nit N1. Both are only observable from
// outside the process: what `swt create` prints when its teardown fails, and what
// it leaves behind when the user gives up on a long green check.
describe("swt create cleanup", () => {
  /**
   * A `.swt-check` that deletes the worktree's own `.git` link and then fails.
   * Teardown afterwards fails for two independent reasons: git refuses to remove
   * a working tree whose `.git` has vanished, and refuses to delete a branch a
   * registered worktree still claims. No permission games, so it behaves the
   * same for an unprivileged user and for root.
   */
  const SABOTAGE_CHECK = '#!/bin/sh\nrm -f "$PWD/.git"\nexit 1\n';

  test("never reports a cleanup that did not happen", () => {
    const repo = makeCommittedGitRepo();
    const name = `sabotaged${fixtureCounter++}`;
    const wt = fixturePath(`${name}.swt`);
    writeSwtCheck(repo, SABOTAGE_CHECK);

    const r = spawnSync("npx", ["tsx", SWT_MODULE, "create", name], { cwd: repo, encoding: "utf8" });
    const stderr = r.stderr ?? "";

    assert.equal(r.status, 1, `expected a failed create: ${stderr}`);
    // The claim is only a lie if the orphans really are orphans.
    assert.ok(existsSync(wt), `fixture precondition: ${wt} should have survived teardown`);
    const branches = swtBranches(repo, name);
    assert.equal(
      branches.length,
      1,
      `expected one leftover branch, got ${JSON.stringify(branches)}`,
    );
    const branch = branches[0] ?? "";

    assert.ok(
      !stderr.includes("Cleaned up"),
      `claimed cleanup while ${wt} and ${branch} both survived:\n${stderr}`,
    );
    assert.match(stderr, /fatal:/, `git's own teardown output was swallowed:\n${stderr}`);
    assert.ok(
      stderr.includes(`git worktree remove --force '${wt}' && git branch -D ${branch}`),
      `no copy-pasteable recovery command naming ${wt} and ${branch}:\n${stderr}`,
    );
  });

  // Nit N1: the green check now runs *after* the worktree exists, so a user who
  // Ctrl-Cs a long check is the one paying for the new ordering unless swt tears
  // its own half-built state down on the way out.
  test("interrupting the green check leaves no worktree and no branch behind", async () => {
    const repo = makeCommittedGitRepo();
    const name = `interrupted${fixtureCounter++}`;
    const wt = fixturePath(`${name}.swt`);
    const started = fixturePath(`check-started-${fixtureCounter++}`);
    // The sentinel is the synchronization point — it proves the check is running,
    // and therefore that the worktree exists. The sleep only guarantees the signal
    // lands mid-check rather than after it.
    writeSwtCheck(repo, `#!/bin/sh\n: > ${JSON.stringify(started)}\nsleep 30\n`);

    const child = spawnSwt(["create", name], repo);
    let stderr = "";
    child.stderr?.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
    });
    const exited = new Promise<void>((resolve) => child.on("exit", () => resolve()));
    const pid = child.pid;
    assert.ok(pid !== undefined, "child process never started");

    try {
      await waitUntil(() => existsSync(started), "the green check to start");
      assert.ok(existsSync(wt), `precondition: create must have built ${wt} before checking it`);
      process.kill(-pid, "SIGINT");
      await withDeadline(exited, "swt to exit after SIGINT");
      await waitUntil(() => !groupAlive(pid), "the interrupted swt to finish exiting");
    } finally {
      // Never leave a 30-second sleep running, whatever went wrong above.
      try {
        process.kill(-pid, "SIGKILL");
      } catch {
        /* the process group is already gone */
      }
      await exited;
    }

    assert.ok(!existsSync(wt), `SIGINT left an orphaned worktree at ${wt}:\n${stderr}`);
    assert.deepEqual(swtBranches(repo, name), [], `SIGINT left an orphaned branch:\n${stderr}`);
  });

  // One Ctrl-C asks swt to stop; a second one, while it is still stopping, must
  // not undo the stopping. Teardown is two git commands run as children of swt,
  // in swt's process group — which is exactly where a terminal sends Ctrl-C — so
  // an impatient second interrupt kills the teardown command mid-flight. The
  // worktree removal never completes, the branch the surviving worktree still
  // claims cannot be deleted, and both are orphaned by the very interrupt that
  // asked for them to go away.
  test("a second interrupt during teardown still leaves no worktree and no branch", async () => {
    const repo = makeCommittedGitRepo();
    const name = `reinterrupted${fixtureCounter++}`;
    const wt = fixturePath(`${name}.swt`);
    const started = fixturePath(`check-started-${fixtureCounter++}`);
    writeSwtCheck(repo, `#!/bin/sh\n: > ${JSON.stringify(started)}\nsleep 30\n`);
    const shim = makeTeardownShim(`${name}-${fixtureCounter++}`);

    const child = spawnSwt(["create", name], repo, {
      PATH: `${shim.dir}${delimiter}${process.env.PATH ?? ""}`,
    });
    let stderr = "";
    child.stderr?.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
    });
    const exited = new Promise<void>((resolve) => child.on("exit", () => resolve()));
    const pid = child.pid;
    assert.ok(pid !== undefined, "child process never started");

    try {
      await waitUntil(() => existsSync(started), "the green check to start");
      assert.ok(existsSync(wt), `precondition: create must have built ${wt} before checking it`);
      process.kill(-pid, "SIGINT");
      // Both sentinels are the synchronization: the first proved the check was
      // running, this one proves teardown is. Only then is the second interrupt
      // sent, so "it arrived mid-teardown" is a fact rather than a hope.
      await waitUntil(() => existsSync(shim.sentinel), "teardown to start");
      process.kill(-pid, "SIGINT");
      await withDeadline(exited, "swt to exit after a second SIGINT");
      await waitUntil(() => !groupAlive(pid), "the interrupted swt to finish exiting");
    } finally {
      // Never leave the 30-second sleep — or a stalled shim — running.
      try {
        process.kill(-pid, "SIGKILL");
      } catch {
        /* the process group is already gone */
      }
      await exited;
    }

    assert.ok(
      !existsSync(wt),
      `a second SIGINT truncated teardown, orphaning the worktree at ${wt}:\n${stderr}`,
    );
    assert.deepEqual(
      swtBranches(repo, name),
      [],
      `a second SIGINT truncated teardown, orphaning the branch:\n${stderr}`,
    );
  });
});
