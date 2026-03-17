import { invoke } from "@tauri-apps/api/core";

interface Message {
  message_id: number;
  text: string | null;
  sender: string;
  is_from_me: boolean;
  date: string;
  date_read: string | null;
  is_audio: boolean;
  attachments: AttachmentInfo[];
  reply_to_guid: string | null;
  associated_emoji: string | null;
}

interface AttachmentInfo {
  attachment_id: number;
  filename: string | null;
  mime_type: string | null;
  total_bytes: number;
  transfer_name: string | null;
  is_sticker: boolean;
}

interface Conversation {
  chat_id: number;
  chat_identifier: string;
  display_name: string | null;
  is_group: boolean;
  participants: string[];
  last_message_date: string;
  last_message_preview: string;
  message_count: number;
}

interface SearchResult {
  message: Message;
  conversation: Conversation;
  context_before: Message[];
  context_after: Message[];
}

let searchTimeout: number | null = null;
let onNavigateCallback: ((chatId: number, messageId: number) => void) | null =
  null;

export function initSearch(
  onNavigate: (chatId: number, messageId: number) => void
): void {
  onNavigateCallback = onNavigate;

  const input = document.getElementById("global-search") as HTMLInputElement;
  input.addEventListener("input", () => {
    if (searchTimeout) clearTimeout(searchTimeout);
    searchTimeout = window.setTimeout(() => performSearch(input.value), 300);
  });

  input.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      input.value = "";
      hideSearchResults();
    }
  });
}

async function performSearch(query: string): Promise<void> {
  if (query.length < 2) {
    hideSearchResults();
    return;
  }

  const results = await invoke<SearchResult[]>("search_messages", {
    query,
    chatId: null,
    offset: 0,
    limit: 50,
  });

  showSearchResults(results);
}

function showSearchResults(results: SearchResult[]): void {
  const list = document.getElementById("conversation-list")!;
  // Replace conversation list with search results temporarily
  list.dataset.mode = "search";

  list.innerHTML = "";
  if (results.length === 0) {
    list.innerHTML = '<div class="search-empty">No results found</div>';
    return;
  }

  for (const result of results) {
    const el = document.createElement("div");
    el.className = "search-result";

    const convName =
      result.conversation.display_name ??
      result.conversation.participants[0] ??
      result.conversation.chat_identifier;

    el.innerHTML = `
      <div class="search-result-header">${escapeHtml(convName)}</div>
      <div class="search-result-sender">${escapeHtml(result.message.sender)}</div>
      <div class="search-result-text">${escapeHtml(result.message.text ?? "")}</div>
      <div class="search-result-time">${new Date(result.message.date).toLocaleDateString()}</div>
    `;

    el.addEventListener("click", () => {
      onNavigateCallback?.(
        result.conversation.chat_id,
        result.message.message_id
      );
    });

    list.appendChild(el);
  }
}

function hideSearchResults(): void {
  const list = document.getElementById("conversation-list")!;
  if (list.dataset.mode === "search") {
    list.dataset.mode = "";
    // Re-initialize conversation list without losing app state
    list.innerHTML = "";
    // Dynamic import to avoid circular deps
    import("./conversations").then((mod) => mod.initConversations());
  }
}

function escapeHtml(text: string): string {
  const div = document.createElement("div");
  div.textContent = text;
  return div.innerHTML;
}
