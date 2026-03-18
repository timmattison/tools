import { invoke } from "@tauri-apps/api/core";
import type { DbStatus } from "./types";
import {
  initConversations,
  onConversationSelect,
  getConversationById,
} from "./conversations";
import { loadConversation } from "./messages";
import { initSearch } from "./search";

async function init(): Promise<void> {
  const status = await invoke<DbStatus>("check_db_access");
  if (!status.accessible) {
    showError(status.error ?? "Cannot access message database");
    return;
  }

  onConversationSelect(handleConversationSelect);
  initSearch(handleSearchNavigate);
  await initConversations();
}

async function handleSearchNavigate(chatId: number, messageId: number): Promise<void> {
  await handleConversationSelect(chatId);

  // Scroll to and highlight the matched message
  requestAnimationFrame(() => {
    const msgEl = document.querySelector(`[data-message-id="${messageId}"]`);
    if (msgEl) {
      msgEl.scrollIntoView({ block: "center", behavior: "smooth" });
      msgEl.classList.add("search-highlight");
      setTimeout(() => msgEl.classList.remove("search-highlight"), 2000);
    }
  });
}

async function handleConversationSelect(chatId: number): Promise<void> {
  const conv = getConversationById(chatId);
  if (!conv) return;

  const displayName = conv.display_name ?? conv.participants[0] ?? conv.chat_identifier;
  await loadConversation(chatId, displayName, conv.chat_identifier, conv.is_group);
}

function showError(message: string): void {
  const overlay = document.getElementById("error-overlay")!;
  overlay.textContent = message;
  overlay.classList.remove("hidden");
}

document.addEventListener("keydown", (e: KeyboardEvent) => {
  // Cmd+F — focus global search
  if (e.metaKey && !e.shiftKey && e.key === "f") {
    e.preventDefault();
    document.getElementById("global-search")?.focus();
  }
  // Cmd+Shift+F — focus in-chat search (Stage 3)
  if (e.metaKey && e.shiftKey && e.key === "f") {
    e.preventDefault();
    document.getElementById("btn-search-in-chat")?.click();
  }
});

document.addEventListener("DOMContentLoaded", init);
