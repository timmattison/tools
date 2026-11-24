import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Conversation, SearchResult } from "./bindings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

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
async function mockInvoke(pending: Deferred<SearchResult[]>[]) {
  const { invoke } = await import("@tauri-apps/api/core");
  const mock = vi.mocked(invoke);
  mock.mockReset();
  let next = 0;
  mock.mockImplementation((command: string) => {
    if (command === "list_conversations") return Promise.resolve([]);
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
});
