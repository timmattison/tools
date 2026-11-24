import { beforeEach, describe, expect, it, vi } from "vitest";
import type { InvokeArgs } from "@tauri-apps/api/core";
import type { Message } from "./bindings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));

const MARKUP = `
  <div id="empty-state"></div>
  <div id="message-view" class="hidden">
    <span id="chat-name"></span><span id="chat-identifier"></span>
    <button id="btn-export"></button>
    <div id="message-area"></div>
    <div id="export-bar" class="hidden">
      <span id="export-status"></span>
      <button id="btn-export-cancel"></button>
      <button id="btn-export-confirm" disabled></button>
    </div>
  </div>
`;

const PAGE_SIZE = 50;

/** One message on the given day (1 to 28 of January 2026), at the given hour. */
function message(id: number, day: number, hour: number, text: string): Message {
  const iso = new Date(Date.UTC(2026, 0, day, hour, 0, 0)).toISOString();
  return {
    message_id: id,
    text,
    sender: "Alice",
    is_from_me: false,
    date: iso,
    date_read: null,
    is_audio: false,
    attachments: [],
    reply_to_guid: null,
    associated_emoji: null,
  };
}

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve: (value: T) => void = () => {};
  let reject: (reason: unknown) => void = () => {};
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function area(): HTMLElement {
  const el = document.getElementById("message-area");
  if (el === null) throw new Error("no message area in the test markup");
  return el;
}

function texts(): string[] {
  return [...document.querySelectorAll(".message-text")].map((el) => el.textContent ?? "");
}

/** The `chatId` argument of an `invoke` call, or null. */
function chatIdOf(args: InvokeArgs | undefined): unknown {
  return args !== undefined && !Array.isArray(args) && !(args instanceof ArrayBuffer)
    && !(args instanceof Uint8Array) && "chatId" in args
    ? args.chatId
    : null;
}

/** Let the pending microtasks run. */
async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

describe("loadConversation", () => {
  beforeEach(() => {
    vi.resetModules();
    document.body.innerHTML = MARKUP;
  });

  it("drops the reply of a chat that is no longer open", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const first = deferred<Message[]>();
    const second = deferred<Message[]>();
    vi.mocked(invoke).mockImplementation((_command: string, args?: InvokeArgs) =>
      chatIdOf(args) === 1 ? first.promise : second.promise
    );
    const { loadConversation } = await import("./messages");

    const loadFirst = loadConversation(1, "One", "one", false);
    await settle();
    const loadSecond = loadConversation(2, "Two", "two", false);
    await settle();

    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "get_messages",
      expect.objectContaining({ chatId: 2, offset: 0 })
    );

    second.resolve([message(20, 2, 9, "from chat 2")]);
    await loadSecond;
    first.resolve([message(10, 1, 9, "from chat 1")]);
    await loadFirst;

    expect(texts()).toEqual(["from chat 2"]);
    expect(document.getElementById("chat-name")?.textContent).toBe("Two");
  });

  it("shows the error text when get_messages rejects", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockRejectedValue("Database error: locked");
    const { loadConversation } = await import("./messages");

    await expect(loadConversation(1, "One", "one", false)).resolves.toBeUndefined();

    const error = area().querySelector(".load-error");
    expect(error?.textContent).toContain("Database error: locked");
  });
});
