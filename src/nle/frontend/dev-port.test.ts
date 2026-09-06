import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { devServerPort } from "./dev-port";

/** The name of the variable `run-nle.sh` sets from `portplz`. */
const PORT_VARIABLE = "NLE_DEV_PORT";

describe("devServerPort", () => {
  let saved: string | undefined;

  beforeEach(() => {
    saved = process.env[PORT_VARIABLE];
    Reflect.deleteProperty(process.env, PORT_VARIABLE);
  });

  afterEach(() => {
    if (saved === undefined) {
      Reflect.deleteProperty(process.env, PORT_VARIABLE);
    } else {
      process.env[PORT_VARIABLE] = saved;
    }
  });

  it("falls back to 1420 when the variable is absent", () => {
    expect(devServerPort()).toBe(1420);
  });

  it("uses the port that run-nle.sh took from portplz", () => {
    process.env[PORT_VARIABLE] = "2621";
    expect(devServerPort()).toBe(2621);
  });

  it("rejects a value that is not a number", () => {
    process.env[PORT_VARIABLE] = "not-a-port";
    expect(() => devServerPort()).toThrow(PORT_VARIABLE);
  });

  it("rejects a port outside the range a socket can bind", () => {
    process.env[PORT_VARIABLE] = "70000";
    expect(() => devServerPort()).toThrow(PORT_VARIABLE);
  });

  it("rejects an empty value", () => {
    process.env[PORT_VARIABLE] = "";
    expect(() => devServerPort()).toThrow(PORT_VARIABLE);
  });
});
