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
