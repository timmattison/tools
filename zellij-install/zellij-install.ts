#!/usr/bin/env -S npx tsx
// zellij-install — refresh the zellij checkout from upstream, build it, prove the
// build actually carries the process-table fix, install it, and report which
// running servers are still executing the old image.
//
// The verification gate is the point. Upstream removed zellij's `ps` shell-out in
// #5324, but every build since v0.44.3 still reports version 0.45.0 — so
// `zellij --version` cannot tell a fixed binary from an unfixed one. Only the
// absence of the embedded `ps` argument vector can, which is what this checks
// before it will overwrite anything.
//
//   zellij-install                 → fetch, reset to upstream/main, build, verify, install
//   zellij-install --no-fetch      → build and install what is already checked out
//   zellij-install --dry-run       → say what would happen, touch nothing
//   zellij-install --status        → skip the build; just report running servers

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, rmSync, copyFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

// ─── constants ──────────────────────────────────────────────────────────────

/** The argument vector a pre-#5324 zellij hands to `ps`. Present in `-ao`
 *  (machine-wide, tty-filtered, ~1.6s on a busy Mac) and `-Ao` (no tty filter,
 *  ~0.1s) builds alike — both scan the whole process table, so both fail the
 *  gate. A build carrying the fix has no `ps` call left to embed. */
const PS_DISCOVERY_MARKER = "ppid,args";

const DEFAULT_REPO = join(homedir(), "code", "zellij");
const DEFAULT_DEST = join(homedir(), ".local", "bin", "zellij");
const DEFAULT_REF = "upstream/main";
const BUILT_BINARY = join("target", "release", "zellij");
const SERVER_FLAG = "--server";
const ZELLIJ_BASENAME = "zellij";

const TICK = "✔";
const CROSS = "✘";
const WARN = "▲";
const DOT = "·";

// ─── branded types ──────────────────────────────────────────────────────────

export type Pid = number & { readonly __brand: "Pid" };
export type Inode = number & { readonly __brand: "Inode" };

/** Narrows a raw number to a process id. */
export const asPid = (value: number): Pid => value as Pid;

/** Narrows a raw number to a filesystem inode number. */
export const asInode = (value: number): Inode => value as Inode;

export interface ZellijServer {
  readonly pid: Pid;
  readonly user: string;
  readonly session: string;
  readonly executable: string;
}

/** `current` = running the inode we just installed. `stale` = the install path
 *  moved underneath it, so it still executes the old image until restarted.
 *  `foreign` = a binary this install does not manage (another prefix, another
 *  user), which we can report but never fix. */
export type ServerStatus = "current" | "stale" | "foreign";

export interface ServerVerdict {
  readonly server: ZellijServer;
  readonly status: ServerStatus;
  readonly runningInode: Inode | null;
}

// ─── pure logic ─────────────────────────────────────────────────────────────

/**
 * Reports whether a zellij binary still discovers pane commands by shelling out
 * to `ps`, which is the pre-#5324 behaviour.
 *
 * @param binaryStrings The binary's contents decoded so that every byte maps to
 *   one character (`latin1`), or the output of `strings` over it.
 * @returns `true` if the `ps` argument vector is still embedded.
 */
export function shellsOutToPs(binaryStrings: string): boolean {
  return binaryStrings.includes(PS_DISCOVERY_MARKER);
}

/**
 * Extracts the running zellij *servers* from `ps -Ao pid=,user=,args=` output.
 *
 * Client processes (`zellij a <session>`) are deliberately excluded: they hold
 * no panes, spawn no discovery scans, and restarting one fixes nothing.
 *
 * @param psOutput Raw `ps` output, one process per line.
 * @returns One entry per server, in the order `ps` reported them.
 */
export function parseZellijServers(psOutput: string): ZellijServer[] {
  const servers: ZellijServer[] = [];
  for (const line of psOutput.split("\n")) {
    const trimmed = line.trim();
    if (trimmed === "") continue;

    const match = /^(\d+)\s+(\S+)\s+(.*)$/.exec(trimmed);
    if (match === null) continue;
    const [, rawPid, user, command] = match;
    if (rawPid === undefined || user === undefined || command === undefined) continue;

    const fields = command.split(/\s+/);
    const executable = fields[0];
    if (executable === undefined) continue;
    if (!executable.endsWith(ZELLIJ_BASENAME)) continue;

    const flagIndex = fields.indexOf(SERVER_FLAG);
    if (flagIndex === -1) continue;
    const socketPath = fields[flagIndex + 1];
    if (socketPath === undefined) continue;

    const session = socketPath.split("/").filter(Boolean).pop();
    if (session === undefined) continue;

    servers.push({ pid: asPid(Number(rawPid)), user, session, executable });
  }
  return servers;
}

