import { useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { setPendingSessionSearch, usePendingSessionSearch } from "@/features/search/search";
import { applySearchHighlight, buildMatchLocations, SESSION_SEARCH_DEBOUNCE_MS } from "@/features/session/search-utils";
import type { ProcessedEntry } from "@/features/session/hooks";

export interface CreateSessionSearchOptions {
  /** Role-filtered entries the search runs against. */
  filteredEntries: ProcessedEntry[];
  /** Whether the session is still loading (gates the pending-search effect). */
  loading: boolean;
  /** The current session id (matched against a pending global search). */
  sessionId: string;
  /** Load the complete searchable window and return the first matching entry. */
  resolveCompleteSearchMatch: (term: string) => Promise<number | null>;
  /** Scroll the virtualizer to an entry. */
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
  /** Entry index per occurrence, in session order — data-level, so the count
   * covers the whole loaded session, not just the mounted rows. */
  matchLocations: number[];
  navigateMatch: (delta: number) => void;
}

/**
 * Owns the in-session search slice of SessionView: the query signals, the
 * pending-global-search consumption, the typed-query debounce, and match
 * navigation.
 *
 * Matches are counted and navigated on entry data (`searchHaystack`), because
 * under virtualized rendering the DOM only ever holds the rows near the
 * viewport. Committing a query first pages in the complete session
 * (`resolveCompleteSearchMatch`), so the locations cover every message; the
 * DOM highlight paint runs separately in SessionView over whatever rows are
 * mounted.
 */
export function useSessionSearch(opts: CreateSessionSearchOptions): CreateSessionSearchResult {
  const [sessionSearch, setSessionSearch] = useState("");
  const [activeSessionSearch, setActiveSessionSearch] = useState("");
  const [searchBarOpen, setSearchBarOpen] = useState(false);
  const [searchMatchIdx, setSearchMatchIdx] = useState(0);

  const pending = usePendingSessionSearch();

  const sessionSearchRef = useRef(sessionSearch);
  sessionSearchRef.current = sessionSearch;

  const matchLocations = useMemo(
    () => buildMatchLocations(opts.filteredEntries, activeSessionSearch),
    [opts.filteredEntries, activeSessionSearch],
  );
  const matchLocationsRef = useRef(matchLocations);
  matchLocationsRef.current = matchLocations;
  const searchMatchIdxRef = useRef(searchMatchIdx);
  searchMatchIdxRef.current = searchMatchIdx;

  const sessionSearchDebounceRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const suppressNextSearchEffectRef = useRef(false);
  const searchRequestIdRef = useRef(0);
  useEffect(() => {
    opts.registerDebounce(() => clearTimeout(sessionSearchDebounceRef.current));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function commitSessionSearch(raw: string) {
    const requestId = ++searchRequestIdRef.current;
    const term = raw.trim();
    setSearchMatchIdx(0);
    if (!term) {
      setActiveSessionSearch("");
      applySearchHighlight([], null);
      return;
    }

    // Page in the complete session so the data-level match list is total.
    await opts.resolveCompleteSearchMatch(term);
    if (requestId !== searchRequestIdRef.current || term !== sessionSearchRef.current.trim()) {
      return;
    }
    setActiveSessionSearch(term);
  }

  // Reveal the first match once a committed query's locations are computed.
  // Runs on term change only — navigation moves searchMatchIdx separately.
  useEffect(() => {
    if (!activeSessionSearch) return;
    const first = matchLocationsRef.current[0];
    if (first !== undefined) {
      opts.revealEntry(first);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeSessionSearch]);

  function navigateMatch(delta: number) {
    const locations = matchLocationsRef.current;
    if (locations.length === 0) return;
    const next = (searchMatchIdxRef.current + delta + locations.length) % locations.length;
    setSearchMatchIdx(next);
    opts.revealEntry(locations[next]);
  }

  // Consume a pending session search set by the global SearchOverlay.
  // Runs after the session finishes loading; applies the query, opens the
  // in-session search bar, and scrolls to the first match.
  useEffect(() => {
    if (!pending || opts.loading) return;
    if (pending.sessionId !== opts.sessionId) return;
    setPendingSessionSearch(null);

    // Only arm the suppress flag when the state write actually changes the
    // value — an identical query re-runs no effect, and a stale flag would
    // swallow the user's next keystroke.
    suppressNextSearchEffectRef.current = pending.query !== sessionSearchRef.current;
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
    sessionSearchDebounceRef.current = setTimeout(() => void commitSessionSearch(raw), SESSION_SEARCH_DEBOUNCE_MS);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionSearch]);

  return {
    sessionSearch,
    setSessionSearch,
    activeSessionSearch,
    searchBarOpen,
    setSearchBarOpen,
    searchMatchIdx,
    matchLocations,
    navigateMatch,
  };
}
