import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import type { ExportResult } from "./bindings";
import { byId } from "./utils";

let selectionMode = false;
let startMessageEl: HTMLElement | null = null;
let endMessageEl: HTMLElement | null = null;
let currentExportChatId: number | null = null;
/** True while an export runs. A second call of `performExport` returns at once. */
let exporting = false;
/** The id of the timer that ends selection mode after an export, or null. */
let closeTimer: ReturnType<typeof setTimeout> | null = null;
/** How long the result of an export stays on the bar. */
const RESULT_DISPLAY_MS = 3000;

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

function clearCloseTimer(): void {
  if (closeTimer !== null) {
    clearTimeout(closeTimer);
    closeTimer = null;
  }
}

function toggleSelectionMode(): void {
  clearCloseTimer();
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
  // The user started a new selection: the timer must not end it
  clearCloseTimer();

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
  }
  setConfirmDisabled(!startMessageEl || !endMessageEl || exporting);
}

function setConfirmDisabled(disabled: boolean): void {
  const confirmBtn = document.getElementById("btn-export-confirm");
  if (disabled) confirmBtn?.setAttribute("disabled", "");
  else confirmBtn?.removeAttribute("disabled");
}

async function performExport(): Promise<void> {
  if (exporting) return;
  const start = startMessageEl;
  const end = endMessageEl;
  if (!start || !end || !currentExportChatId) return;

  const startId = start.dataset.messageId;
  const endId = end.dataset.messageId;
  if (startId === undefined || endId === undefined) return;

  exporting = true;
  setConfirmDisabled(true);
  let succeeded = false;
  try {
    const dir = await save({
      title: "Choose export location",
      defaultPath: `msgs-export-${new Date().toISOString().slice(0, 10)}`,
    });
    if (!dir) return;

    const result = await invoke<ExportResult>("export_messages", {
      chatId: currentExportChatId,
      startMessageId: Number(startId),
      endMessageId: Number(endId),
      includeAttachments: true,
      exportPath: dir,
    });
    succeeded = true;

    // The user left this selection while the export ran: say nothing
    if (startMessageEl !== start || endMessageEl !== end) return;

    const status = byId("export-status", HTMLElement);
    status.textContent = `Exported ${result.message_count} messages and ${result.attachment_count} attachments to ${result.export_path}`;

    clearCloseTimer();
    closeTimer = setTimeout(() => {
      closeTimer = null;
      cancelSelection();
    }, RESULT_DISPLAY_MS);
  } catch (e) {
    if (startMessageEl === start && endMessageEl === end) {
      byId("export-status", HTMLElement).textContent = `Export failed: ${e}`;
    }
  } finally {
    exporting = false;
    if (startMessageEl !== start || endMessageEl !== end) {
      // The user changed the selection while the export ran: the button
      // follows the new selection
      if (selectionMode) updateExportBar();
    } else if (!succeeded) {
      // After a success the button stays disabled until the selection changes
      setConfirmDisabled(false);
    }
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
  clearCloseTimer();
  selectionMode = false;
  startMessageEl = null;
  endMessageEl = null;
  clearHighlights();
  setConfirmDisabled(true);
  document.getElementById("export-bar")?.classList.add("hidden");
  document.getElementById("message-area")?.classList.remove("selecting");
}
