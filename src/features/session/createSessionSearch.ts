import { useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { setPendingSessionSearch, usePendingSessionSearch } from "@/features/search/search";
import { isSystemContent } from "@/features/session/hooks";
import {
  activeMatchTarget,
  applySearchHighlight,
  buildMatchLocations,
  SESSION_SEARCH_DEBOUNCE_MS,
  type MatchTarget,
  type SearchableMessage,
} from "@/features/session/search-utils";
import { useI18n } from "@/i18n/index";
import { getSessionSearchText } from "@/lib/tauri";
import type { MessageRole } from "@/lib/types";
import { toastError } from "@/stores/toast";

export interface CreateSessionSearchOptions {
  /** Roles hidden in the filter toolbar; their messages are not searched. */
  hiddenRoles: ReadonlySet<MessageRole>;
  /** Whether the session is still loading (gates the pending-search effect). */
  loading: boolean;
  /** The current session id (matched against a pending global search). */
  sessionId: string;
  /** Session message count; growing past the searchable snapshot refreshes it. */
  totalMessages: number;
  /** Scroll a match of `term` into view, loading its message first. */
  revealMatch: (term: string, target: MatchTarget) => Promise<boolean>;
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
  /** Absolute message index per occurrence, in session order — counted over
   * the whole session, not just the loaded window or the mounted rows. */
  matchLocations: number[];
  navigateMatch: (delta: number) => void;
}

interface SearchableSnapshot {
  sessionId: string;
  total: number;
  messages: SearchableMessage[];
}

/**
 * Owns the in-session search slice of SessionView: the query signals, the
 * pending-global-search consumption, the typed-query debounce, and match
 * navigation.
 *
 * Matches are counted on the session's searchable text — the user/assistant
 * dialogue, fetched on the first committed query — so the count covers the
 * whole session while the timeline keeps loading one window. Navigating
 * reveals the match (`revealMatch`); the DOM highlight paint runs separately
 * in SessionView over whatever rows are mounted.
 */
export function useSessionSearch(opts: CreateSessionSearchOptions): CreateSessionSearchResult {
  const { t } = useI18n();
  const [sessionSearch, setSessionSearch] = useState("");
  const [activeSessionSearch, setActiveSessionSearch] = useState("");
  const [searchBarOpen, setSearchBarOpen] = useState(false);
  const [searchMatchIdx, setSearchMatchIdx] = useState(0);
  const [searchable, setSearchable] = useState<SearchableSnapshot | null>(null);

  const pending = usePendingSessionSearch();

  const sessionSearchRef = useRef(sessionSearch);
  sessionSearchRef.current = sessionSearch;
  const optsRef = useRef(opts);
  optsRef.current = opts;
  const searchableRef = useRef(searchable);
  searchableRef.current = searchable;

  const matchLocations = useMemo(
    () =>
      searchable?.sessionId === opts.sessionId
        ? buildMatchLocations(searchable.messages, activeSessionSearch, opts.hiddenRoles)
        : [],
    [searchable, opts.sessionId, activeSessionSearch, opts.hiddenRoles],
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

  // The search-text fetch in flight. Every caller that needs the snapshot
  // meanwhile shares it, so typing on never starts another whole-session read.
  const searchableFetchRef = useRef<{ sessionId: string; promise: Promise<boolean> } | null>(null);

  /** Fetch the searchable text unless the held snapshot covers the current
   * session at its current size, joining a fetch already in flight. False when
   * it could not be fetched. */
  function ensureSearchable(): Promise<boolean> {
    const { sessionId, totalMessages } = optsRef.current;
    const held = searchableRef.current;
    if (held?.sessionId === sessionId && held.total === totalMessages) return Promise.resolve(true);
    const inFlight = searchableFetchRef.current;
    if (inFlight?.sessionId === sessionId) return inFlight.promise;
    const promise = fetchSearchable(sessionId).finally(() => {
      if (searchableFetchRef.current?.promise === promise) searchableFetchRef.current = null;
    });
    searchableFetchRef.current = { sessionId, promise };
    return promise;
  }

  async function fetchSearchable(sessionId: string): Promise<boolean> {
    try {
      const text = await getSessionSearchText(sessionId);
      if (sessionId !== optsRef.current.sessionId) return false;
      const snapshot: SearchableSnapshot = {
        sessionId,
        total: text.total,
        // The timeline hides injected system content, so search skips it too.
        messages: text.messages
          .filter((message) => !isSystemContent(message))
          .map((message) => ({
            messageIndex: message.message_index,
            role: message.role,
            haystack: message.content.toLocaleLowerCase(),
          })),
      };
      searchableRef.current = snapshot;
      setSearchable(snapshot);
      return true;
    } catch (error) {
      console.error("Failed to load session search text:", error);
      toastError(t("toast.sessionSearchFailed"));
      return false;
    }
  }

  async function commitSessionSearch(raw: string) {
    const requestId = ++searchRequestIdRef.current;
    const term = raw.trim();
    setSearchMatchIdx(0);
    if (!term) {
      setActiveSessionSearch("");
      applySearchHighlight([], null);
      return;
    }

    const ready = await ensureSearchable();
    if (requestId !== searchRequestIdRef.current || term !== sessionSearchRef.current.trim()) {
      return;
    }
    const snapshot = searchableRef.current;
    if (!ready || !snapshot) {
      // Without the text there is no honest match count: commit nothing
      // rather than show "No matches".
      setActiveSessionSearch("");
      return;
    }
    setActiveSessionSearch(term);
    // Every commit lands on the first match — also when the term is unchanged
    // (a re-run from global search), since the index was just reset.
    const first = buildMatchLocations(snapshot.messages, term, optsRef.current.hiddenRoles)[0];
    if (first !== undefined) void optsRef.current.revealMatch(term, { messageIndex: first, occurrence: 0 });
  }

  // A live session grew past the searchable snapshot: refresh it so the new
  // messages count too.
  useEffect(() => {
    if (!activeSessionSearch || !searchable || searchable.total === opts.totalMessages) return;
    void ensureSearchable();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opts.totalMessages]);

  function navigateMatch(delta: number) {
    const locations = matchLocationsRef.current;
    if (locations.length === 0) return;
    const next = (searchMatchIdxRef.current + delta + locations.length) % locations.length;
    setSearchMatchIdx(next);
    const target = activeMatchTarget(locations, next);
    if (target) void opts.revealMatch(activeSessionSearch, target);
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
