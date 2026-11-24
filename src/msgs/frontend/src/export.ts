import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import type { ExportResult } from "./bindings";
import { byId } from "./utils";

let selectionMode = false;
let startMessageEl: HTMLElement | null = null;
let endMessageEl: HTMLElement | null = null;
let currentExportChatId: number | null = null;

let initialized = false;

export function initExport(chatId: number): void {
  currentExportChatId = chatId;
  if (initialized) return;
  initialized = true;

  const exportBtn = byId("btn-export", HTMLElement);
  exportBtn.addEventListener("click", toggleSelectionMode);

  const cancelBtn = document.getElementById("btn-export-cancel");
  cancelBtn?.addEventListener("click", cancelSelection);

  const confirmBtn = document.getElementById("btn-export-confirm");
  confirmBtn?.addEventListener("click", performExport);

  // Escape to cancel
  document.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Escape" && selectionMode) {
      cancelSelection();
    }
  });
}

function toggleSelectionMode(): void {
  selectionMode = !selectionMode;
  if (selectionMode) {
    document.getElementById("export-bar")?.classList.remove("hidden");
    document.getElementById("message-area")?.classList.add("selecting");
    updateExportBar();
  } else {
    cancelSelection();
  }
}

export function handleMessageClick(el: HTMLElement): void {
  if (!selectionMode) return;

  if (!startMessageEl) {
    startMessageEl = el;
    el.classList.add("export-start");
    updateExportBar();
  } else if (!endMessageEl) {
    endMessageEl = el;

    // Ensure start is before end, by position in the list
    if (
      startMessageEl.compareDocumentPosition(endMessageEl) &
      Node.DOCUMENT_POSITION_PRECEDING
    ) {
      [startMessageEl, endMessageEl] = [endMessageEl, startMessageEl];
    }

    // Highlight range
    highlightRange();
    updateExportBar();
  } else {
    // Reset and start new selection
    clearHighlights();
    startMessageEl = el;
    endMessageEl = null;
    el.classList.add("export-start");
    updateExportBar();
  }
}

function highlightRange(): void {
  clearHighlights();
  if (!startMessageEl || !endMessageEl) return;

  const messages = document.querySelectorAll<HTMLElement>(".message");
  let inRange = false;

  for (const msg of messages) {
    if (msg === startMessageEl) {
      inRange = true;
    }
    if (inRange) {
      msg.classList.add("export-selected");
    }
    if (msg === endMessageEl) {
      inRange = false;
    }
  }
}

function clearHighlights(): void {
  document.querySelectorAll(".export-selected, .export-start").forEach((el) => {
    el.classList.remove("export-selected", "export-start");
  });
}

function updateExportBar(): void {
  const status = byId("export-status", HTMLElement);

  if (!startMessageEl) {
    status.textContent = "Click a message to set the start of the export range";
  } else if (!endMessageEl) {
    status.textContent = "Click another message to set the end of the range";
  } else {
    const count = document.querySelectorAll(".export-selected").length;
    status.textContent = `${count} messages selected`;
    document.getElementById("btn-export-confirm")?.removeAttribute("disabled");
  }
}

async function performExport(): Promise<void> {
  if (!startMessageEl || !endMessageEl || !currentExportChatId) return;

  const startId = startMessageEl.dataset.messageId;
  const endId = endMessageEl.dataset.messageId;
  if (startId === undefined || endId === undefined) return;

  const dir = await save({
    title: "Choose export location",
    defaultPath: `msgs-export-${new Date().toISOString().slice(0, 10)}`,
  });

  if (!dir) return;

  try {
    const result = await invoke<ExportResult>("export_messages", {
      chatId: currentExportChatId,
      startMessageId: Number(startId),
      endMessageId: Number(endId),
      includeAttachments: true,
      exportPath: dir,
    });

    const status = byId("export-status", HTMLElement);
    status.textContent = `Exported ${result.message_count} messages and ${result.attachment_count} attachments to ${result.export_path}`;

    setTimeout(cancelSelection, 3000);
  } catch (e) {
    const status = byId("export-status", HTMLElement);
    status.textContent = `Export failed: ${e}`;
  }
}

/**
 * Leave selection mode and forget the selection: hide the export bar, clear
 * the marks, and disable the confirm button. Call it when the chat changes.
 */
export function resetExport(): void {
  cancelSelection();
}

function cancelSelection(): void {
  selectionMode = false;
  startMessageEl = null;
  endMessageEl = null;
  clearHighlights();
  document.getElementById("btn-export-confirm")?.setAttribute("disabled", "");
  document.getElementById("export-bar")?.classList.add("hidden");
  document.getElementById("message-area")?.classList.remove("selecting");
}
