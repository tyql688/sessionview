import type { ProcessedEntry } from "@/components/SessionView/hooks";

export const SESSION_SEARCH_DEBOUNCE_MS = 180;

function normalizeSessionSearch(term: string): string {
  return term.trim().toLocaleLowerCase();
}

function entryMatchesSearch(
  entry: ProcessedEntry,
  normalizedTerm: string,
): boolean {
  if (!normalizedTerm) return false;
  // `searchHaystack` is lowercased once at normalize time; using it directly
  // skips a `toLocaleLowerCase()` per entry per keystroke, which dominates
  // in-session search cost on 4k-message sessions.
  return entry.searchHaystack.includes(normalizedTerm);
}

export function findFirstMatchingEntryIndex(
  entries: ProcessedEntry[],
  term: string,
): number {
  const normalizedTerm = normalizeSessionSearch(term);
  if (!normalizedTerm) return -1;

  for (let i = 0; i < entries.length; i++) {
    if (entryMatchesSearch(entries[i], normalizedTerm)) {
      return i;
    }
  }

  return -1;
}

// --- DOM highlighting via the CSS Custom Highlight API -----------------------
//
// Markdown now renders through Streamdown, which offers no hook for injecting
// <mark> elements, so highlighting moved to the DOM layer: occurrences are
// collected as Ranges over the rendered text and painted with
// ::highlight(session-search) / ::highlight(session-search-active) (styled in
// session.css). Only subtrees tagged [data-searchable] participate — search
// covers user + assistant dialogue, not tool output or thinking.

const HIGHLIGHT_NAME = "session-search";
const ACTIVE_HIGHLIGHT_NAME = "session-search-active";

function supportsHighlightApi(): boolean {
  return typeof CSS !== "undefined" && "highlights" in CSS;
}

/**
 * All occurrences of `term` inside [data-searchable] subtrees, as Ranges in
 * VISUAL order (top→bottom). Sorting by bounding box is required because the
 * messages container uses `column-reverse`: DOM order is newest-first while
 * visual order is oldest-first.
 */
export function collectSearchRanges(
  container: HTMLElement | undefined,
  term: string,
): Range[] {
  const normalized = normalizeSessionSearch(term);
  if (!container || !normalized) return [];

  const ranges: Range[] = [];
  for (const scope of container.querySelectorAll("[data-searchable]")) {
    const walker = document.createTreeWalker(scope, NodeFilter.SHOW_TEXT);
    for (
      let node = walker.nextNode();
      node !== null;
      node = walker.nextNode()
    ) {
      const text = node.textContent;
      if (!text) continue;
      const lower = text.toLocaleLowerCase();
      let from = 0;
      for (
        let at = lower.indexOf(normalized, from);
        at !== -1;
        at = lower.indexOf(normalized, from)
      ) {
        const range = document.createRange();
        range.setStart(node, at);
        range.setEnd(node, at + normalized.length);
        ranges.push(range);
        from = at + normalized.length;
      }
    }
  }

  ranges.sort((a, b) => {
    const ra = a.getBoundingClientRect();
    const rb = b.getBoundingClientRect();
    return ra.top - rb.top || ra.left - rb.left;
  });
  return ranges;
}

/** Paint the collected ranges; `activeIndex` gets the distinct active style.
 * No-op when the Highlight API is unavailable (e.g. happy-dom in tests). */
export function applySearchHighlight(
  ranges: Range[],
  activeIndex: number | null,
): void {
  if (!supportsHighlightApi()) return;
  const rest = ranges.filter((_, i) => i !== activeIndex);
  CSS.highlights.set(HIGHLIGHT_NAME, new Highlight(...rest));
  if (activeIndex !== null && ranges[activeIndex]) {
    CSS.highlights.set(
      ACTIVE_HIGHLIGHT_NAME,
      new Highlight(ranges[activeIndex]),
    );
  } else {
    CSS.highlights.delete(ACTIVE_HIGHLIGHT_NAME);
  }
}

/** Scroll a range's nearest element into view. */
export function scrollRangeIntoView(range: Range): void {
  const node = range.startContainer;
  const element = node instanceof Element ? node : (node.parentElement ?? null);
  element?.scrollIntoView({ behavior: "smooth", block: "center" });
}
