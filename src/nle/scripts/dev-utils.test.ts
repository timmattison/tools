import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  FRONTEND_DIR,
  buildTauriDevArgs,
  buildViteArgs,
  parsePort,
  type Port,
} from "./dev-utils.ts";

/** A port that portplz could answer. */
const PORT = 34567 as Port;

describe("parsePort", () => {
  it("reads a port with a trailing newline", () => {
    assert.equal(parsePort("34567\n"), 34567);
  });

  it("reads the lowest port", () => {
    assert.equal(parsePort("1"), 1);
  });

  it("reads the highest port", () => {
    assert.equal(parsePort("65535"), 65535);
  });

  for (const bad of [
    "0",
    "65536",
    "123456",
    "",
    "not-a-port",
    "12a",
    "-1",
    "1e3",
  ]) {
    it(`refuses ${JSON.stringify(bad)}`, () => {
      assert.equal(parsePort(bad), null);
    });
  }
});

describe("buildViteArgs", () => {
  it("gives the port and strict mode on the command line", () => {
    assert.deepEqual(buildViteArgs(PORT), [
      "exec",
      "vite",
      "--port",
      "34567",
      "--strictPort",
    ]);
  });
});

describe("buildTauriDevArgs", () => {
  it("passes one JSON config that names the port for Tauri and for Vite", () => {
    const args = buildTauriDevArgs(PORT);
    assert.deepEqual(args.slice(0, 3), ["tauri", "dev", "--config"]);
    assert.equal(args.length, 4);
    assert.deepEqual(JSON.parse(args[3] ?? ""), {
      build: {
        devUrl: "http://localhost:34567",
        beforeDevCommand: {
          script: "pnpm exec vite --port 34567 --strictPort",
          cwd: FRONTEND_DIR,
        },
      },
    });
  });
});
