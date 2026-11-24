const ERROR_OVERLAY_ID = "error-overlay";

const HTML_ESCAPES: Readonly<Record<string, string>> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};

/** Escape the five characters that are special in HTML text and in quoted attributes. */
export function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (ch) => HTML_ESCAPES[ch] ?? ch);
}

/** Shown where a message or a chat has no date. */
export const UNKNOWN_DATE_LABEL = "Unknown date";

/**
 * Find an element by id and check its type.
 * @throws Error that names the id when the element is missing or is not a `ctor`.
 */
export function byId<T extends HTMLElement>(id: string, ctor: new () => T): T {
  const el = document.getElementById(id);
  if (el instanceof ctor) return el;
  throw new Error(`msgs: element #${id} is missing or is not a ${ctor.name}`);
}

/**
 * Show a message in the error overlay. Never throws: when the document has no
 * overlay, this makes one. The message goes in as text, never as markup.
 */
export function showError(message: string): void {
  let overlay = document.getElementById(ERROR_OVERLAY_ID);
  if (overlay === null) {
    overlay = document.createElement("div");
    overlay.id = ERROR_OVERLAY_ID;
    document.body.appendChild(overlay);
  }
  overlay.textContent = message;
  overlay.classList.remove("hidden");
}

/**
 * The text of an error of unknown type. Tauri rejects a command with the
 * message string, so a string is the message itself.
 */
export function messageOf(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
