import { invoke } from "@tauri-apps/api/core";
import type { Conversation } from "./types";
import { escapeHtml } from "./utils";

const PAGE_SIZE = 50;
let currentOffset = 0;
let loading = false;
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
  const list = document.getElementById("conversation-list")!;

  if (!conversationsInitialized) {
    conversationsInitialized = true;
    list.addEventListener("scroll", handleScroll);

    document.addEventListener("keydown", (e: KeyboardEvent) => {
      // Don't capture keys when an input is focused
      if (document.activeElement instanceof HTMLInputElement ||
          document.activeElement instanceof HTMLTextAreaElement) {
        return;
      }

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
        const chatId = Number(newEl.dataset.chatId);
        highlightConversation(chatId, newEl);
        newEl.scrollIntoView({ block: "nearest" });
      }

      if (e.key === "Enter") {
        const activeEl = document.querySelector<HTMLElement>(".conversation-item.active");
        if (activeEl) {
          const chatId = Number(activeEl.dataset.chatId);
          onSelectCallback?.(chatId);
        }
      }
    });
  }

  // Always reload conversations
  currentOffset = 0;
  list.innerHTML = "";
  conversationCache.clear();
  await loadMore();
}

async function loadMore(): Promise<void> {
  if (loading) return;
  loading = true;
  try {
    const conversations = await invoke<Conversation[]>("list_conversations", {
      offset: currentOffset,
      limit: PAGE_SIZE,
    });
    const list = document.getElementById("conversation-list")!;
    for (const conv of conversations) {
      conversationCache.set(conv.chat_id, conv);
      list.appendChild(createConversationElement(conv));
    }
    currentOffset += conversations.length;
  } finally {
    loading = false;
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

export function selectConversation(chatId: number, el: HTMLElement): void {
  highlightConversation(chatId, el);
  onSelectCallback?.(chatId);
}

function handleScroll(e: Event): void {
  const target = e.target as HTMLElement;
  if (target.scrollTop + target.clientHeight >= target.scrollHeight - 100) {
    loadMore().catch(console.error);
  }
}

function formatRelativeDate(isoDate: string): string {
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
