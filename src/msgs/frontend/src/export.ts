import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import type { ExportResult } from "./types";

let selectionMode = false;
let startMessageEl: HTMLElement | null = null;
let endMessageEl: HTMLElement | null = null;
let startDate: string | null = null;
let endDate: string | null = null;
let currentExportChatId: number | null = null;

let initialized = false;

export function initExport(chatId: number): void {
  currentExportChatId = chatId;
  if (initialized) return;
  initialized = true;

  const exportBtn = document.getElementById("btn-export")!;
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

  const date = el.dataset.messageDate!;

  if (!startMessageEl) {
    startMessageEl = el;
    startDate = date;
    el.classList.add("export-start");
    updateExportBar();
  } else if (!endMessageEl) {
    endMessageEl = el;
    endDate = date;

    // Ensure start is before end (compare as epoch timestamps, not strings)
    if (new Date(startDate!).getTime() > new Date(endDate!).getTime()) {
      [startMessageEl, endMessageEl] = [endMessageEl, startMessageEl];
      [startDate, endDate] = [endDate, startDate];
    }

    // Highlight range
    highlightRange();
    updateExportBar();
  } else {
    // Reset and start new selection
    clearHighlights();
    startMessageEl = el;
    startDate = date;
    endMessageEl = null;
    endDate = null;
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
  const status = document.getElementById("export-status")!;

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
  if (!startDate || !endDate || !currentExportChatId) return;

  const dir = await save({
    title: "Choose export location",
    defaultPath: `msgs-export-${new Date().toISOString().slice(0, 10)}`,
  });

  if (!dir) return;

  try {
    const result = await invoke<ExportResult>("export_messages", {
      chatId: currentExportChatId,
      startMessageDate: dateToAppleTimestamp(startDate),
      endMessageDate: dateToAppleTimestamp(endDate),
      includeAttachments: true,
      exportPath: dir,
    });

    const status = document.getElementById("export-status")!;
    status.textContent = `Exported ${result.message_count} messages and ${result.attachment_count} attachments to ${result.export_path}`;

    setTimeout(cancelSelection, 3000);
  } catch (e) {
    const status = document.getElementById("export-status")!;
    status.textContent = `Export failed: ${e}`;
  }
}

function cancelSelection(): void {
  selectionMode = false;
  startMessageEl = null;
  endMessageEl = null;
  startDate = null;
  endDate = null;
  clearHighlights();
  document.getElementById("export-bar")?.classList.add("hidden");
  document.getElementById("message-area")?.classList.remove("selecting");
}

function dateToAppleTimestamp(isoDate: string): number {
  const unixSeconds = Math.floor(new Date(isoDate).getTime() / 1000);
  return (unixSeconds - 978307200) * 1000000000;
}
