import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SearchResult } from "./bindings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));

const MARKUP = `
  <input type="text" id="global-search" placeholder="Search messages..." />
  <div id="conversation-list"></div>
  <div id="empty-state"></div>
  <div id="message-view" class="hidden">
    <span id="chat-name"></span><span id="chat-identifier"></span>
    <button id="btn-export"></button>
    <div id="message-area"></div>
    <div id="export-bar" class="hidden"><span id="export-status"></span></div>
  </div>
  <div id="error-overlay" class="hidden"></div>
`;

type Answers = Record<string, () => Promise<unknown>>;

/** Make `invoke` answer by command name. */
async function answerWith(answers: Answers): Promise<void> {
  const { invoke } = await import("@tauri-apps/api/core");
  vi.mocked(invoke).mockImplementation((command: string) => {
    const answer = answers[command];
    if (answer === undefined) return Promise.reject(`unexpected command ${command}`);
    return answer();
  });
}

function overlay(): HTMLElement {
  const el = document.getElementById("error-overlay");
  if (el === null) throw new Error("no overlay in the test markup");
  return el;
}

describe("init", () => {
  beforeEach(() => {
    vi.resetModules();
    document.body.innerHTML = MARKUP;
  });

  it("shows the error when check_db_access rejects", async () => {
    await answerWith({
      check_db_access: () => Promise.reject("Database error: disk I/O error"),
    });
    const { init } = await import("./main");

    await expect(init()).resolves.toBeUndefined();

    expect(overlay().classList.contains("hidden")).toBe(false);
    expect(overlay().textContent).toContain("Database error: disk I/O error");
  });

  it("shows the error when the first page of conversations rejects", async () => {
    await answerWith({
      check_db_access: () =>
        Promise.resolve({ accessible: true, message_count: 1, error: null }),
      get_cache_status: () => Promise.resolve({ state: "ready" }),
      list_conversations: () => Promise.reject("Database error: locked"),
    });
    const { init } = await import("./main");

    await expect(init()).resolves.toBeUndefined();

    expect(overlay().classList.contains("hidden")).toBe(false);
    expect(overlay().textContent).toContain("Database error: locked");
  });
});

describe("a click on a search result", () => {
  beforeEach(() => {
    vi.resetModules();
    document.body.innerHTML = MARKUP;
  });

  it("opens a chat that the sidebar did not load", async () => {
    const hit: SearchResult = {
      message: {
        message_id: 77,
        text: "needle",
        sender: "Zed",
        is_from_me: false,
        date: null,
        date_read: null,
        is_audio: false,
        attachments: [],
        reply_to_guid: null,
        associated_emoji: null,
      },
      conversation: {
        chat_id: 9,
        chat_identifier: "zed@example.com",
        display_name: "Zed Chat",
        is_group: false,
        participants: ["Zed"],
        last_message_date: null,
        last_message_preview: "",
        message_count: 1,
      },
      context_before: [],
      context_after: [],
    };
    await answerWith({
      check_db_access: () =>
        Promise.resolve({ accessible: true, message_count: 1, error: null }),
      get_cache_status: () => Promise.resolve({ state: "ready" }),
      list_conversations: () => Promise.resolve([]),
      search_messages: () => Promise.resolve([hit]),
      get_message_position: () => Promise.resolve(0),
      get_messages: () => Promise.resolve([]),
    });
    const { init } = await import("./main");
    await init();

    const box = document.getElementById("global-search");
    if (!(box instanceof HTMLInputElement)) throw new Error("no search box");
    box.value = "needle";
    box.dispatchEvent(new Event("input"));
    await vi.waitFor(() => expect(document.querySelector(".search-result")).not.toBeNull());
    document.querySelector<HTMLElement>(".search-result")?.click();

    await vi.waitFor(() =>
      expect(document.getElementById("chat-name")?.textContent).toBe("Zed Chat")
    );
    expect(document.getElementById("chat-identifier")?.textContent).toBe("zed@example.com");
  });
});
