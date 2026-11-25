import { beforeEach, describe, expect, it } from "vitest";
import { byId, escapeHtml, showError } from "./utils";

describe("escapeHtml", () => {
  it("escapes the markup characters of text context", () => {
    expect(escapeHtml("<b>Tom & Jerry</b>")).toBe("&lt;b&gt;Tom &amp; Jerry&lt;/b&gt;");
  });

  it("escapes both quote characters, so the output is safe in an attribute", () => {
    expect(escapeHtml(`a"b'c`)).toBe("a&quot;b&#39;c");
  });
});

describe("byId", () => {
  beforeEach(() => {
    document.body.innerHTML = `<div id="x"></div><input id="field">`;
  });

  it("returns the element when it has the requested type", () => {
    expect(byId("field", HTMLInputElement)).toBe(document.getElementById("field"));
  });

  it("names the element in the error when it has another type", () => {
    expect(() => byId("x", HTMLInputElement)).toThrow(/#x/);
  });

  it("names the element in the error when it is missing", () => {
    expect(() => byId("nope", HTMLInputElement)).toThrow(/#nope/);
  });
});

describe("showError", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("makes the overlay when the document has none", () => {
    expect(() => showError("boom")).not.toThrow();
    const overlay = document.getElementById("error-overlay");
    expect(overlay?.textContent).toBe("boom");
    expect(overlay?.classList.contains("hidden")).toBe(false);
  });

  it("shows the message in the existing overlay", () => {
    document.body.innerHTML = `<div id="error-overlay" class="hidden"></div>`;
    showError("<b>bad</b>");
    const overlay = document.getElementById("error-overlay");
    expect(overlay?.textContent).toBe("<b>bad</b>");
    expect(overlay?.classList.contains("hidden")).toBe(false);
  });
});
