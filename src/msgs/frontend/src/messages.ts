import { invoke } from "@tauri-apps/api/core";
import type { Message } from "./bindings";
import { byId, messageOf, UNKNOWN_DATE_LABEL } from "./utils";
import { handleMessageClick, initExport, resetExport } from "./export";

const PAGE_SIZE = 50;
let currentChatId: number | null = null;
let currentOffset = 0;
/** Counts each call of `loadConversation`. A reply of an older generation is stale. */
let loadGeneration = 0;
/** The generation that has a page request in flight, or null. */
let loadingGeneration: number | null = null;
let allLoaded = false;
/** True after a page request of the current generation failed. */
let loadFailed = false;
let isGroupChat = false;

let messageScrollController: AbortController | null = null;

export async function loadConversation(
  chatId: number,
  displayName: string,
  identifier: string,
  group: boolean
): Promise<void> {
  // A selection of the old chat must not reach the new chat
  resetExport();

  const generation = ++loadGeneration;
  currentChatId = chatId;
  currentOffset = 0;
  allLoaded = false;
  loadFailed = false;
  isGroupChat = group;

  // Update header
  byId("chat-name", HTMLElement).textContent = displayName;
  byId("chat-identifier", HTMLElement).textContent = identifier;

  // Show message view, hide empty state
  byId("empty-state", HTMLElement).classList.add("hidden");
  byId("message-view", HTMLElement).classList.remove("hidden");

  // Clear messages
  const area = byId("message-area", HTMLElement);
  area.innerHTML = "";

  // Load first page
  await loadMoreMessages();

  // A newer call took over while the page was in flight
  if (generation !== loadGeneration) return;

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

/**
 * Open a chat around one message and highlight it.
 * Stub: opens the newest page, as `loadConversation` does.
 */
export async function openConversationAt(
  chatId: number,
  displayName: string,
  identifier: string,
  group: boolean,
  messageId: number
): Promise<void> {
  void messageId;
  await loadConversation(chatId, displayName, identifier, group);
}

async function loadMoreMessages(): Promise<void> {
  const generation = loadGeneration;
  if (loadingGeneration === generation || allLoaded || loadFailed || currentChatId === null) {
    return;
  }
  loadingGeneration = generation;

  try {
    const messages = await invoke<Message[]>("get_messages", {
      chatId: currentChatId,
      offset: currentOffset,
      limit: PAGE_SIZE,
    });

    // The user opened another chat while this page was in flight
    if (generation !== loadGeneration) return;

    if (messages.length < PAGE_SIZE) {
      allLoaded = true;
    }

    const area = byId("message-area", HTMLElement);
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
      // The new page ends on a day that the screen starts with: keep one separator
      const onScreen = area.querySelector(".date-separator");
      if (onScreen !== null && onScreen.textContent === lastDateStr) onScreen.remove();
      area.insertBefore(fragment, area.firstChild);
      // Maintain scroll position after prepending
      area.scrollTop = area.scrollHeight - previousScrollHeight;
    }

    currentOffset += messages.length;
  } catch (e) {
    if (generation !== loadGeneration) return;
    loadFailed = true;
    showLoadError(messageOf(e));
  } finally {
    if (loadingGeneration === generation) loadingGeneration = null;
  }
}

/** Show the text of a failed page request at the top of the message area. */
function showLoadError(text: string): void {
  const el = document.createElement("div");
  el.className = "load-error";
  el.textContent = text;
  const area = byId("message-area", HTMLElement);
  area.insertBefore(el, area.firstChild);
}

function createMessageElement(msg: Message): HTMLElement {
  const wrapper = document.createElement("div");
  wrapper.className = `message ${msg.is_from_me ? "sent" : "received"}`;
  wrapper.dataset.messageId = String(msg.message_id);

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
  if (!(e.target instanceof HTMLElement)) return;
  const target = e.target;
  // Load more when scrolling near the top
  if (target.scrollTop < 200) {
    loadMoreMessages().catch(console.error);
  }
}

function formatDateSeparator(isoDate: string | null): string {
  if (isoDate === null) return UNKNOWN_DATE_LABEL;
  const date = new Date(isoDate);
  return date.toLocaleDateString([], {
    weekday: "long",
    year: "numeric",
    month: "long",
    day: "numeric",
  });
}

function formatTime(isoDate: string | null): string {
  if (isoDate === null) return "";
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
