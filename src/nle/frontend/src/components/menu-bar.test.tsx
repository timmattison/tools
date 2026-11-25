import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { MenuBar } from "./menu-bar";
import type { LoadedRecording } from "../bindings";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const SELECTED_PATH = "/tmp/demo.json";

const LOADED: LoadedRecording = {
  recording: {
    version: 1,
    width: 80,
    height: 24,
    timestamp: 1700000000,
    duration: 1.5,
    command: "echo hello",
    title: "Demo",
    env: {},
    events: [{ time: 0, type: "o", data: "hello\r\n" }],
  },
  metadata: {
    file_path: SELECTED_PATH,
    title: "Demo",
    command: "echo hello",
    duration: 1.5,
    event_count: 1,
    width: 80,
    height: 24,
    timestamp: 1700000000,
  },
};

const mockOpen = vi.mocked(open);
const mockInvoke = vi.mocked(invoke);

function renderMenuBar() {
  const onRecordingLoaded = vi.fn();
  const onError = vi.fn();
  render(<MenuBar onRecordingLoaded={onRecordingLoaded} onError={onError} />);
  fireEvent.click(screen.getByRole("button", { name: "Open Recording" }));
  return { onRecordingLoaded, onError };
}

describe("MenuBar open flow", () => {
  afterEach(cleanup);

  beforeEach(() => {
    mockOpen.mockReset();
    mockInvoke.mockReset();
  });

  it("loads the selected path and hands the result on", async () => {
    mockOpen.mockResolvedValue(SELECTED_PATH);
    mockInvoke.mockResolvedValue(LOADED);

    const { onRecordingLoaded, onError } = renderMenuBar();

    await waitFor(() => {
      expect(onRecordingLoaded).toHaveBeenCalledWith(LOADED);
    });
    expect(mockOpen).toHaveBeenCalledWith(
      expect.objectContaining({ multiple: false }),
    );
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith("load_recording", {
      path: SELECTED_PATH,
    });
    expect(onError).not.toHaveBeenCalled();
  });

  it("does nothing when the dialog is cancelled", async () => {
    mockOpen.mockResolvedValue(null);

    const { onRecordingLoaded, onError } = renderMenuBar();

    await waitFor(() => {
      expect(mockOpen).toHaveBeenCalled();
    });
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(mockInvoke).not.toHaveBeenCalled();
    expect(onRecordingLoaded).not.toHaveBeenCalled();
    expect(onError).not.toHaveBeenCalled();
  });

  it("reports an invoke that rejects with a string, as Tauri does", async () => {
    mockOpen.mockResolvedValue(SELECTED_PATH);
    mockInvoke.mockRejectedValue("File not found: /tmp/demo.json");

    const { onRecordingLoaded, onError } = renderMenuBar();

    await waitFor(() => {
      expect(onError).toHaveBeenCalledWith(
        "Failed to open recording: File not found: /tmp/demo.json",
      );
    });
    expect(onRecordingLoaded).not.toHaveBeenCalled();
  });

  it("reports an invoke that rejects with an Error", async () => {
    const error = new Error("disk on fire");
    mockOpen.mockResolvedValue(SELECTED_PATH);
    mockInvoke.mockRejectedValue(error);

    const { onRecordingLoaded, onError } = renderMenuBar();

    await waitFor(() => {
      expect(onError).toHaveBeenCalledWith(
        "Failed to open recording: " + error.message,
      );
    });
    expect(onRecordingLoaded).not.toHaveBeenCalled();
  });

  it("starts no second load while the first load runs", async () => {
    let finishLoad: (result: LoadedRecording) => void = () => undefined;
    mockOpen.mockResolvedValue(SELECTED_PATH);
    mockInvoke.mockReturnValue(
      new Promise<LoadedRecording>((resolve) => {
        finishLoad = resolve;
      }),
    );

    const { onRecordingLoaded } = renderMenuBar();
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledTimes(1);
    });
    const button = screen.getByRole("button", { name: "Open Recording" });
    expect(button).toBeDisabled();

    fireEvent.click(button);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(mockOpen).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledTimes(1);

    finishLoad(LOADED);
    await waitFor(() => {
      expect(button).toBeEnabled();
    });
    expect(onRecordingLoaded).toHaveBeenCalledTimes(1);
    expect(onRecordingLoaded).toHaveBeenCalledWith(LOADED);
  });

  it("lets the user open again after a load fails", async () => {
    mockOpen.mockResolvedValue(SELECTED_PATH);
    mockInvoke.mockRejectedValue("File not found: /tmp/demo.json");

    const { onError } = renderMenuBar();

    await waitFor(() => {
      expect(onError).toHaveBeenCalledTimes(1);
    });
    expect(
      screen.getByRole("button", { name: "Open Recording" }),
    ).toBeEnabled();
  });

  it("reports a dialog that rejects, and does not invoke", async () => {
    mockOpen.mockRejectedValue(new Error("dialog broke"));

    const { onRecordingLoaded, onError } = renderMenuBar();

    await waitFor(() => {
      expect(onError).toHaveBeenCalledWith(
        "Failed to open recording: dialog broke",
      );
    });
    expect(mockInvoke).not.toHaveBeenCalled();
    expect(onRecordingLoaded).not.toHaveBeenCalled();
  });
});
