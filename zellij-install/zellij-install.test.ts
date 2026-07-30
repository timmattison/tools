import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  asInode,
  asPid,
  classifyServers,
  parseZellijServers,
  shellsOutToPs,
} from "./zellij-install.ts";

// Real `strings` output fragments. A pre-#5324 zellij embeds the argument
// vector it hands to `ps`; a fixed one has no `ps` call left at all.
const PRE_FIX_STRINGS = `
zellij-server/src/os_input_output.rs
ppid,args
-ao
failed to spawn
`;

const POST_FIX_STRINGS = `
zellij-server/src/os_input_output.rs
sysinfo-0.37.2/src/unix/apple/macos/process.rs
failed to spawn
`;

const INSTALL_PATH = "/Users/tim/.local/bin/zellij";

describe("shellsOutToPs", () => {
  it("flags a binary that still embeds the ps argument vector", () => {
    assert.equal(shellsOutToPs(PRE_FIX_STRINGS), true);
  });

  it("clears a binary with no ps call left", () => {
    assert.equal(shellsOutToPs(POST_FIX_STRINGS), false);
  });

  it("flags the fast -Ao variant too, since it is still a process-table scan", () => {
    assert.equal(shellsOutToPs("-Ao\nppid,args\n"), true);
  });
});

describe("parseZellijServers", () => {
  const PS_OUTPUT = [
    "11128 timmattison /Users/tim/.local/bin/zellij --server /var/folders/m8/T/zellij-501/contract_version_1/property-tax",
    "56531 timmattison /opt/homebrew/Cellar/zellij/0.44.3/bin/zellij --server /var/folders/m8/T/zellij-501/contract_version_1/muxiavelli",
    "82030 scyloswork /Users/scyloswork/.local/bin/zellij --server /var/folders/zd/T/zellij-503/contract_version_1/scylos",
    "94038 timmattison zellij -s tools",
    "82027 scyloswork zellij a scylos",
    "  512 root /usr/sbin/syslogd",
  ].join("\n");

  it("extracts pid, user, executable and session name for each server", () => {
    const servers = parseZellijServers(PS_OUTPUT);
    assert.deepEqual(
      servers.map((s) => [s.pid, s.user, s.session, s.executable]),
      [
        [11128, "timmattison", "property-tax", "/Users/tim/.local/bin/zellij"],
        [
          56531,
          "timmattison",
          "muxiavelli",
          "/opt/homebrew/Cellar/zellij/0.44.3/bin/zellij",
        ],
        [82030, "scyloswork", "scylos", "/Users/scyloswork/.local/bin/zellij"],
      ],
    );
  });

  it("ignores zellij client processes, which hold no session state", () => {
    const sessions = parseZellijServers(PS_OUTPUT).map((s) => s.session);
    assert.equal(sessions.includes("tools"), false);
  });

  it("ignores unrelated processes", () => {
    assert.deepEqual(parseZellijServers("512 root /usr/sbin/syslogd"), []);
  });

  it("returns nothing for empty input", () => {
    assert.deepEqual(parseZellijServers("   \n\n"), []);
  });
});

describe("classifyServers", () => {
  const server = (pid: number, executable: string) => ({
    pid: asPid(pid),
    user: "timmattison",
    session: `session-${pid}`,
    executable,
  });

  const INSTALLED = asInode(1185642437);

  it("calls a server current when it runs the inode we just installed", () => {
    const verdicts = classifyServers(
      [server(94042, INSTALL_PATH)],
      new Map([[asPid(94042), INSTALLED]]),
      INSTALLED,
      INSTALL_PATH,
    );
    assert.equal(verdicts[0]?.status, "current");
  });

  it("calls a server stale when it holds an older inode at the install path", () => {
    const verdicts = classifyServers(
      [server(11128, INSTALL_PATH)],
      new Map([[asPid(11128), asInode(1185087873)]]),
      INSTALLED,
      INSTALL_PATH,
    );
    assert.equal(verdicts[0]?.status, "stale");
  });

  it("calls a server stale when its inode cannot be read at the install path", () => {
    const verdicts = classifyServers(
      [server(11128, INSTALL_PATH)],
      new Map(),
      INSTALLED,
      INSTALL_PATH,
    );
    assert.equal(verdicts[0]?.status, "stale");
  });

  it("calls a server foreign when it runs a binary this install does not manage", () => {
    const verdicts = classifyServers(
      [
        server(56531, "/opt/homebrew/Cellar/zellij/0.44.3/bin/zellij"),
        server(82030, "/Users/scyloswork/.local/bin/zellij"),
      ],
      new Map(),
      INSTALLED,
      INSTALL_PATH,
    );
    assert.deepEqual(
      verdicts.map((v) => v.status),
      ["foreign", "foreign"],
    );
  });

  it("preserves the running inode so the report can show what changed", () => {
    const verdicts = classifyServers(
      [server(11128, INSTALL_PATH)],
      new Map([[asPid(11128), asInode(1185087873)]]),
      INSTALLED,
      INSTALL_PATH,
    );
    assert.equal(verdicts[0]?.runningInode, 1185087873);
  });
});
