// @vitest-environment node
import { describe, expect, it } from "vitest";
import { parsePort } from "./dev-utils.ts";

describe("parsePort", () => {
  it("reads a port followed by a newline", () => {
    expect(parsePort("4123\n")).toBe(4123);
  });

  it.each(["", "abc", "0", "70000", "12 34", "-5"])("rejects %j", (output) => {
    expect(parsePort(output)).toBeNull();
  });
});
