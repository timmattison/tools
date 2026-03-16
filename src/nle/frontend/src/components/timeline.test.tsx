import { render, screen } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import { Timeline } from "./timeline";
import type { Recording } from "../bindings";

const TEST_RECORDING: Recording = {
  version: 1,
  width: 80,
  height: 24,
  timestamp: 1700000000,
  duration: 125.5,
  command: "echo hello",
  title: "Test",
  env: {},
  events: [
    { time: 0, type: "o", data: "hello\r\n" },
    { time: 1.5, type: "i", data: "q" },
  ],
};

describe("Timeline", () => {
  it("shows placeholder when no recording", () => {
    render(<Timeline recording={null} />);
    expect(screen.getByText("No recording loaded")).toBeInTheDocument();
  });

  it("displays event count and duration", () => {
    render(<Timeline recording={TEST_RECORDING} />);
    expect(screen.getByText(/2 events/)).toBeInTheDocument();
    expect(screen.getByText(/2m 5\.5s/)).toBeInTheDocument();
  });
});
