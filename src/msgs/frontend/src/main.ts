import { invoke } from "@tauri-apps/api/core";
import type { DbStatus } from "./bindings";
import {
  initConversations,
  onConversationSelect,
  getConversationById,
} from "./conversations";
import { loadConversation } from "./messages";
import { initCacheStatus } from "./cache-status";
import { initSearch } from "./search";
import { messageOf, showError } from "./utils";

export async function init(): Promise<void> {
  try {
    const status = await invoke<DbStatus>("check_db_access");
    if (!status.accessible) {
      showError(status.error ?? "Cannot access message database");
      return;
    }

    onConversationSelect(handleConversationSelect);
    initSearch(handleSearchNavigate);
    await initCacheStatus();
    await initConversations();
  } catch (error) {
    showError(`msgs could not start: ${messageOf(error)}`);
  }
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

document.addEventListener("keydown", (e: KeyboardEvent) => {
  // Cmd+F — focus global search
  if (e.metaKey && !e.shiftKey && e.key.toLowerCase() === "f") {
    e.preventDefault();
    document.getElementById("global-search")?.focus();
  }
});

document.addEventListener("DOMContentLoaded", init);
