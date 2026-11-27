import { invoke } from "@tauri-apps/api/core";
import type { Message } from "./bindings";
import { byId, messageOf, UNKNOWN_DATE_LABEL } from "./utils";
import { handleMessageClick, initExport, resetExport } from "./export";

const PAGE_SIZE = 50;
/** How many messages a hit gets on its older side when its chat opens around it. */
const HALF_PAGE = 25;
/** How close to an edge of the message area (in pixels) a scroll loads the next page. */
const EDGE_DISTANCE = 200;
/** How long the hit of a search stays highlighted, in milliseconds. */
const HIGHLIGHT_MS = 2000;
const HIGHLIGHT_CLASS = "search-highlight";
const MISSING_HIT_NOTICE = "The message of this search result is no longer in this chat.";

let currentChatId: number | null = null;
/** The offset where the next OLDER page starts: the oldest loaded offset plus its count. */
let olderOffset = 0;
/** The offset of the newest loaded message. Newer messages exist while it is above 0. */
let newestOffset = 0;
/** Counts each open of a chat. A reply of an older generation is stale. */
let loadGeneration = 0;
/** The generation that has a page request in flight, or null. */
let loadingGeneration: number | null = null;
/** True when no older message is left to load. */
let allOlderLoaded = false;
/** True after a page request of the current generation failed. */
let loadFailed = false;
let isGroupChat = false;

let messageScrollController: AbortController | null = null;

/** Which end of the loaded window a page joins. */
type PageEnd = "older" | "newer";

/**
 * Reset the view for a chat and return the generation of this open.
 * A selection of the old chat must not reach the new chat.
 */
function beginOpen(
  chatId: number,
  displayName: string,
  identifier: string,
  group: boolean
): number {
  resetExport();
  // A scroll of the old chat must not load a page of the new chat
  messageScrollController?.abort();
  messageScrollController = null;

  const generation = ++loadGeneration;
  currentChatId = chatId;
  olderOffset = 0;
  newestOffset = 0;
  allOlderLoaded = false;
  loadFailed = false;
  isGroupChat = group;

  byId("chat-name", HTMLElement).textContent = displayName;
  byId("chat-identifier", HTMLElement).textContent = identifier;

  byId("empty-state", HTMLElement).classList.add("hidden");
  byId("message-view", HTMLElement).classList.remove("hidden");

  byId("message-area", HTMLElement).innerHTML = "";
  return generation;
}

/** Wire the scroll handler and the export mode once the first page is in. */
function finishOpen(chatId: number, area: HTMLElement): void {
  messageScrollController?.abort();
  messageScrollController = new AbortController();

  area.addEventListener("scroll", handleMessageScroll, { signal: messageScrollController.signal });

  // Initialize export selection mode
  initExport(chatId);
}

/** Open a chat at its newest message. */
export async function loadConversation(
  chatId: number,
  displayName: string,
  identifier: string,
  group: boolean
): Promise<void> {
  const generation = beginOpen(chatId, displayName, identifier, group);
  await openAtNewest(generation, chatId);
}

/** Load the newest page, then scroll to the bottom and wire the scroll. */
async function openAtNewest(generation: number, chatId: number): Promise<void> {
  const area = byId("message-area", HTMLElement);
  await loadOlderMessages();

  // A newer call took over while the page was in flight
  if (generation !== loadGeneration) return;

  // Scroll to bottom (most recent)
  area.scrollTop = area.scrollHeight;
  finishOpen(chatId, area);
}

/**
 * Open a chat around one message and highlight it. The window grows both ways
 * from there. When the backend no longer finds the message, the chat opens at
 * its newest message and a notice says so.
 */
export async function openConversationAt(
  chatId: number,
  displayName: string,
  identifier: string,
  group: boolean,
  messageId: number
): Promise<void> {
  const generation = beginOpen(chatId, displayName, identifier, group);

  let position: number;
  try {
    position = await invoke<number>("get_message_position", { chatId, messageId });
  } catch {
    if (generation !== loadGeneration) return;
    showNotice(MISSING_HIT_NOTICE);
    await openAtNewest(generation, chatId);
    return;
  }
  if (generation !== loadGeneration) return;

  const start = Math.max(0, position - HALF_PAGE);
  olderOffset = start;
  newestOffset = start;
  await loadOlderMessages();
  if (generation !== loadGeneration) return;

  const area = byId("message-area", HTMLElement);
  highlightHit(area, messageId);
  finishOpen(chatId, area);
}

