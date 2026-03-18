import { invoke } from "@tauri-apps/api/core";
import type { Message } from "./types";
import { handleMessageClick, initExport } from "./export";

const PAGE_SIZE = 50;
let currentChatId: number | null = null;
let currentOffset = 0;
let loading = false;
let allLoaded = false;
let isGroupChat = false;

let messageScrollController: AbortController | null = null;

export async function loadConversation(
  chatId: number,
  displayName: string,
  identifier: string,
  group: boolean
): Promise<void> {
  currentChatId = chatId;
  currentOffset = 0;
  allLoaded = false;
  isGroupChat = group;

  // Update header
  document.getElementById("chat-name")!.textContent = displayName;
  document.getElementById("chat-identifier")!.textContent = identifier;

  // Show message view, hide empty state
  document.getElementById("empty-state")!.classList.add("hidden");
  document.getElementById("message-view")!.classList.remove("hidden");

  // Clear messages
  const area = document.getElementById("message-area")!;
  area.innerHTML = "";

  // Load first page
  await loadMoreMessages();

  // Scroll to bottom (most recent)
  area.scrollTop = area.scrollHeight;

  // Clean up previous scroll listener
  messageScrollController?.abort();
  messageScrollController = new AbortController();

  // Set up scroll handler for loading older messages
  area.addEventListener("scroll", handleMessageScroll, { signal: messageScrollController.signal });

  // Initialize export selection mode
  initExport(chatId);
}

async function loadMoreMessages(): Promise<void> {
  if (loading || allLoaded || currentChatId === null) return;
  loading = true;

  try {
    const messages = await invoke<Message[]>("get_messages", {
      chatId: currentChatId,
      offset: currentOffset,
      limit: PAGE_SIZE,
    });

    if (messages.length < PAGE_SIZE) {
      allLoaded = true;
    }

    const area = document.getElementById("message-area")!;
    const previousScrollHeight = area.scrollHeight;

    // Messages come in reverse chronological order; reverse for display
    const chronological = [...messages].reverse();

    // Prepend older messages at the top
    const fragment = document.createDocumentFragment();
    let lastDateStr = "";

    for (const msg of chronological) {
      const dateStr = formatDateSeparator(msg.date);
      if (dateStr !== lastDateStr) {
        fragment.appendChild(createDateSeparator(dateStr));
        lastDateStr = dateStr;
      }
      fragment.appendChild(createMessageElement(msg));
    }

    if (currentOffset === 0) {
      area.appendChild(fragment);
    } else {
      area.insertBefore(fragment, area.firstChild);
      // Maintain scroll position after prepending
      area.scrollTop = area.scrollHeight - previousScrollHeight;
    }

    currentOffset += messages.length;
  } finally {
    loading = false;
  }
}

function createMessageElement(msg: Message): HTMLElement {
  const wrapper = document.createElement("div");
  wrapper.className = `message ${msg.is_from_me ? "sent" : "received"}`;
  wrapper.dataset.messageId = String(msg.message_id);
  wrapper.dataset.messageDate = msg.date;

  const bubble = document.createElement("div");
  bubble.className = "bubble";

  // Sender label for group chats
  if (isGroupChat && !msg.is_from_me) {
    const sender = document.createElement("div");
    sender.className = "sender-label";
    sender.textContent = msg.sender;
    bubble.appendChild(sender);
  }

  // Message text
  if (msg.text) {
    const text = document.createElement("div");
    text.className = "message-text";
    text.textContent = msg.text;
    bubble.appendChild(text);
  }

  // Attachment indicators
  for (const att of msg.attachments) {
    const attEl = document.createElement("div");
    attEl.className = "attachment-indicator";
    const name = att.transfer_name ?? att.filename ?? "Attachment";
    const size = formatBytes(att.total_bytes);
    attEl.textContent = `📎 ${name} (${size})`;
    bubble.appendChild(attEl);
  }

  // Timestamp
  const time = document.createElement("div");
  time.className = "message-time";
  time.textContent = formatTime(msg.date);
  bubble.appendChild(time);

  // Reaction/tapback
  if (msg.associated_emoji) {
    const emoji = document.createElement("span");
    emoji.className = "tapback";
    emoji.textContent = msg.associated_emoji;
    wrapper.appendChild(emoji);
  }

  wrapper.appendChild(bubble);
  wrapper.addEventListener("click", () => handleMessageClick(wrapper));
  return wrapper;
}

function createDateSeparator(dateStr: string): HTMLElement {
  const el = document.createElement("div");
  el.className = "date-separator";
  el.textContent = dateStr;
  return el;
}

function handleMessageScroll(e: Event): void {
  const target = e.target as HTMLElement;
  // Load more when scrolling near the top
  if (target.scrollTop < 200) {
    loadMoreMessages().catch(console.error);
  }
}

function formatDateSeparator(isoDate: string): string {
  const date = new Date(isoDate);
  return date.toLocaleDateString([], {
    weekday: "long",
    year: "numeric",
    month: "long",
    day: "numeric",
  });
}

function formatTime(isoDate: string): string {
  return new Date(isoDate).toLocaleTimeString([], {
    hour: "numeric",
    minute: "2-digit",
  });
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const size = bytes / Math.pow(1024, i);
  return `${size.toFixed(i > 0 ? 1 : 0)} ${units[i]}`;
}
