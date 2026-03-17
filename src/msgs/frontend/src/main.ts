import { invoke } from "@tauri-apps/api/core";
import { initConversations } from "./conversations";

interface DbStatus {
  accessible: boolean;
  message_count: number | null;
  error: string | null;
}

async function init(): Promise<void> {
  const status = await invoke<DbStatus>("check_db_access");
  if (!status.accessible) {
    showError(status.error ?? "Cannot access message database");
    return;
  }
  await initConversations();
}

function showError(message: string): void {
  const overlay = document.getElementById("error-overlay")!;
  overlay.textContent = message;
  overlay.classList.remove("hidden");
}

document.addEventListener("DOMContentLoaded", init);