/** Scroll the message of a search hit to the middle and highlight it for a short time. */
function highlightHit(area: HTMLElement, messageId: number): void {
  const hit = area.querySelector(`[data-message-id="${messageId}"]`);
  if (hit === null) return;
  hit.scrollIntoView({ block: "center" });
  hit.classList.add(HIGHLIGHT_CLASS);
  setTimeout(() => hit.classList.remove(HIGHLIGHT_CLASS), HIGHLIGHT_MS);
}

/** Load the page that ends where the loaded messages start (older messages). */
async function loadOlderMessages(): Promise<void> {
  if (allOlderLoaded) return;
  await loadPage("older", olderOffset, PAGE_SIZE);
}

/** Load the page that starts where the loaded messages end (newer messages). */
async function loadNewerMessages(): Promise<void> {
  if (newestOffset <= 0) return;
  const offset = Math.max(0, newestOffset - PAGE_SIZE);
  await loadPage("newer", offset, newestOffset - offset);
}

/** The date separators of a page in chronological order, and its message elements. */
function buildPage(chronological: Message[], skipSeparatorFor: string | null): DocumentFragment {
  const fragment = document.createDocumentFragment();
  let lastDateStr = skipSeparatorFor ?? "";
  for (const msg of chronological) {
    const dateStr = formatDateSeparator(msg.date);
    if (dateStr !== lastDateStr) {
      fragment.appendChild(createDateSeparator(dateStr));
      lastDateStr = dateStr;
    }
    fragment.appendChild(createMessageElement(msg));
  }
  return fragment;
}

/** Fetch one page of the current chat and join it to the loaded window at `end`. */
async function loadPage(end: PageEnd, offset: number, limit: number): Promise<void> {
  const generation = loadGeneration;
  if (loadingGeneration === generation || loadFailed || currentChatId === null) {
    return;
  }
  loadingGeneration = generation;

  try {
    const messages = await invoke<Message[]>("get_messages", {
      chatId: currentChatId,
      offset,
      limit,
    });

    // The user opened another chat while this page was in flight
    if (generation !== loadGeneration) return;

    const area = byId("message-area", HTMLElement);
    // Messages come in reverse chronological order; reverse for display
    const chronological = [...messages].reverse();

    if (end === "older") {
      if (messages.length < limit) allOlderLoaded = true;
      const previousScrollHeight = area.scrollHeight;
      const firstOnScreen = area.querySelector(".message") !== null;
      const fragment = buildPage(chronological, null);
      // The new page ends on a day that the screen starts with: keep one separator
      const lastDateStr = formatDateSeparator(chronological[chronological.length - 1]?.date ?? null);
      const onScreen = area.querySelector(".date-separator");
      if (onScreen !== null && onScreen.textContent === lastDateStr) onScreen.remove();
      area.insertBefore(fragment, area.querySelector(".message, .date-separator"));
      // Maintain scroll position after prepending
      if (firstOnScreen) area.scrollTop = area.scrollHeight - previousScrollHeight;
      olderOffset = offset + messages.length;
    } else {
      // The new page starts on a day that the screen ends with: keep one separator
      const separators = area.querySelectorAll(".date-separator");
      const lastOnScreen = separators[separators.length - 1]?.textContent ?? null;
      area.appendChild(buildPage(chronological, lastOnScreen));
      newestOffset = offset;
    }
  } catch (e) {
    if (generation !== loadGeneration) return;
    loadFailed = true;
    showNotice(messageOf(e));
  } finally {
    if (loadingGeneration === generation) loadingGeneration = null;
  }
}

/** Show a notice, or the text of a failed page request, at the top of the message area. */
function showNotice(text: string): void {
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
  if (target.scrollTop < EDGE_DISTANCE) {
    loadOlderMessages().catch(console.error);
  }
  // Load newer messages when scrolling near the bottom of a window that does not reach the newest
  if (target.scrollTop + target.clientHeight >= target.scrollHeight - EDGE_DISTANCE) {
    loadNewerMessages().catch(console.error);
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
