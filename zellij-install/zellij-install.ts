#!/usr/bin/env -S npx tsx
// zellij-install — refresh the zellij checkout from upstream, build it, prove the
// build actually carries the process-table fix, install it, and report which
// running servers are still executing the old image.
//
// The verification gate is the point. Upstream removed zellij's `ps` shell-out in
// #5324, but every build since v0.44.3 still reports version 0.45.0 — so
// `zellij --version` cannot tell a fixed binary from an unfixed one. Only the
// absence of the embedded `ps` argument vector can.

export type Pid = number & { readonly __brand: "Pid" };
export type Inode = number & { readonly __brand: "Inode" };

export const asPid = (value: number): Pid => value as Pid;
export const asInode = (value: number): Inode => value as Inode;

export interface ZellijServer {
  readonly pid: Pid;
  readonly user: string;
  readonly session: string;
  readonly executable: string;
}

/** `current` = running what we just installed. `stale` = the install path moved
 *  underneath it. `foreign` = a binary this install does not manage. */
export type ServerStatus = "current" | "stale" | "foreign";

export interface ServerVerdict {
  readonly server: ZellijServer;
  readonly status: ServerStatus;
  readonly runningInode: Inode | null;
}

export function shellsOutToPs(_binaryStrings: string): boolean {
  return false;
}

export function parseZellijServers(_psOutput: string): ZellijServer[] {
  return [];
}

export function classifyServers(
  _servers: readonly ZellijServer[],
  _runningInodeByPid: ReadonlyMap<Pid, Inode>,
  _installedInode: Inode,
  _installPath: string,
): ServerVerdict[] {
  return [];
}
