import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CacheStatus } from "./bindings";

const tauri = vi.hoisted(() => ({
  listener: null as null | ((event: { payload: CacheStatus }) => void),
  invoke: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: tauri.listen }));

import { initCacheStatus } from "./cache-status";

const READY_PLACEHOLDER = "Search messages...";

function placeholder(): string {
  return (document.getElementById("global-search") as HTMLInputElement).placeholder;
}

function emit(status: CacheStatus): void {
  if (!tauri.listener) throw new Error("no listener for cache-status");
  tauri.listener({ payload: status });
}

describe("initCacheStatus", () => {
  beforeEach(() => {
    document.body.innerHTML = `<input id="global-search" placeholder="${READY_PLACEHOLDER}">`;
    tauri.listener = null;
    tauri.invoke.mockReset();
    tauri.invoke.mockResolvedValue({ state: "not_started" });
    tauri.listen.mockReset();
    tauri.listen.mockImplementation(async (name: string, handler: typeof tauri.listener) => {
      expect(name).toBe("cache-status");
      tauri.listener = handler;
      return () => {};
    });
  });

  it("shows the percent of a building index", async () => {
    await initCacheStatus();
    emit({ state: "building", indexed: 50, total: 200 });
    expect(placeholder()).toBe("Indexing messages for search: 25%");
  });

  it("shows 0 percent when the total is 0", async () => {
    await initCacheStatus();
    emit({ state: "building", indexed: 0, total: 0 });
    expect(placeholder()).toBe("Indexing messages for search: 0%");
  });

  it("restores the placeholder when the index is ready", async () => {
    await initCacheStatus();
    emit({ state: "building", indexed: 50, total: 200 });
    emit({ state: "ready" });
    expect(placeholder()).toBe(READY_PLACEHOLDER);
  });

  it("shows the error of a failed build", async () => {
    await initCacheStatus();
    emit({ state: "failed", error: "disk full" });
    expect(placeholder()).toBe("Search unavailable: disk full");
  });

  it("applies the status that get_cache_status gives when no event comes", async () => {
    tauri.invoke.mockResolvedValue({ state: "building", indexed: 1, total: 4 });
    await initCacheStatus();
    expect(tauri.invoke).toHaveBeenCalledWith("get_cache_status");
    expect(placeholder()).toBe("Indexing messages for search: 25%");
  });
});
