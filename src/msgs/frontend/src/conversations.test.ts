import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Conversation } from "./bindings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const MARKUP = `
  <input type="text" id="global-search" />
  <div id="conversation-list"></div>
  <div id="message-area" tabindex="0"></div>
  <button id="btn-export"></button>
`;

const PAGE_SIZE = 50;

function conversation(id: number): Conversation {
  return {
    chat_id: id,
    chat_identifier: `chat-${id}`,
    display_name: `Chat ${id}`,
    is_group: false,
    participants: [`person-${id}`],
    last_message_date: null,
    last_message_preview: `preview ${id}`,
    message_count: 1,
  };
}

/** `count` conversations with ids from `first`. */
function page(first: number, count: number): Conversation[] {
  return Array.from({ length: count }, (_, i) => conversation(first + i));
}

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

function list(): HTMLElement {
  const el = document.getElementById("conversation-list");
  if (el === null) throw new Error("no list in the test markup");
  return el;
}

/** Make the list look scrolled to its bottom. */
function scrollToBottom(el: HTMLElement): void {
  Object.defineProperty(el, "scrollTop", { value: 900, configurable: true });
  Object.defineProperty(el, "clientHeight", { value: 100, configurable: true });
  Object.defineProperty(el, "scrollHeight", { value: 1000, configurable: true });
}

function itemIds(): string[] {
  return Array.from(document.querySelectorAll<HTMLElement>(".conversation-item")).map(
    (el) => el.dataset.chatId ?? "",
  );
}

async function flush(): Promise<void> {
  for (let i = 0; i < 6; i++) await Promise.resolve();
}

async function mockInvoke() {
  const { invoke } = await import("@tauri-apps/api/core");
  const mock = vi.mocked(invoke);
  mock.mockReset();
  return mock;
}

describe("conversation list scrolling", () => {
  beforeEach(() => {
    vi.resetModules();
    document.body.innerHTML = MARKUP;
  });

  it("does not load more conversations while the list shows search results", async () => {
    const invoke = await mockInvoke();
    invoke.mockResolvedValue(page(1, PAGE_SIZE));
    const { initConversations } = await import("./conversations");
    await initConversations();
    expect(invoke).toHaveBeenCalledTimes(1);

    const el = list();
    el.dataset.mode = "search";
    el.innerHTML = '<div class="search-result">hit</div>';
    scrollToBottom(el);
    el.dispatchEvent(new Event("scroll"));
    await flush();

    expect(invoke).toHaveBeenCalledTimes(1);
    expect(document.querySelectorAll(".conversation-item")).toHaveLength(0);
  });
});
