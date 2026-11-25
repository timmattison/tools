import { invoke } from "@tauri-apps/api/core";
import type { CacheStatus, SearchResult } from "./bindings";
import { currentCacheStatus, indexingProgress, onCacheStatus } from "./cache-status";
import { initConversations } from "./conversations";
import { byId, escapeHtml, messageOf, showError, UNKNOWN_DATE_LABEL } from "./utils";

let searchTimeout: number | null = null;
// Numbers each search. A reply that is not for the latest number is dropped.
let searchSequence = 0;
// The search that waits for the build of the index, and the number of that search.
let waiting: { query: string; sequence: number } | null = null;
let onNavigateCallback: ((result: SearchResult) => void) | null = null;

export function initSearch(onNavigate: (result: SearchResult) => void): void {
  onNavigateCallback = onNavigate;
  onCacheStatus(resumeWaitingSearch);

  const input = byId("global-search", HTMLInputElement);
  input.addEventListener("input", () => {
    if (searchTimeout) clearTimeout(searchTimeout);
    searchTimeout = window.setTimeout(() => performSearch(input.value), 300);
  });

  input.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      if (searchTimeout) clearTimeout(searchTimeout);
      searchTimeout = null;
      searchSequence += 1;
      waiting = null;
      input.value = "";
      hideSearchResults();
    }
  });
}

async function performSearch(query: string): Promise<void> {
  searchSequence += 1;
  const sequence = searchSequence;
  waiting = null;
  if (query.length < 2) {
    hideSearchResults();
    return;
  }

  // The backend refuses a search while the index builds. Wait for the build.
  const status = currentCacheStatus();
  if (status.state === "building") {
    waiting = { query, sequence };
    showIndexWait(query, status);
    return;
  }
  await runSearch(query, sequence);
}

/** Continue the search that waits for the index, when the index state changes. */
function resumeWaitingSearch(status: CacheStatus): void {
  if (waiting === null) return;
  if (waiting.sequence !== searchSequence) {
    waiting = null;
    return;
  }
  if (status.state === "building") {
    showIndexWait(waiting.query, status);
    return;
  }
  const { query, sequence } = waiting;
  waiting = null;
  void runSearch(query, sequence);
}

async function runSearch(query: string, sequence: number): Promise<void> {
  showSearchLoading(query);

  try {
    const results = await invoke<SearchResult[]>("search_messages", {
      query,
      chatId: null,
      offset: 0,
      limit: 50,
    });

    if (sequence !== searchSequence) return;
    showSearchResults(results);
  } catch (e) {
    if (sequence !== searchSequence) return;
    showSearchError(String(e));
  }
}

function showIndexWait(query: string, status: Extract<CacheStatus, { state: "building" }>): void {
  const list = byId("conversation-list", HTMLElement);
  list.dataset.mode = "search";
  list.innerHTML = `<div class="search-loading"><div class="search-spinner"></div>${escapeHtml(indexingProgress(status))}. The search for "${escapeHtml(query)}" starts when the index is ready.</div>`;
}

function showSearchLoading(query: string): void {
  const list = byId("conversation-list", HTMLElement);
  list.dataset.mode = "search";
  list.innerHTML = `<div class="search-loading"><div class="search-spinner"></div>Searching for "${escapeHtml(query)}"...</div>`;
}

function showSearchError(message: string): void {
  const list = byId("conversation-list", HTMLElement);
  list.dataset.mode = "search";
  list.innerHTML = `<div class="search-empty">Search failed: ${escapeHtml(message)}</div>`;
}

function showSearchResults(results: SearchResult[]): void {
  const list = byId("conversation-list", HTMLElement);
  // Replace conversation list with search results temporarily
  list.dataset.mode = "search";

  list.innerHTML = "";
  if (results.length === 0) {
    list.innerHTML = '<div class="search-empty">No results found</div>';
    return;
  }

  for (const result of results) {
    const el = document.createElement("div");
    el.className = "search-result";

    const convName =
      result.conversation.display_name ??
      result.conversation.participants[0] ??
      result.conversation.chat_identifier;

    el.innerHTML = `
      <div class="search-result-header">${escapeHtml(convName)}</div>
      <div class="search-result-sender">${escapeHtml(result.message.sender)}</div>
      <div class="search-result-text">${escapeHtml(result.message.text ?? "")}</div>
      <div class="search-result-time">${escapeHtml(result.message.date === null ? UNKNOWN_DATE_LABEL : new Date(result.message.date).toLocaleDateString())}</div>
    `;

    el.addEventListener("click", () => {
      onNavigateCallback?.(result);
    });

    list.appendChild(el);
  }
}

function hideSearchResults(): void {
  const list = byId("conversation-list", HTMLElement);
  if (list.dataset.mode === "search") {
    list.dataset.mode = "";
    // Re-initialize conversation list without losing app state
    list.innerHTML = "";
    initConversations().catch((e: unknown) => showError(messageOf(e)));
  }
}
