import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CacheStatus } from "./bindings";

const CACHE_STATUS_EVENT = "cache-status";
const SEARCH_INPUT_ID = "global-search";
const READY_PLACEHOLDER = "Search messages...";
const PERCENT_SCALE = 100;

/** The placeholder text that shows the state of the search index. */
function placeholderFor(status: CacheStatus): string {
  switch (status.state) {
    case "building": {
      const percent =
        status.total > 0 ? Math.floor((status.indexed * PERCENT_SCALE) / status.total) : 0;
      return `Indexing messages for search: ${percent}%`;
    }
    case "failed":
      return `Search unavailable: ${status.error}`;
    case "ready":
    case "not_started":
      return READY_PLACEHOLDER;
  }
}

function apply(status: CacheStatus): void {
  const input = document.getElementById(SEARCH_INPUT_ID);
  if (input instanceof HTMLInputElement) input.placeholder = placeholderFor(status);
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
