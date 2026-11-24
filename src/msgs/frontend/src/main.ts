import { invoke } from "@tauri-apps/api/core";
import type { Conversation, DbStatus, SearchResult } from "./bindings";
import {
  initConversations,
  onConversationSelect,
  getConversationById,
  highlightConversationById,
} from "./conversations";
import { loadConversation, openConversationAt } from "./messages";
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

/** Open the chat of a search result around the hit. Needs no sidebar item for the chat. */
async function handleSearchNavigate(result: SearchResult): Promise<void> {
  const conv = result.conversation;
  highlightConversationById(conv.chat_id);
  await openConversationAt(
    conv.chat_id,
    titleOf(conv),
    conv.chat_identifier,
    conv.is_group,
    result.message.message_id
  );
}

function titleOf(conv: Conversation): string {
  return conv.display_name ?? conv.participants[0] ?? conv.chat_identifier;
}

async function handleConversationSelect(chatId: number): Promise<void> {
  const conv = getConversationById(chatId);
  if (!conv) return;

  await loadConversation(chatId, titleOf(conv), conv.chat_identifier, conv.is_group);
}

document.addEventListener("keydown", (e: KeyboardEvent) => {
  // Cmd+F — focus global search
  if (e.metaKey && !e.shiftKey && e.key.toLowerCase() === "f") {
    e.preventDefault();
    document.getElementById("global-search")?.focus();
  }
});

document.addEventListener("DOMContentLoaded", init);
