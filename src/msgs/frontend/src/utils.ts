export function escapeHtml(text: string): string {
  const div = document.createElement("div");
  div.textContent = text;
  return div.innerHTML;
}

/** Shown where a message or a chat has no date. */
export const UNKNOWN_DATE_LABEL = "Unknown date";
