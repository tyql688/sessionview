import type { ProcessedEntry } from "./hooks";

export const SESSION_SEARCH_DEBOUNCE_MS = 180;
export const SEARCH_CONTEXT_ENTRIES = 20;
export const SEARCH_WINDOW_ENTRIES = 140;

export function normalizeSessionSearch(term: string): string {
  return term.trim().toLocaleLowerCase();
}

export function entrySearchText(entry: ProcessedEntry): string {
  if (entry.type === "message") {
    return entry.msg.content ?? "";
  }
  if (entry.type === "merged-tools") {
    return entry.messages
      .map((message) =>
        [message.tool_name, message.tool_input, message.content]
          .filter((value): value is string => !!value)
          .join("\n"),
      )
      .join("\n");
  }
  return "";
}

export function entryMatchesSearch(
  entry: ProcessedEntry,
  normalizedTerm: string,
): boolean {
  if (!normalizedTerm) return false;
  // `searchHaystack` is the lowercased text pre-computed in `processMessages`;
  // using it directly skips a `toLocaleLowerCase()` per entry per keystroke,
  // which dominates the in-session search cost on 4k-message sessions.
  return entry.searchHaystack.includes(normalizedTerm);
}

export function findNewestMatchingEntryIndex(
  entries: ProcessedEntry[],
  term: string,
): number {
  const normalizedTerm = normalizeSessionSearch(term);
  if (!normalizedTerm) return -1;

  for (let i = entries.length - 1; i >= 0; i--) {
    if (entryMatchesSearch(entries[i], normalizedTerm)) {
      return i;
    }
  }

  return -1;
}

export function countMatchingEntries(
  entries: ProcessedEntry[],
  term: string,
): number {
  const normalizedTerm = normalizeSessionSearch(term);
  if (!normalizedTerm) return 0;
  return entries.reduce(
    (count, entry) =>
      entryMatchesSearch(entry, normalizedTerm) ? count + 1 : count,
    0,
  );
}

export function searchWindowBounds(
  entriesLength: number,
  matchIndex: number,
): { start: number; end: number } | null {
  if (matchIndex < 0 || matchIndex >= entriesLength) {
    return null;
  }
  const windowSize = Math.min(entriesLength, SEARCH_WINDOW_ENTRIES);
  let start = Math.max(0, matchIndex - SEARCH_CONTEXT_ENTRIES);
  if (start + windowSize > entriesLength) {
    start = Math.max(0, entriesLength - windowSize);
  }
  return { start, end: start + windowSize };
}
