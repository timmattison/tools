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
function message(id: number, day: number, hour: number, text: string, minute = 0): Message {
  const iso = new Date(Date.UTC(2026, 0, day, hour, minute, 0)).toISOString();
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

/** The `offset` argument of an `invoke` call, or null. */
function offsetOf(args: InvokeArgs | undefined): unknown {
  return args !== undefined && !Array.isArray(args) && !(args instanceof ArrayBuffer)
    && !(args instanceof Uint8Array) && "offset" in args
    ? args.offset
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

function byTestId(id: string): HTMLElement {
  const el = document.getElementById(id);
  if (el === null) throw new Error(`no #${id} in the test markup`);
  return el;
}

function messageEls(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>(".message")];
}

describe("export selection", () => {
  beforeEach(() => {
    vi.resetModules();
    document.body.innerHTML = MARKUP;
  });

  it("is reset when another chat opens", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const { save } = await import("@tauri-apps/plugin-dialog");
    vi.mocked(save).mockResolvedValue("/tmp/msgs-test-export");
    vi.mocked(invoke).mockImplementation((_command: string, args?: InvokeArgs) =>
      Promise.resolve(
        chatIdOf(args) === 1
          ? [message(11, 1, 10, "b"), message(10, 1, 9, "a")]
          : [message(21, 2, 10, "d"), message(20, 2, 9, "c")]
      )
    );
    const { loadConversation } = await import("./messages");

    await loadConversation(1, "One", "one", false);
    byTestId("btn-export").click();
    const [first, second] = messageEls();
    first?.click();
    second?.click();
    expect(byTestId("btn-export-confirm").hasAttribute("disabled")).toBe(false);

    await loadConversation(2, "Two", "two", false);

    expect(byTestId("export-bar").classList.contains("hidden")).toBe(true);
    expect(byTestId("btn-export-confirm").hasAttribute("disabled")).toBe(true);
    byTestId("btn-export-confirm").click();
    await settle();
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith("export_messages", expect.anything());
  });
});

describe("older pages", () => {
  beforeEach(() => {
    vi.resetModules();
    document.body.innerHTML = MARKUP;
  });

  it("show one date separator where two pages meet on one day", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    // The backend answers newest first. Page 1 holds 50 messages of day 10
    // at 12:xx. Page 2 holds one message of day 10 at 11:30 and one of day 9.
    const newest = Array.from({ length: PAGE_SIZE }, (_, i) =>
      message(200 - i, 10, 12, `new ${i}`, PAGE_SIZE - 1 - i)
    );
    const older = [message(100, 10, 11, "old 1", 30), message(99, 9, 11, "old 2")];
    vi.mocked(invoke).mockImplementation((_command: string, args?: InvokeArgs) =>
      Promise.resolve(offsetOf(args) === 0 ? newest : older)
    );
    const { loadConversation } = await import("./messages");

    await loadConversation(1, "One", "one", false);
    const day10 = document.querySelector(".date-separator")?.textContent ?? "";
    expect(day10).not.toBe("");

    area().scrollTop = 0;
    area().dispatchEvent(new Event("scroll"));
    await vi.waitFor(() => expect(texts()).toContain("old 1"));

    const separators = [...document.querySelectorAll(".date-separator")].map(
      (el) => el.textContent
    );
    expect(separators.filter((text) => text === day10)).toHaveLength(1);
    expect(separators).toHaveLength(2);
  });
});

