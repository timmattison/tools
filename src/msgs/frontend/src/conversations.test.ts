import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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

// The module adds its keydown handler to the document, and a reset of the
// modules does not remove it. Take each handler out after its test.
const added: EventListenerOrEventListenerObject[] = [];

beforeEach(() => {
  const original = document.addEventListener.bind(document);
  vi.spyOn(document, "addEventListener").mockImplementation((type, listener, options) => {
    if (type === "keydown" && listener !== null) added.push(listener);
    original(type, listener, options);
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  for (const listener of added.splice(0)) document.removeEventListener("keydown", listener);
});

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

  it("drops the late reply of a page that a search made stale", async () => {
    const invoke = await mockInvoke();
    const oldPage = deferred<Conversation[]>();
    invoke.mockResolvedValueOnce(page(1, PAGE_SIZE));
    const { initConversations } = await import("./conversations");
    await initConversations();

    invoke.mockReturnValueOnce(oldPage.promise);
    const el = list();
    scrollToBottom(el);
    el.dispatchEvent(new Event("scroll"));
    await flush();
    expect(invoke).toHaveBeenLastCalledWith("list_conversations", {
      offset: PAGE_SIZE,
      limit: PAGE_SIZE,
    });

    // The search module paints its results while the page is in flight.
    el.dataset.mode = "search";
    el.innerHTML = '<div class="search-result">hit</div>';
    oldPage.resolve(page(51, PAGE_SIZE));
    await flush();

    expect(document.querySelectorAll(".conversation-item")).toHaveLength(0);
    expect(document.querySelectorAll(".search-result")).toHaveLength(1);
  });

  it("drops the late reply of a page that a reset made stale", async () => {
    const invoke = await mockInvoke();
    const first = page(1, PAGE_SIZE);
    const oldPage = deferred<Conversation[]>();
    const newPage = deferred<Conversation[]>();
    invoke.mockResolvedValueOnce(first);
    const { initConversations } = await import("./conversations");
    await initConversations();

    invoke.mockReturnValueOnce(oldPage.promise);
    const el = list();
    scrollToBottom(el);
    el.dispatchEvent(new Event("scroll"));
    await flush();
    expect(invoke).toHaveBeenLastCalledWith("list_conversations", {
      offset: PAGE_SIZE,
      limit: PAGE_SIZE,
    });

    invoke.mockReturnValueOnce(newPage.promise);
    const reset = initConversations();
    await flush();
    expect(invoke).toHaveBeenLastCalledWith("list_conversations", {
      offset: 0,
      limit: PAGE_SIZE,
    });

    newPage.resolve(page(101, 3));
    await reset;
    oldPage.resolve(page(51, PAGE_SIZE));
    await flush();

    expect(itemIds()).toEqual(["101", "102", "103"]);
  });
});

describe("conversation keyboard handler", () => {
  beforeEach(() => {
    vi.resetModules();
    document.body.innerHTML = MARKUP;
  });

  /** Load three conversations and make the first one active. Returns the select callback. */
  async function setUp() {
    const invoke = await mockInvoke();
    invoke.mockResolvedValue(page(1, 3));
    const { initConversations, onConversationSelect } = await import("./conversations");
    const onSelect = vi.fn();
    onConversationSelect(onSelect);
    await initConversations();
    document.querySelector<HTMLElement>(".conversation-item")?.click();
    onSelect.mockClear();
    return onSelect;
  }

  function activeId(): string | undefined {
    return document.querySelector<HTMLElement>(".conversation-item.active")?.dataset.chatId;
  }

  function press(target: EventTarget, key: string): KeyboardEvent {
    const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
    target.dispatchEvent(event);
    return event;
  }

  it("ignores Enter on a focused button", async () => {
    const onSelect = await setUp();
    const button = document.getElementById("btn-export");
    if (button === null) throw new Error("no button in the test markup");
    button.focus();

    press(button, "Enter");

    expect(onSelect).not.toHaveBeenCalled();
  });

  it("leaves the arrow keys of the message area to the browser", async () => {
    await setUp();
    const area = document.getElementById("message-area");
    if (area === null) throw new Error("no message area in the test markup");

    const event = press(area, "ArrowDown");

    expect(event.defaultPrevented).toBe(false);
    expect(activeId()).toBe("1");
  });

  it("still moves the selection on ArrowDown from the document body", async () => {
    await setUp();

    const event = press(document.body, "ArrowDown");

    expect(event.defaultPrevented).toBe(true);
    expect(activeId()).toBe("2");
  });
});