/**
 * Decides, for each running server, whether it is executing the binary that was
 * just installed.
 *
 * A server holds its executable open by inode, so replacing the file on disk
 * leaves every already-running server on the old image indefinitely — the whole
 * reason this report exists.
 *
 * @param servers Servers discovered by {@link parseZellijServers}.
 * @param runningInodeByPid Inode each server actually has open, where readable.
 * @param installedInode Inode of the binary now at `installPath`.
 * @param installPath Destination this run installed to.
 * @returns One verdict per server, in the order given.
 */
export function classifyServers(
  servers: readonly ZellijServer[],
  runningInodeByPid: ReadonlyMap<Pid, Inode>,
  installedInode: Inode,
  installPath: string,
): ServerVerdict[] {
  return servers.map((server) => {
    const runningInode = runningInodeByPid.get(server.pid) ?? null;
    if (server.executable !== installPath) {
      return { server, status: "foreign" as const, runningInode };
    }
    const status = runningInode === installedInode ? "current" : "stale";
    return { server, status, runningInode };
  });
}

// ─── shell helpers ──────────────────────────────────────────────────────────

interface Run {
  readonly ok: boolean;
  readonly out: string;
}

const run = (cmd: string, cwd?: string): Run => {
  const r = spawnSync("sh", ["-c", cmd], { cwd, encoding: "utf8" });
  return { ok: r.status === 0, out: `${r.stdout ?? ""}${r.stderr ?? ""}`.trim() };
};

const stream = (cmd: string, cwd: string): boolean => {
  process.stderr.write(`\n  $ ${cmd}\n\n`);
  return spawnSync("sh", ["-c", cmd], { cwd, stdio: "inherit" }).status === 0;
};

const die = (message: string): never => {
  process.stderr.write(`\n${CROSS} ${message}\n`);
  process.exit(1);
};

/**
 * Reads the inode each process currently has open as its executable text.
 *
 * Processes owned by other users are silently absent: `lsof` cannot see them
 * without privileges, and they are classified `foreign` anyway.
 */
function readRunningInodes(pids: readonly Pid[]): Map<Pid, Inode> {
  const inodes = new Map<Pid, Inode>();
  if (pids.length === 0) return inodes;

  const { out } = run(`lsof -p ${pids.join(",")} -Fpfin 2>/dev/null`);
  let pid: Pid | null = null;
  let isText = false;
  let inode: Inode | null = null;

  for (const line of out.split("\n")) {
    const tag = line[0];
    const value = line.slice(1);
    if (tag === "p") {
      pid = asPid(Number(value));
      isText = false;
      inode = null;
    } else if (tag === "f") {
      isText = value === "txt";
      inode = null;
    } else if (tag === "i") {
      inode = asInode(Number(value));
    } else if (tag === "n") {
      if (pid !== null && isText && inode !== null && value.endsWith(ZELLIJ_BASENAME)) {
        inodes.set(pid, inode);
      }
    }
  }
  return inodes;
}

/** Gathers every running zellij server and decides which ones are stale. */
function serverReport(installedInode: Inode, installPath: string): ServerVerdict[] {
  const { out } = run("ps -Ao pid=,user=,args=");
  const servers = parseZellijServers(out);
  return classifyServers(servers, readRunningInodes(servers.map((s) => s.pid)), installedInode, installPath);
}

function printReport(verdicts: readonly ServerVerdict[], installPath: string): void {
  const stale = verdicts.filter((v) => v.status === "stale");
  const foreign = verdicts.filter((v) => v.status === "foreign");
  const current = verdicts.filter((v) => v.status === "current");

  process.stderr.write(`\n  running servers\n`);
  if (verdicts.length === 0) process.stderr.write(`    ${DOT} none\n`);
  for (const { server, status } of verdicts) {
    const mark = status === "current" ? TICK : status === "stale" ? WARN : DOT;
    process.stderr.write(
      `    ${mark} ${server.session.padEnd(16)} pid ${String(server.pid).padEnd(7)} ${status.padEnd(8)} ${
        status === "foreign" ? server.executable : ""
      }\n`,
    );
  }

  if (stale.length > 0) {
    process.stderr.write(
      `\n  ${WARN} ${stale.length} server(s) still run the previous ${installPath} image.\n` +
        `    They keep the old binary open by inode and will not pick this up until restarted:\n\n`,
    );
    for (const { server } of stale) {
      process.stderr.write(`      zellij kill-session ${server.session}   # then reattach\n`);
    }
  }
  if (foreign.length > 0) {
    process.stderr.write(
      `\n  ${DOT} ${foreign.length} server(s) run a binary this install does not manage.\n` +
        `    Check them separately — an old one here slows the whole machine down.\n`,
    );
  }
  if (stale.length === 0 && foreign.length === 0 && current.length > 0) {
    process.stderr.write(`\n  ${TICK} every running server is on the freshly installed binary.\n`);
  }
}

// ─── orchestration ──────────────────────────────────────────────────────────

interface Options {
  readonly repo: string;
  readonly dest: string;
  readonly ref: string;
  readonly fetch: boolean;
  readonly dryRun: boolean;
  readonly statusOnly: boolean;
  readonly force: boolean;
}

