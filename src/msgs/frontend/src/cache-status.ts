import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CacheStatus } from "./bindings";

const CACHE_STATUS_EVENT = "cache-status";
const SEARCH_INPUT_ID = "global-search";
const READY_PLACEHOLDER = "Search messages...";
const PERCENT_SCALE = 100;

let latest: CacheStatus = { state: "not_started" };
const listeners: ((status: CacheStatus) => void)[] = [];

/** The line that tells how far the build of the search index has come. */
export function indexingProgress(status: Extract<CacheStatus, { state: "building" }>): string {
  const percent =
    status.total > 0 ? Math.floor((status.indexed * PERCENT_SCALE) / status.total) : 0;
  return `Indexing messages for search: ${percent}%`;
}

/** The latest state of the search index that this window has seen. */
export function currentCacheStatus(): CacheStatus {
  return latest;
}

/** Call `listener` with each new state of the search index. */
export function onCacheStatus(listener: (status: CacheStatus) => void): void {
  listeners.push(listener);
}

/** The placeholder text that shows the state of the search index. */
function placeholderFor(status: CacheStatus): string {
  switch (status.state) {
    case "building":
      return indexingProgress(status);
    case "failed":
      return `Search unavailable: ${status.error}`;
    case "ready":
    case "not_started":
      return READY_PLACEHOLDER;
  }
}

function apply(status: CacheStatus): void {
  latest = status;
  const input = document.getElementById(SEARCH_INPUT_ID);
  if (input instanceof HTMLInputElement) input.placeholder = placeholderFor(status);
  for (const listener of listeners) listener(status);
}

/**
 * Show the state of the search index in the placeholder of the search box.
 * Subscribes to the `cache-status` events first, then applies the status that
 * the backend holds, so a change between the two steps is not lost. An event
 * is newer than the answer of the backend, so the answer does not replace it.
 */
export async function initCacheStatus(): Promise<void> {
  let eventSeen = false;
  await listen<CacheStatus>(CACHE_STATUS_EVENT, (event) => {
    eventSeen = true;
    apply(event.payload);
  });
  const status = await invoke<CacheStatus>("get_cache_status");
  if (!eventSeen) apply(status);
}