describe("export run", () => {
  const RESULT = { export_path: "/tmp/msgs-test-export", message_count: 2, attachment_count: 0 };

  beforeEach(() => {
    vi.resetModules();
    document.body.innerHTML = MARKUP;
  });

  /** Open chat 1, enter selection mode, and select its two messages. */
  async function selectTwo(): Promise<HTMLElement[]> {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockImplementation((command: string) =>
      Promise.resolve(
        command === "export_messages"
          ? RESULT
          : [message(11, 1, 10, "b"), message(10, 1, 9, "a")]
      )
    );
    const { loadConversation } = await import("./messages");
    await loadConversation(1, "One", "one", false);
    byTestId("btn-export").click();
    const els = messageEls();
    els[0]?.click();
    els[1]?.click();
    return els;
  }

  function exportCalls(invoke: typeof import("@tauri-apps/api/core").invoke): number {
    return vi.mocked(invoke).mock.calls.filter(([command]) => command === "export_messages")
      .length;
  }

  it("starts one export for two clicks on the confirm button", async () => {
    await selectTwo();
    const { invoke } = await import("@tauri-apps/api/core");
    const { save } = await import("@tauri-apps/plugin-dialog");
    const dialog = deferred<string | null>();
    vi.mocked(save).mockReturnValue(dialog.promise);

    byTestId("btn-export-confirm").click();
    byTestId("btn-export-confirm").click();
    dialog.resolve("/tmp/msgs-test-export");
    await settle();

    expect(exportCalls(invoke)).toBe(1);
  });

  it("keeps a new selection that starts within 3 s of a finished export", async () => {
    vi.useFakeTimers();
    try {
      const els = await selectTwo();
      const { save } = await import("@tauri-apps/plugin-dialog");
      vi.mocked(save).mockResolvedValue("/tmp/msgs-test-export");

      byTestId("btn-export-confirm").click();
      await settle();
      expect(byTestId("export-status").textContent).toContain("Exported 2 messages");

      els[0]?.click();
      vi.advanceTimersByTime(3000);

      expect(byTestId("export-bar").classList.contains("hidden")).toBe(false);
      expect(els[0]?.classList.contains("export-start")).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  /**
   * Open chat 1 with three messages, select the first two, and start an export
   * whose reply waits. Then select the last two while the export runs.
   */
  async function newSelectionDuringExport(): Promise<Deferred<typeof RESULT>> {
    const { invoke } = await import("@tauri-apps/api/core");
    const { save } = await import("@tauri-apps/plugin-dialog");
    const reply = deferred<typeof RESULT>();
    vi.mocked(invoke).mockImplementation((command: string) =>
      command === "export_messages"
        ? reply.promise
        : Promise.resolve([message(12, 1, 11, "c"), message(11, 1, 10, "b"), message(10, 1, 9, "a")])
    );
    vi.mocked(save).mockResolvedValue("/tmp/msgs-test-export");
    const { loadConversation } = await import("./messages");
    await loadConversation(1, "One", "one", false);
    byTestId("btn-export").click();
    const els = messageEls();
    els[0]?.click();
    els[1]?.click();
    byTestId("btn-export-confirm").click();
    await settle();
    expect(exportCalls(invoke)).toBe(1);

    els[1]?.click();
    els[2]?.click();
    expect(byTestId("btn-export-confirm").hasAttribute("disabled")).toBe(true);
    return reply;
  }

  it("enables the confirm button for a selection made while the export ran", async () => {
    const reply = await newSelectionDuringExport();

    reply.resolve(RESULT);
    await settle();

    expect(byTestId("btn-export-confirm").hasAttribute("disabled")).toBe(false);
  });

  it("enables the confirm button for a selection made while a failed export ran", async () => {
    const reply = await newSelectionDuringExport();

    reply.reject("Export failed: disk full");
    await settle();

    expect(byTestId("btn-export-confirm").hasAttribute("disabled")).toBe(false);
  });

  it("disables the confirm button while the selection lacks an end", async () => {
    const els = await selectTwo();
    expect(byTestId("btn-export-confirm").hasAttribute("disabled")).toBe(false);

    els[0]?.click();

    expect(byTestId("btn-export-confirm").hasAttribute("disabled")).toBe(true);
  });

  it("disables the confirm button after a cancel", async () => {
    await selectTwo();
    expect(byTestId("btn-export-confirm").hasAttribute("disabled")).toBe(false);

    byTestId("btn-export-cancel").click();

    expect(byTestId("btn-export-confirm").hasAttribute("disabled")).toBe(true);
  });
});

/** `count` messages, newest first, with ids from `topId` down. One message each minute of day 10. */
function pageOf(topId: number, count: number): Message[] {
  return Array.from({ length: count }, (_, i) =>
    message(topId - i, 10, 12, `m${topId - i}`, 59 - (i % 60))
  );
}

/** Give the message area a size, so the scroll tests have a bottom and a top. */
function sizeArea(scrollTop: number): void {
  const el = area();
  Object.defineProperty(el, "scrollHeight", { configurable: true, value: 5000 });
  Object.defineProperty(el, "clientHeight", { configurable: true, value: 500 });
  el.scrollTop = scrollTop;
}

function messageCalls(invoke: typeof import("@tauri-apps/api/core").invoke): unknown[][] {
  return vi.mocked(invoke).mock.calls.filter(([command]) => command === "get_messages");
}

describe("openConversationAt", () => {
  const HIT = 120;
  const NOTICE = "The message of this search result is no longer in this chat.";

  beforeEach(() => {
    vi.resetModules();
    document.body.innerHTML = MARKUP;
  });

  /** `get_message_position` answers `position`, and `get_messages` answers by offset. */
  async function answerWithPosition(position: number | Promise<number>) {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockImplementation((command: string, args?: InvokeArgs) => {
      if (command === "get_message_position") {
        return typeof position === "number" ? Promise.resolve(position) : position;
      }
      const offset = offsetOf(args);
      if (typeof offset !== "number") return Promise.reject("no offset");
      // Message ids count down with the offset: id = 1000 - offset.
      const limit =
        args !== undefined && !Array.isArray(args) && !(args instanceof ArrayBuffer)
          && !(args instanceof Uint8Array) && typeof args.limit === "number"
          ? args.limit
          : 0;
      return Promise.resolve(pageOf(1000 - offset, limit));
    });
    return invoke;
  }

  it("loads the page around the hit and highlights the hit", async () => {
    const invoke = await answerWithPosition(120);
    const { openConversationAt } = await import("./messages");

    // The hit sits at position 120, thus its id is 1000 - 120 = 880.
    await openConversationAt(1, "One", "one", false, 880);

    expect(invoke).toHaveBeenCalledWith("get_message_position", { chatId: 1, messageId: 880 });
    expect(invoke).toHaveBeenCalledWith(
      "get_messages",
      expect.objectContaining({ chatId: 1, offset: 95, limit: PAGE_SIZE })
    );
    const hit = document.querySelector('[data-message-id="880"]');
    expect(hit?.classList.contains("search-highlight")).toBe(true);
  });

  it("loads newer messages when the scroll reaches the bottom, down to offset 0", async () => {
    const invoke = await answerWithPosition(120);
    const { openConversationAt } = await import("./messages");
    await openConversationAt(1, "One", "one", false, 880);
    expect(messageCalls(invoke)).toHaveLength(1);

    sizeArea(4500);
    area().dispatchEvent(new Event("scroll"));
    await vi.waitFor(() => expect(messageCalls(invoke)).toHaveLength(2));
    expect(messageCalls(invoke)[1]?.[1]).toEqual(
      expect.objectContaining({ chatId: 1, offset: 45, limit: PAGE_SIZE })
    );
    await vi.waitFor(() => expect(texts()).toContain("m955"));
    // One day, one separator, also where the newer page joins.
    expect(document.querySelectorAll(".date-separator")).toHaveLength(1);
    // The newer messages sit below the older ones.
    const order = messageEls().map((el) => Number(el.dataset.messageId));
    expect(order.indexOf(955)).toBeGreaterThan(order.indexOf(880));

    area().dispatchEvent(new Event("scroll"));
    await vi.waitFor(() => expect(messageCalls(invoke)).toHaveLength(3));
    expect(messageCalls(invoke)[2]?.[1]).toEqual(
      expect.objectContaining({ chatId: 1, offset: 0, limit: 45 })
    );
    await vi.waitFor(() => expect(texts()).toContain("m1000"));

    area().dispatchEvent(new Event("scroll"));
    await settle();
    expect(messageCalls(invoke)).toHaveLength(3);
  });

  it("shows a notice and opens the newest page when the position is gone", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockImplementation((command: string) =>
      command === "get_message_position"
        ? Promise.reject("Database error: Message 5 is not in chat 1")
        : Promise.resolve([message(11, 1, 10, "b"), message(10, 1, 9, "a")])
    );
    const { openConversationAt } = await import("./messages");

    await openConversationAt(1, "One", "one", false, 5);

    expect(area().querySelector(".load-error")?.textContent).toBe(NOTICE);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "get_messages",
      expect.objectContaining({ chatId: 1, offset: 0, limit: PAGE_SIZE })
    );
    expect(texts()).toEqual(["a", "b"]);
  });

  it("ignores a scroll of the old chat while the position of the hit is in flight", async () => {
    const slow = deferred<number>();
    const invoke = await answerWithPosition(slow.promise);
    const { openConversationAt, loadConversation } = await import("./messages");
    await loadConversation(1, "One", "one", false);

    const opening = openConversationAt(2, "Two", "two", false, 880);
    await settle();
    area().scrollTop = 0;
    area().dispatchEvent(new Event("scroll"));
    slow.resolve(120);
    await opening;

    const chatTwoOffsets = messageCalls(invoke)
      .filter((call) => chatIdOf(call[1] as InvokeArgs) === 2)
      .map((call) => offsetOf(call[1] as InvokeArgs));
    expect(chatTwoOffsets).toEqual([95]);
    const hit = document.querySelector('[data-message-id="880"]');
    expect(hit?.classList.contains("search-highlight")).toBe(true);
  });

  it("drops the position of a chat that is no longer open", async () => {
    const slow = deferred<number>();
    const invoke = await answerWithPosition(slow.promise);
    const { openConversationAt, loadConversation } = await import("./messages");

    const opening = openConversationAt(1, "One", "one", false, 880);
    await settle();
    await loadConversation(2, "Two", "two", false);
    slow.resolve(120);
    await opening;

    expect(document.getElementById("chat-name")?.textContent).toBe("Two");
    expect(messageCalls(invoke).every((call) => chatIdOf(call[1] as InvokeArgs) === 2)).toBe(true);
  });
});
