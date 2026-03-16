import { render, screen } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import { PropertiesPanel } from "./properties-panel";
import type { RecordingMetadata } from "../bindings";

const TEST_METADATA: RecordingMetadata = {
  file_path: "/home/user/recordings/demo.json",
  title: "Demo Recording",
  command: "echo hello",
  duration: 65.3,
  event_count: 42,
  width: 120,
  height: 40,
  timestamp: 1700000000,
};

describe("PropertiesPanel", () => {
  it("shows placeholder when no metadata", () => {
    render(<PropertiesPanel metadata={null} />);
    expect(screen.getByText("No recording loaded")).toBeInTheDocument();
  });

  it("displays recording metadata", () => {
    render(<PropertiesPanel metadata={TEST_METADATA} />);
    expect(screen.getByText("Demo Recording")).toBeInTheDocument();
    expect(screen.getByText("echo hello")).toBeInTheDocument();
    expect(screen.getByText("65.3s")).toBeInTheDocument();
    expect(screen.getByText("42")).toBeInTheDocument();
    expect(screen.getByText("120x40")).toBeInTheDocument();
  });

  it("shows (untitled) for empty title", () => {
    render(<PropertiesPanel metadata={{ ...TEST_METADATA, title: "" }} />);
    expect(screen.getByText("(untitled)")).toBeInTheDocument();
  });

  it("shows only filename, not full path", () => {
    const { container } = render(
      <PropertiesPanel metadata={TEST_METADATA} />,
    );
    const fileValue = container.querySelector(
      '.metadata-item:last-child .metadata-value',
    );
    expect(fileValue).toHaveTextContent("demo.json");
    expect(fileValue).toHaveAttribute(
      "title",
      "/home/user/recordings/demo.json",
    );
  });

  it("handles Windows-style paths", () => {
    const { container } = render(
      <PropertiesPanel
        metadata={{
          ...TEST_METADATA,
          file_path: "C:\\Users\\test\\recordings\\demo.json",
        }}
      />,
    );
    const fileValue = container.querySelector(
      '.metadata-item:last-child .metadata-value',
    );
    expect(fileValue).toHaveTextContent("demo.json");
  });
});
