import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CacheStatus, Conversation, SearchResult } from "./bindings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const events = vi.hoisted(() => ({
  listener: null as null | ((event: { payload: CacheStatus }) => void),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name: string, handler: (event: { payload: CacheStatus }) => void) => {
    events.listener = handler;
    return () => {};
  }),
}));

function emitCacheStatus(status: CacheStatus): void {
  if (!events.listener) throw new Error("no listener for cache-status");
  events.listener({ payload: status });
}

const MARKUP = `
  <input type="text" id="global-search" />
  <div id="conversation-list"></div>
  <div id="error-overlay" class="hidden"></div>
`;

const DEBOUNCE_MS = 300;

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function conversation(): Conversation {
  return {
    chat_id: 1,
    chat_identifier: "chat-1",
    display_name: "Chat",
    is_group: false,
    participants: ["Alice"],
    last_message_date: null,
    last_message_preview: "",
    message_count: 1,
  };
}

function result(id: number, text: string): SearchResult {
  return {
    message: {
      message_id: id,
      text,
      sender: "Alice",
      is_from_me: false,
      date: null,
      date_read: null,
      is_audio: false,
      attachments: [],
      reply_to_guid: null,
      associated_emoji: null,
    },
    conversation: conversation(),
    context_before: [],
    context_after: [],
  };
}

function input(): HTMLInputElement {
  const el = document.getElementById("global-search");
  if (!(el instanceof HTMLInputElement)) throw new Error("no input in the test markup");
  return el;
}

function list(): HTMLElement {
  const el = document.getElementById("conversation-list");
  if (el === null) throw new Error("no list in the test markup");
  return el;
}

function type(text: string): void {
  input().value = text;
  input().dispatchEvent(new Event("input"));
}

function pressEscape(): void {
  input().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
}

async function flush(): Promise<void> {
  for (let i = 0; i < 8; i++) await Promise.resolve();
}

/** `invoke` answers `search_messages` from `pending` in call order and `list_conversations` with no items. */
async function mockInvoke(
  pending: Deferred<SearchResult[]>[],
  cacheStatus: CacheStatus = { state: "ready" },
) {
  const { invoke } = await import("@tauri-apps/api/core");
  const mock = vi.mocked(invoke);
  mock.mockReset();
  let next = 0;
  mock.mockImplementation((command: string) => {
    if (command === "list_conversations") return Promise.resolve([]);
    if (command === "get_cache_status") return Promise.resolve(cacheStatus);
    const reply = pending[next];
    next += 1;
    if (reply === undefined) return Promise.reject(`unexpected call ${next}`);
    return reply.promise;
  });
  return mock;
}

function searchCalls(invoke: Awaited<ReturnType<typeof mockInvoke>>): number {
  return invoke.mock.calls.filter(([command]) => command === "search_messages").length;
}

describe("global search", () => {
  beforeEach(async () => {
    vi.resetModules();
    events.listener = null;
    vi.useFakeTimers();
    document.body.innerHTML = MARKUP;
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("keeps the newer results when an older reply arrives late", async () => {
    const ab = deferred<SearchResult[]>();
    const abc = deferred<SearchResult[]>();
    await mockInvoke([ab, abc]);
    const { initSearch } = await import("./search");
    initSearch(() => {});

    type("ab");
    vi.advanceTimersByTime(DEBOUNCE_MS);
    type("abc");
    vi.advanceTimersByTime(DEBOUNCE_MS);

    abc.resolve([result(1, "result X")]);
    await flush();
    ab.resolve([result(2, "result Y")]);
    await flush();

    expect(list().textContent).toContain("result X");
    expect(list().textContent).not.toContain("result Y");
  });

  it("drops a reply that arrives after Escape", async () => {
    const ab = deferred<SearchResult[]>();
    const invoke = await mockInvoke([ab]);
    const { initSearch } = await import("./search");
    initSearch(() => {});

    type("ab");
    vi.advanceTimersByTime(DEBOUNCE_MS);
    pressEscape();
    ab.resolve([result(1, "late result")]);
    await flush();

    expect(searchCalls(invoke)).toBe(1);
    expect(list().dataset.mode).not.toBe("search");
    expect(list().textContent).not.toContain("late result");
  });

  it("does not search when Escape comes before the debounce ends", async () => {
    const invoke = await mockInvoke([]);
    const { initSearch } = await import("./search");
    initSearch(() => {});

    type("ab");
    pressEscape();
    vi.advanceTimersByTime(DEBOUNCE_MS);
    await flush();

    expect(searchCalls(invoke)).toBe(0);
  });

  it("waits for a building index, shows its progress, and searches when it is ready", async () => {
    const lunch = deferred<SearchResult[]>();
    const invoke = await mockInvoke([lunch], { state: "building", indexed: 1, total: 4 });
    const { initSearch } = await import("./search");
    const { initCacheStatus } = await import("./cache-status");
    initSearch(() => {});
    await initCacheStatus();

    type("lunch");
    vi.advanceTimersByTime(DEBOUNCE_MS);
    await flush();
    expect(searchCalls(invoke)).toBe(0);
    expect(list().textContent).toContain("Indexing messages for search: 25%");

    emitCacheStatus({ state: "building", indexed: 2, total: 4 });
    expect(list().textContent).toContain("Indexing messages for search: 50%");

    emitCacheStatus({ state: "ready" });
    await flush();
    expect(searchCalls(invoke)).toBe(1);
    lunch.resolve([result(1, "lunch at noon")]);
    await flush();
    expect(list().textContent).toContain("lunch at noon");
  });

  it("drops a search that waits for the index after Escape", async () => {
    const invoke = await mockInvoke([], { state: "building", indexed: 1, total: 4 });
    const { initSearch } = await import("./search");
    const { initCacheStatus } = await import("./cache-status");
    initSearch(() => {});
    await initCacheStatus();

    type("lunch");
    vi.advanceTimersByTime(DEBOUNCE_MS);
    await flush();
    pressEscape();
    emitCacheStatus({ state: "ready" });
    await flush();

    expect(searchCalls(invoke)).toBe(0);
    expect(list().dataset.mode).not.toBe("search");
  });
});
