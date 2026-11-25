import { invoke } from "@tauri-apps/api/core";
import type { Conversation } from "./bindings";
import { byId, escapeHtml } from "./utils";

const PAGE_SIZE = 50;
let currentOffset = 0;
let loading = false;
// Counts resets of the list. A reply of an older generation is dropped.
let listGeneration = 0;
let selectedChatId: number | null = null;
let onSelectCallback: ((chatId: number) => void) | null = null;

// Cache conversation data for use by other modules
const conversationCache = new Map<number, Conversation>();

let conversationsInitialized = false;

export function getConversationById(chatId: number): Conversation | undefined {
  return conversationCache.get(chatId);
}

export function onConversationSelect(callback: (chatId: number) => void): void {
  onSelectCallback = callback;
}

export async function initConversations(): Promise<void> {
  const list = byId("conversation-list", HTMLElement);

  if (!conversationsInitialized) {
    conversationsInitialized = true;
    list.addEventListener("scroll", handleScroll);

    document.addEventListener("keydown", (e: KeyboardEvent) => {
      if (!isConversationKeyTarget(e.target)) return;

      const items = document.querySelectorAll<HTMLElement>(".conversation-item");
      if (items.length === 0) return;

      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const activeEl = document.querySelector<HTMLElement>(".conversation-item.active");
        const activeIndex = activeEl
          ? Array.from(items).indexOf(activeEl)
          : -1;

        let newIndex: number;
        if (e.key === "ArrowDown") {
          newIndex = activeIndex < items.length - 1 ? activeIndex + 1 : activeIndex;
        } else {
          newIndex = activeIndex > 0 ? activeIndex - 1 : 0;
        }

        const newEl = items[newIndex];
        const newId = newEl?.dataset.chatId;
        if (newEl === undefined || newId === undefined) return;
        highlightConversation(Number(newId), newEl);
        newEl.scrollIntoView({ block: "nearest" });
      }

      if (e.key === "Enter") {
        const activeEl = document.querySelector<HTMLElement>(".conversation-item.active");
        const activeId = activeEl?.dataset.chatId;
        if (activeId !== undefined) onSelectCallback?.(Number(activeId));
      }
    });
  }

  // Always reload conversations
  listGeneration += 1;
  currentOffset = 0;
  loading = false;
  list.innerHTML = "";
  conversationCache.clear();
  await loadMore();
}

/**
 * Whether a key press belongs to the conversation list. It does when the target
 * is the page body or sits inside the sidebar, and is not a control that uses
 * the key itself.
 */
function isConversationKeyTarget(target: EventTarget | null): boolean {
  if (target === document.body) return true;
  if (!(target instanceof HTMLElement)) return false;
  if (target.closest("#sidebar") === null) return false;
  if (target.isContentEditable) return false;
  return target.closest("input, textarea, select, button") === null;
}

async function loadMore(): Promise<void> {
  if (loading) return;
  loading = true;
  const generation = listGeneration;
  try {
    const conversations = await invoke<Conversation[]>("list_conversations", {
      offset: currentOffset,
      limit: PAGE_SIZE,
    });
    const list = byId("conversation-list", HTMLElement);
    // A reset or a search replaced the list while the page was in flight
    if (generation !== listGeneration || showsSearch(list)) return;
    for (const conv of conversations) {
      conversationCache.set(conv.chat_id, conv);
      list.appendChild(createConversationElement(conv));
    }
    currentOffset += conversations.length;
  } finally {
    if (generation === listGeneration) loading = false;
  }
}

function createConversationElement(conv: Conversation): HTMLElement {
  const el = document.createElement("div");
  el.className = "conversation-item";
  el.dataset.chatId = String(conv.chat_id);

  const displayName = conv.display_name ?? conv.participants[0] ?? conv.chat_identifier;
  const dateStr = formatRelativeDate(conv.last_message_date);

  el.innerHTML = `
    <div class="header">
      <span class="name">${escapeHtml(displayName)}</span>
      <span class="time">${escapeHtml(dateStr)}</span>
    </div>
    <div class="preview">${escapeHtml(conv.last_message_preview)}</div>
  `;

  el.addEventListener("click", () => selectConversation(conv.chat_id, el));
  return el;
}

function highlightConversation(chatId: number, el: HTMLElement): void {
  document.querySelectorAll(".conversation-item.active").forEach((item) => {
    item.classList.remove("active");
  });
  el.classList.add("active");
  selectedChatId = chatId;
}

/** Mark the sidebar item of a chat as active. Does nothing when the sidebar has no such item. */
export function highlightConversationById(chatId: number): void {
  const el = document.querySelector<HTMLElement>(`.conversation-item[data-chat-id="${chatId}"]`);
  if (el !== null) highlightConversation(chatId, el);
}

export function selectConversation(chatId: number, el: HTMLElement): void {
  highlightConversation(chatId, el);
  onSelectCallback?.(chatId);
}

/** Whether the list shows search results in place of the conversations. */
function showsSearch(list: HTMLElement): boolean {
  return list.dataset.mode === "search";
}

function handleScroll(e: Event): void {
  if (!(e.target instanceof HTMLElement)) return;
  const target = e.target;
  if (showsSearch(target)) return;
  if (target.scrollTop + target.clientHeight >= target.scrollHeight - 100) {
    loadMore().catch(console.error);
  }
}

function formatRelativeDate(isoDate: string | null): string {
  if (isoDate === null) return "";
  const date = new Date(isoDate);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));
  if (diffDays === 0) return date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  if (diffDays === 1) return "Yesterday";
  if (diffDays < 7) return date.toLocaleDateString([], { weekday: "short" });
  return date.toLocaleDateString([], { month: "short", day: "numeric" });
}

export function getSelectedChatId(): number | null {
  return selectedChatId;
}
