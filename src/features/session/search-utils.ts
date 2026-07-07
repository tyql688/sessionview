import type { ProcessedEntry } from "@/features/session/hooks";

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
 * document order — which, with the virtualized top-down timeline, is also
 * visual order. Only the rows currently mounted by the virtualizer are
 * scanned; match counting and navigation run on entry data instead
 * (`buildMatchLocations`), so the DOM walk stays a cheap paint-only pass.
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

  return ranges;
}

/** One element per occurrence of `term` across the loaded entries: the value
 * is the entry index the occurrence lives in. Counting runs on entry data —
 * not the DOM — so totals cover the whole loaded session even though the
 * virtualizer only mounts the rows near the viewport. */
export function buildMatchLocations(
  entries: ProcessedEntry[],
  term: string,
): number[] {
  const normalized = normalizeSessionSearch(term);
  if (!normalized) return [];
  const locations: number[] = [];
  entries.forEach((entry, entryIndex) => {
    const haystack = entry.searchHaystack;
    for (
      let at = haystack.indexOf(normalized);
      at !== -1;
      at = haystack.indexOf(normalized, at + normalized.length)
    ) {
      locations.push(entryIndex);
    }
  });
  return locations;
}

/** The active match, addressed as (entry index, nth occurrence within that
 * entry) — the shape the DOM paint pass needs to pick the right Range. */
export function activeMatchTarget(
  locations: number[],
  activeIdx: number,
): { entryIndex: number; occurrence: number } | null {
  const entryIndex = locations[activeIdx];
  if (entryIndex === undefined) return null;
  let occurrence = 0;
  for (let i = activeIdx - 1; i >= 0 && locations[i] === entryIndex; i -= 1) {
    occurrence += 1;
  }
  return { entryIndex, occurrence };
}

/** Paint highlights over the currently mounted rows and return the active
 * Range if it is mounted (for scroll-into-view). `active` addresses the
 * match by entry key + occurrence because Ranges are rebuilt from the live
 * DOM on every pass — virtual rows mount and unmount as the user scrolls. */
export function paintVisibleHighlights(
  container: HTMLElement | undefined,
  term: string,
  active: { entryKey: string; occurrence: number } | null,
): Range | null {
  const ranges = collectSearchRanges(container, term);
  let activeIndex: number | null = null;
  if (active && container) {
    const entryEl = container.querySelector(
      `[data-entry-key="${CSS.escape(active.entryKey)}"]`,
    );
    if (entryEl) {
      const inEntry = ranges
        .map((range, index) => ({ range, index }))
        .filter(({ range }) => entryEl.contains(range.startContainer));
      // The DOM shows rendered markdown while occurrences are counted over
      // the source text, so the nth source occurrence may not map 1:1 onto
      // the nth rendered Range — clamp instead of dropping the highlight.
      const target = inEntry[Math.min(active.occurrence, inEntry.length - 1)];
      if (target) activeIndex = target.index;
    }
  }
  applySearchHighlight(ranges, activeIndex);
  return activeIndex !== null ? (ranges[activeIndex] ?? null) : null;
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