function parseArgs(argv: readonly string[]): Options {
  const flag = (name: string): boolean => argv.includes(name);
  const value = (name: string, fallback: string): string => {
    const i = argv.indexOf(name);
    return i === -1 ? fallback : (argv[i + 1] ?? fallback);
  };
  return {
    repo: value("--repo", DEFAULT_REPO),
    dest: value("--dest", DEFAULT_DEST),
    ref: value("--ref", DEFAULT_REF),
    fetch: !flag("--no-fetch"),
    dryRun: flag("--dry-run"),
    statusOnly: flag("--status"),
    force: flag("--force"),
  };
}

const USAGE = `zellij-install — build zellij from upstream and install it, only if the build carries the ps fix

  --repo <path>   zellij checkout             (default ${DEFAULT_REPO})
  --dest <path>   install destination         (default ${DEFAULT_DEST})
  --ref <ref>     ref to reset the repo to    (default ${DEFAULT_REF})
  --no-fetch      build what is checked out; do not fetch or reset
  --dry-run       report the plan, change nothing
  --status        skip build/install; only report running servers
  --force         allow discarding local commits not present in --ref
`;

function main(): void {
  const argv = process.argv.slice(2);
  if (argv.includes("--help") || argv.includes("-h")) {
    process.stdout.write(USAGE);
    return;
  }
  const opts = parseArgs(argv);

  if (opts.statusOnly) {
    const inode = existsSync(opts.dest) ? asInode(statSync(opts.dest).ino) : asInode(-1);
    printReport(serverReport(inode, opts.dest), opts.dest);
    return;
  }

  if (!existsSync(join(opts.repo, ".git"))) die(`not a git checkout: ${opts.repo}`);

  // A reset --hard is unrecoverable for uncommitted work, so refuse to run over
  // a dirty tree rather than deciding on the user's behalf what is disposable.
  if (opts.fetch) {
    const dirty = run("git status --porcelain --untracked-files=no", opts.repo).out;
    if (dirty !== "") {
      die(`${opts.repo} has uncommitted changes; commit or stash first:\n\n${dirty}`);
    }
  }

  process.stderr.write(`\n  repo   ${opts.repo}\n  dest   ${opts.dest}\n  ref    ${opts.ref}\n`);

  if (opts.fetch) {
    const remote = opts.ref.includes("/") ? opts.ref.split("/")[0] : "origin";
    if (opts.dryRun) {
      process.stderr.write(`\n  ${DOT} would fetch ${remote} and reset --hard to ${opts.ref}\n`);
    } else {
      if (!stream(`git fetch ${remote}`, opts.repo)) die(`git fetch ${remote} failed`);

      // Commits on the checked-out branch that the target ref does not contain
      // would be destroyed by the reset. That is almost always a mistake.
      const ahead = run(`git log --oneline ${opts.ref}..HEAD`, opts.repo).out;
      if (ahead !== "" && !opts.force) {
        die(
          `HEAD has commits not in ${opts.ref}; reset --hard would discard them.\n` +
            `Re-run with --force if that is intended:\n\n${ahead}`,
        );
      }
      if (!stream(`git reset --hard ${opts.ref}`, opts.repo)) die("git reset failed");
    }
  }

  const built = join(opts.repo, BUILT_BINARY);
  if (opts.dryRun) {
    process.stderr.write(`  ${DOT} would build, verify, and install to ${opts.dest}\n\n`);
    return;
  }

  if (!stream("cargo build --release", opts.repo)) die("cargo build --release failed");
  if (!existsSync(built)) die(`build reported success but ${built} is missing`);

  // The gate. latin1 maps every byte to one character, so an ASCII marker is
  // found exactly without UTF-8 decoding mangling the binary.
  if (shellsOutToPs(readFileSync(built, "latin1"))) {
    die(
      `refusing to install: the build at ${built} still shells out to \`ps\`.\n` +
        `  It embeds "${PS_DISCOVERY_MARKER}", so it predates zellij#5324 regardless of what\n` +
        `  \`--version\` reports. Check that ${opts.ref} really contains the fix.`,
    );
  }
  process.stderr.write(`\n${TICK} verified: no \`ps\` process-table scan in the build\n`);

  // Copying over the destination in place gets the new binary SIGKILLed on
  // macOS — the kernel caches the code signature against the vnode, and an
  // overwritten file fails validation on exec. Unlinking first gives a new one.
  rmSync(opts.dest, { force: true });
  copyFileSync(built, opts.dest);
  const installedInode = asInode(statSync(opts.dest).ino);
  process.stderr.write(`${TICK} installed ${opts.dest} (inode ${installedInode})\n`);

  printReport(serverReport(installedInode, opts.dest), opts.dest);
  process.stderr.write("\n");
}

// Only run the CLI when executed directly, so the tests can import the pure
// functions without the script trying to build anything.
if (process.argv[1]?.endsWith("zellij-install.ts")) main();
