import { createEffect, createSignal, on } from "solid-js";
import type { Accessor, Setter } from "solid-js";
import {
  pendingSessionSearch,
  setPendingSessionSearch,
} from "../../stores/search";
import type { ProcessedEntry } from "./hooks";
import {
  SESSION_SEARCH_DEBOUNCE_MS,
  findNewestMatchingEntryIndex,
  getMarksInVisualOrder,
} from "./search-utils";

export interface CreateSessionSearchOptions {
  /** Role-filtered entries the search runs against. */
  filteredEntries: Accessor<ProcessedEntry[]>;
  /** Lazy ref getter — the messages container may not exist yet. */
  getMessagesRef: () => HTMLDivElement | undefined;
  /** Whether the session is still loading (gates the pending-search effect). */
  loading: Accessor<boolean>;
  /** The current session id (matched against a pending global search). */
  sessionId: Accessor<string>;
  /** Load older message windows until the query can be resolved or exhausted. */
  loadUntilSearchMatch: (term: string) => Promise<number | null>;
  /** Register the debounce timer for cleanup by the owning component. */
  registerDebounce: (clear: () => void) => void;
}

export interface CreateSessionSearchResult {
  sessionSearch: Accessor<string>;
  setSessionSearch: Setter<string>;
  activeSessionSearch: Accessor<string>;
  searchFocusEntryIndex: Accessor<number | null>;
  searchBarOpen: Accessor<boolean>;
  setSearchBarOpen: Setter<boolean>;
  searchMatchIdx: Accessor<number>;
  setSearchMatchIdx: Setter<number>;
}

/**
 * Owns the in-session search slice of SessionView: the search query signals,
 * the active/focus state, the match count memo, and the two effects that
 * (1) consume a pending global search and (2) debounce typed queries. Bodies
 * are moved verbatim from the inline component so dependency tracking, the
 * debounce timing, and the `suppressNextSearchEffect` guard are unchanged.
 *
 * The debounce timer is owned here but its cleanup is registered back with the
 * component via `registerDebounce` so onCleanup stays in one place.
 */
export function createSessionSearch(
  opts: CreateSessionSearchOptions,
): CreateSessionSearchResult {
  const [sessionSearch, setSessionSearch] = createSignal("");
  const [activeSessionSearch, setActiveSessionSearch] = createSignal("");
  const [searchFocusEntryIndex, setSearchFocusEntryIndex] = createSignal<
    number | null
  >(null);
  const [searchBarOpen, setSearchBarOpen] = createSignal(false);
  const [searchMatchIdx, setSearchMatchIdx] = createSignal(0);

  let sessionSearchDebounce: ReturnType<typeof setTimeout> | undefined;
  let suppressNextSearchEffect = false;
  let searchRequestId = 0;
  opts.registerDebounce(() => clearTimeout(sessionSearchDebounce));

  function focusFirstRenderedSearchMatch() {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const messagesRef = opts.getMessagesRef();
        if (!messagesRef) return;
        // Activate the FIRST mark in visual order (top->bottom) — the same list
        // Next/Prev cycles and that searchMatchIdx=0 refers to — so the scroll
        // target, the active highlight, and the navigation cursor all agree on
        // the top match. (Previously this used querySelector = DOM order, which
        // under the column-reverse layout is the opposite end, so the highlight
        // disagreed with where the view scrolled and where Next started.)
        const first = getMarksInVisualOrder(messagesRef)[0];
        if (!first) return;
        messagesRef
          .querySelector("mark.search-active")
          ?.classList.remove("search-active");
        first.classList.add("search-active");
        first.scrollIntoView({ behavior: "smooth", block: "center" });
      });
    });
  }

  async function commitSessionSearch(raw: string) {
    const requestId = ++searchRequestId;
    const term = raw.trim();
    setSearchMatchIdx(0);
    if (!term) {
      setActiveSessionSearch("");
      setSearchFocusEntryIndex(null);
      return;
    }

    const entries = opts.filteredEntries();
    let matchIdx = findNewestMatchingEntryIndex(entries, term);
    setActiveSessionSearch(term);
    if (matchIdx < 0) {
      matchIdx = (await opts.loadUntilSearchMatch(term)) ?? -1;
    }
    if (requestId !== searchRequestId || term !== sessionSearch().trim()) {
      return;
    }
    setSearchFocusEntryIndex(matchIdx >= 0 ? matchIdx : null);
    focusFirstRenderedSearchMatch();
  }

  // Consume a pending session search set by the global SearchOverlay.
  // Runs after the session finishes loading; applies the query, opens the
  // in-session search bar, and scrolls to the first match.
  createEffect(() => {
    const pending = pendingSessionSearch();
    if (!pending || opts.loading()) return;
    if (pending.sessionId !== opts.sessionId()) return;
    setPendingSessionSearch(null);

    suppressNextSearchEffect = true;
    setSessionSearch(pending.query);
    setSearchBarOpen(true);
    void commitSessionSearch(pending.query);
  });

  createEffect(
    on(sessionSearch, (raw) => {
      clearTimeout(sessionSearchDebounce);
      if (suppressNextSearchEffect) {
        suppressNextSearchEffect = false;
        return;
      }
      if (!raw.trim()) {
        void commitSessionSearch("");
        return;
      }
      sessionSearchDebounce = setTimeout(
        () => void commitSessionSearch(raw),
        SESSION_SEARCH_DEBOUNCE_MS,
      );
    }),
  );

  return {
    sessionSearch,
    setSessionSearch,
    activeSessionSearch,
    searchFocusEntryIndex,
    searchBarOpen,
    setSearchBarOpen,
    searchMatchIdx,
    setSearchMatchIdx,
  };
}
