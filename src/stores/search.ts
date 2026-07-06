import { create } from "zustand";
import { searchSessions } from "../lib/tauri";
import type { SearchFilters, SearchResult } from "../lib/types";
import { toastError } from "./toast";

// Pending in-session search: set by the global search overlay when opening a
// session so SessionView can auto-apply the query and scroll to the first match.
interface PendingSessionSearch {
  sessionId: string;
  query: string;
}

interface SearchState {
  query: string;
  results: SearchResult[];
  isSearching: boolean;
  pendingSessionSearch: PendingSessionSearch | null;
}

export const useSearchStore = create<SearchState>(() => ({
  query: "",
  results: [],
  isSearching: false,
  pendingSessionSearch: null,
}));

let debounceTimer: ReturnType<typeof setTimeout>;
let searchVersion = 0;

export function parseSearchQuery(raw: string): SearchFilters {
  let remaining = raw;
  let after: number | undefined;
  let before: number | undefined;

  const extract = (prefix: string): string | undefined => {
    const regex = new RegExp(`${prefix}:(\\S+)`, "i");
    const match = remaining.match(regex);
    if (match) {
      remaining = remaining.replace(match[0], "").trim();
      return match[1];
    }
    return undefined;
  };

  const provider = extract("provider");
  const project = extract("project");

  const afterStr = extract("after");
  if (afterStr) {
    const d = Date.parse(afterStr);
    if (!Number.isNaN(d)) {
      after = Math.floor(d / 1000);
    }
  }

  const beforeStr = extract("before");
  if (beforeStr) {
    const d = Date.parse(beforeStr);
    if (!Number.isNaN(d)) {
      before = Math.floor(d / 1000);
    }
  }

  return {
    query: remaining.trim(),
    provider,
    project,
    after,
    before,
  };
}

export function search(q: string) {
  useSearchStore.setState({ query: q });
  clearTimeout(debounceTimer);
  if (!q.trim()) {
    // Empty query — clear results explicitly so the panel doesn't keep
    // showing matches from an abandoned query.
    useSearchStore.setState({ results: [], isSearching: false });
    return;
  }
  // Keep previous results visible during the debounce window so the panel
  // doesn't flash empty between keystrokes; `searchVersion` already discards
  // stale responses when they land out of order.
  useSearchStore.setState({ isSearching: true });
  const version = ++searchVersion;
  debounceTimer = setTimeout(async () => {
    try {
      const filters = parseSearchQuery(q);
      const r = await searchSessions(filters);
      if (version !== searchVersion) return; // stale response, discard
      useSearchStore.setState({ results: r });
    } catch (e) {
      if (version !== searchVersion) return;
      toastError(String(e));
      useSearchStore.setState({ results: [] });
    } finally {
      if (version === searchVersion) {
        useSearchStore.setState({ isSearching: false });
      }
    }
  }, 300);
}

export function clearSearch() {
  useSearchStore.setState({ query: "", results: [], isSearching: false });
  clearTimeout(debounceTimer);
}

export function setPendingSessionSearch(value: PendingSessionSearch | null) {
  useSearchStore.setState({ pendingSessionSearch: value });
}

export function getPendingSessionSearch(): PendingSessionSearch | null {
  return useSearchStore.getState().pendingSessionSearch;
}

// Reactive hooks for components.
export function useSearchQuery(): string {
  return useSearchStore((state) => state.query);
}

export function useSearchResults(): SearchResult[] {
  return useSearchStore((state) => state.results);
}

export function useIsSearching(): boolean {
  return useSearchStore((state) => state.isSearching);
}

export function usePendingSessionSearch(): PendingSessionSearch | null {
  return useSearchStore((state) => state.pendingSessionSearch);
}
