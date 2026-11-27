import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { buildDxServeArgs, parsePort, type Port } from "./dev-utils.ts";

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

describe("buildDxServeArgs", () => {
  it("gives the desktop target and the port on the command line", () => {
    assert.deepEqual(buildDxServeArgs(PORT), [
      "serve",
      "--desktop",
      "--port",
      "34567",
    ]);
  });
});
