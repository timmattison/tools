export function escapeHtml(text: string): string {
  const div = document.createElement("div");
  div.textContent = text;
  return div.innerHTML;
}

/** Shown where a message or a chat has no date. */
export const UNKNOWN_DATE_LABEL = "Unknown date";

/** Stub: the real lookup arrives in the green commit. */
export function byId<T extends HTMLElement>(id: string, ctor: new () => T): T {
  const el = document.getElementById(id);
  if (el instanceof ctor) return el;
  throw new Error("not implemented");
}

/** Stub: the real overlay arrives in the green commit. */
export function showError(_message: string): void {}
