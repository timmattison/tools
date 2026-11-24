import { describe, expect, it } from "vitest";
import { escapeHtml } from "./utils";

describe("escapeHtml", () => {
  it("escapes the markup characters of text context", () => {
    expect(escapeHtml("<b>Tom & Jerry</b>")).toBe("&lt;b&gt;Tom &amp; Jerry&lt;/b&gt;");
  });
});
