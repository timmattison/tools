import { describe, it, expect } from "vitest";
import { formatDuration } from "./format-duration";

describe("formatDuration", () => {
  it.each([
    [0, "0m 0.0s"],
    [59.96, "1m 0.0s"],
    [65.3, "1m 5.3s"],
    [3599.96, "1h 0m 0.0s"],
    [3725.5, "1h 2m 5.5s"],
  ])("formats %s seconds as %s", (seconds, expected) => {
    expect(formatDuration(seconds)).toBe(expected);
  });
});
