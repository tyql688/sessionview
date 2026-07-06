import { useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import {
  setPendingSessionSearch,
  usePendingSessionSearch,
} from "../../stores/search";
import type { ProcessedEntry } from "./hooks";
import {
  SESSION_SEARCH_DEBOUNCE_MS,
  getMarksInVisualOrder,
} from "./search-utils";

export interface CreateSessionSearchOptions {
  /** Role-filtered entries the search runs against. */
  filteredEntries: ProcessedEntry[];
  /** Lazy ref getter — the messages container may not exist yet. */
  getMessagesRef: () => HTMLDivElement | undefined;
  /** Whether the session is still loading (gates the pending-search effect). */
  loading: boolean;
  /** The current session id (matched against a pending global search). */
  sessionId: string;
  /** Load the complete searchable window and return the first matching entry. */
  resolveCompleteSearchMatch: (term: string) => Promise<number | null>;
  /** Expand the normal render window until the matched entry is present. */
  revealEntry: (entryIndex: number) => void;
  /** Register the debounce timer for cleanup by the owning component. */
  registerDebounce: (clear: () => void) => void;
}

export interface CreateSessionSearchResult {
  sessionSearch: string;
  setSessionSearch: Dispatch<SetStateAction<string>>;
  activeSessionSearch: string;
  searchBarOpen: boolean;
  setSearchBarOpen: Dispatch<SetStateAction<boolean>>;
  searchMatchIdx: number;
  setSearchMatchIdx: Dispatch<SetStateAction<number>>;
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
 *
 * Now a React hook: call it at the top level of a component. Latest-value refs
 * back `sessionSearch`/`filteredEntries` so the debounced/awaited callbacks read
 * current values rather than a stale closure capture.
 */
export function useSessionSearch(
  opts: CreateSessionSearchOptions,
): CreateSessionSearchResult {
  const [sessionSearch, setSessionSearch] = useState("");
  const [activeSessionSearch, setActiveSessionSearch] = useState("");
  const [searchBarOpen, setSearchBarOpen] = useState(false);
  const [searchMatchIdx, setSearchMatchIdx] = useState(0);

  const pending = usePendingSessionSearch();

  const sessionSearchRef = useRef(sessionSearch);
  sessionSearchRef.current = sessionSearch;
  const filteredEntriesRef = useRef(opts.filteredEntries);
  filteredEntriesRef.current = opts.filteredEntries;

  const sessionSearchDebounceRef = useRef<
    ReturnType<typeof setTimeout> | undefined
  >(undefined);
  const suppressNextSearchEffectRef = useRef(false);
  const searchRequestIdRef = useRef(0);
  useEffect(() => {
    opts.registerDebounce(() => clearTimeout(sessionSearchDebounceRef.current));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function focusRenderedSearchMatch(entryKey: string | undefined) {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const messagesRef = opts.getMessagesRef();
        if (!messagesRef) return;
        const marks = getMarksInVisualOrder(messagesRef);
        const targetEntry = entryKey
          ? Array.from(
              messagesRef.querySelectorAll<HTMLElement>(".session-entry"),
            ).find((entry) => entry.dataset.entryKey === entryKey)
          : undefined;
        const target =
          (targetEntry && getMarksInVisualOrder(targetEntry)[0]) ?? marks[0];
        if (!target) return;

        messagesRef
          .querySelector("mark.search-active")
          ?.classList.remove("search-active");
        target.classList.add("search-active");
        const targetIndex = marks.indexOf(target);
        setSearchMatchIdx(targetIndex >= 0 ? targetIndex : 0);
        target.scrollIntoView({ behavior: "smooth", block: "center" });
      });
    });
  }

  async function commitSessionSearch(raw: string) {
    const requestId = ++searchRequestIdRef.current;
    const term = raw.trim();
    setSearchMatchIdx(0);
    if (!term) {
      setActiveSessionSearch("");
      return;
    }

    const matchIdx = (await opts.resolveCompleteSearchMatch(term)) ?? -1;
    if (
      requestId !== searchRequestIdRef.current ||
      term !== sessionSearchRef.current.trim()
    ) {
      return;
    }
    const targetEntry =
      matchIdx >= 0 ? filteredEntriesRef.current[matchIdx] : null;
    if (targetEntry) {
      opts.revealEntry(matchIdx);
    }
    setActiveSessionSearch(term);
    focusRenderedSearchMatch(targetEntry?.key);
  }

  // Consume a pending session search set by the global SearchOverlay.
  // Runs after the session finishes loading; applies the query, opens the
  // in-session search bar, and scrolls to the first match.
  useEffect(() => {
    if (!pending || opts.loading) return;
    if (pending.sessionId !== opts.sessionId) return;
    setPendingSessionSearch(null);

    suppressNextSearchEffectRef.current = true;
    setSessionSearch(pending.query);
    setSearchBarOpen(true);
    void commitSessionSearch(pending.query);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pending, opts.loading, opts.sessionId]);

  useEffect(() => {
    const raw = sessionSearch;
    clearTimeout(sessionSearchDebounceRef.current);
    if (suppressNextSearchEffectRef.current) {
      suppressNextSearchEffectRef.current = false;
      return;
    }
    if (!raw.trim()) {
      void commitSessionSearch("");
      return;
    }
    sessionSearchDebounceRef.current = setTimeout(
      () => void commitSessionSearch(raw),
      SESSION_SEARCH_DEBOUNCE_MS,
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionSearch]);

  return {
    sessionSearch,
    setSessionSearch,
    activeSessionSearch,
    searchBarOpen,
    setSearchBarOpen,
    searchMatchIdx,
    setSearchMatchIdx,
  };
}
