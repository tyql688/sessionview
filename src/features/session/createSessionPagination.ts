import { useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { flushSync } from "react-dom";
import { useVirtualizer, type Virtualizer } from "@tanstack/react-virtual";
import { getSessionMessagesWindow, isLoadCanceledError } from "@/lib/tauri";
import type { Message, SessionMeta, TokenTotals } from "@/lib/types";
import { findFirstMatchingEntryIndex } from "@/features/session/search-utils";
import type { ProcessedEntry } from "@/features/session/hooks";

/** Backend page sizes: how many messages the initial open fetches and how
 * many each older-page fetch adds. Rendering no longer windows anything —
 * the virtualizer draws only the on-screen rows regardless of how many
 * messages are in memory — so these exist purely to bound a single IPC
 * payload. */
export const INITIAL_TAIL = 300;
const TAIL_BATCH = 600;

/** Fetch the next older page while this many rows of runway remain above the
 * viewport, so the prepend usually lands before the user reaches the top. */
const PREFETCH_ROW_RUNWAY = 60;

/** Pre-measurement row height estimate. Real heights replace it the moment a
 * row renders (measureElement); it only positions never-rendered rows. */
const ESTIMATED_ROW_PX = 120;

export interface CreateSessionPaginationOptions {
  /** Current session id (guards stale async results). */
  sessionId: string;
  /** Role-filtered entries the virtualizer renders. */
  filteredEntries: ProcessedEntry[];
  /** Absolute session index of messages[0] — owned by the component because
   * `processMessages` needs it before this hook can run. */
  windowStart: number;
  setWindowStart: Dispatch<SetStateAction<number>>;
  /** Scroll container (null until it mounts — state twin of the ref). */
  scrollElement: HTMLDivElement | null;
  setMessages: Dispatch<SetStateAction<Message[]>>;
  setMeta: Dispatch<SetStateAction<SessionMeta>>;
  /** Apply fresh token totals onto a meta object. */
  withTokenTotals: (metaData: SessionMeta, totals: TokenTotals) => SessionMeta;
}

export interface CreateSessionPaginationResult {
  virtualizer: Virtualizer<HTMLDivElement, Element>;
  totalMessages: number;
  setTotalMessages: Dispatch<SetStateAction<number>>;
  resolveCompleteSearchMatch: (term: string) => Promise<number | null>;
  revealEntry: (entryIndex: number) => void;
  revealMessageIndex: (messageIndex: number) => Promise<boolean>;
  scrollToEnd: () => void;
}

/**
 * Owns the virtualized-scrolling slice of SessionView.
 *
 * Rendering is fully virtual (@tanstack/react-virtual): only the rows near
 * the viewport exist in the DOM, absolutely positioned inside a fixed-height
 * spacer, so a row's height never pushes other rows around — scrolling is
 * symmetric in both directions and opening a session mounts one screenful,
 * not a whole tail.
 *
 * Message *loading* stays windowed (the backend pages the parsed session
 * over IPC): older pages are prepended as the viewport approaches the top of
 * the loaded window. Entry keys are built on absolute message indices, and
 * the virtualizer keys its measurements by entry key, so a prepend shifts
 * indices without invalidating any measured height; the scroll offset is
 * compensated by the spacer growth in the same synchronous flush.
 */
export function useSessionPagination(
  opts: CreateSessionPaginationOptions,
): CreateSessionPaginationResult {
  const [totalMessages, setTotalMessages] = useState(0);
  const { windowStart, setWindowStart } = opts;

  // Latest-value refs so async callbacks read current values across awaits
  // instead of the values captured when the closure was created.
  const filteredEntriesRef = useRef(opts.filteredEntries);
  filteredEntriesRef.current = opts.filteredEntries;
  const sessionIdRef = useRef(opts.sessionId);
  sessionIdRef.current = opts.sessionId;
  const windowStartRef = useRef(windowStart);
  windowStartRef.current = windowStart;
  const totalMessagesRef = useRef(totalMessages);
  totalMessagesRef.current = totalMessages;
  const scrollElementRef = useRef(opts.scrollElement);
  scrollElementRef.current = opts.scrollElement;

  const olderFetchInFlightRef = useRef(false);

  const virtualizer = useVirtualizer({
    count: opts.filteredEntries.length,
    getScrollElement: () => scrollElementRef.current,
    estimateSize: () => ESTIMATED_ROW_PX,
    // Measurements survive prepends: keys are stable per entry, not per index.
    getItemKey: (index) => opts.filteredEntries[index]?.key ?? index,
    overscan: 8,
    // Non-zero rect before the first element measurement (and in DOM-less
    // test environments) so the first render already mounts a screenful.
    initialRect: { width: 800, height: 800 },
  });

  /** Prepend an older page and keep the viewport glued to the rows the user
   * was reading: the flushSync commit grows the spacer above the viewport,
   * and the scroll offset moves by exactly that growth before paint. */
  function prependMessages(older: {
    messages: Message[];
    start: number;
    total: number;
    token_totals: TokenTotals;
  }) {
    const scroller = scrollElementRef.current;
    const prevTotalSize = virtualizer.getTotalSize();
    const prevScrollTop = scroller?.scrollTop ?? 0;
    flushSync(() => {
      opts.setMeta((prev) => opts.withTokenTotals(prev, older.token_totals));
      opts.setMessages((prev) => [...older.messages, ...prev]);
      setWindowStart(older.start);
      setTotalMessages(older.total);
    });
    if (scroller) {
      const delta = virtualizer.getTotalSize() - prevTotalSize;
      if (delta > 0) {
        scroller.scrollTop = prevScrollTop + delta;
      }
    }
  }

  async function loadOlderTail(): Promise<boolean> {
    if (olderFetchInFlightRef.current || windowStartRef.current <= 0) {
      return false;
    }
    const sessionId = sessionIdRef.current;
    olderFetchInFlightRef.current = true;
    const newStart = Math.max(0, windowStartRef.current - TAIL_BATCH);
    const span = windowStartRef.current - newStart;
    try {
      const older = await getSessionMessagesWindow(sessionId, newStart, span);
      if (sessionId !== sessionIdRef.current) return false;
      prependMessages(older);
      return older.messages.length > 0;
    } catch (e) {
      if (isLoadCanceledError(e)) return false;
      console.warn("load older messages failed:", e);
      return false;
    } finally {
      olderFetchInFlightRef.current = false;
    }
  }

  // Prefetch: when the rendered window nears the top of the loaded messages,
  // page in the next older batch. Runs off the virtualizer's own render
  // output — no scroll listener, no geometry reads.
  const virtualItems = virtualizer.getVirtualItems();
  const firstRenderedIndex = virtualItems[0]?.index ?? 0;
  useEffect(() => {
    if (windowStart <= 0) return;
    if (firstRenderedIndex > PREFETCH_ROW_RUNWAY) return;
    void loadOlderTail();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [firstRenderedIndex, windowStart]);

  function revealEntry(entryIndex: number) {
    const total = filteredEntriesRef.current.length;
    if (entryIndex < 0 || entryIndex >= total) return;
    virtualizer.scrollToIndex(entryIndex, { align: "center" });
  }

  async function revealMessageIndex(messageIndex: number): Promise<boolean> {
    if (messageIndex < 0 || messageIndex >= totalMessagesRef.current) {
      return false;
    }

    if (messageIndex < windowStartRef.current) {
      if (olderFetchInFlightRef.current) return false;
      const sessionId = sessionIdRef.current;
      olderFetchInFlightRef.current = true;
      try {
        const span = windowStartRef.current - messageIndex;
        const older = await getSessionMessagesWindow(
          sessionId,
          messageIndex,
          span,
        );
        if (sessionId !== sessionIdRef.current) return false;
        prependMessages(older);
      } catch (e) {
        if (isLoadCanceledError(e)) return false;
        console.warn("reveal message failed:", e);
        return false;
      } finally {
        olderFetchInFlightRef.current = false;
      }
    }

    const entries = filteredEntriesRef.current;
    let entryIndex = entries.findIndex(
      (entry) =>
        entry.type === "message" && entry.messageIndex === messageIndex,
    );
    if (entryIndex < 0) {
      // The exact message may be folded into a merged-tool row or filtered
      // out; land on the first entry at or after it instead of failing.
      entryIndex = entries.findIndex((entry) => {
        if (entry.type === "message") return entry.messageIndex >= messageIndex;
        if (entry.type === "merged-tools") {
          return entry.messageIndices.some((index) => index >= messageIndex);
        }
        return false;
      });
    }
    if (entryIndex < 0) return false;
    virtualizer.scrollToIndex(entryIndex, { align: "start" });
    return true;
  }

  async function loadAllOlderEntriesForSearch(): Promise<boolean> {
    if (olderFetchInFlightRef.current) return false;
    const start = windowStartRef.current;
    if (start <= 0) return true;

    const sessionId = sessionIdRef.current;
    olderFetchInFlightRef.current = true;
    try {
      const older = await getSessionMessagesWindow(sessionId, 0, start);
      if (sessionId !== sessionIdRef.current) return false;
      prependMessages(older);
      return older.start === 0;
    } catch (e) {
      if (isLoadCanceledError(e)) return false;
      console.warn("load complete session for search failed:", e);
      return false;
    } finally {
      olderFetchInFlightRef.current = false;
    }
  }

  async function resolveCompleteSearchMatch(
    term: string,
  ): Promise<number | null> {
    if (windowStartRef.current > 0) {
      const loadedCompleteWindow = await loadAllOlderEntriesForSearch();
      if (!loadedCompleteWindow) return null;
    }

    const matchIndex = findFirstMatchingEntryIndex(
      filteredEntriesRef.current,
      term,
    );
    return matchIndex >= 0 ? matchIndex : null;
  }

  function scrollToEnd() {
    const scrollLast = () => {
      const last = filteredEntriesRef.current.length - 1;
      if (last >= 0) virtualizer.scrollToIndex(last, { align: "end" });
    };
    scrollLast();
    // Dynamic row heights: the first pass lands on estimates; a second pass
    // after the mounted rows report real sizes settles the exact bottom.
    requestAnimationFrame(() => {
      requestAnimationFrame(scrollLast);
    });
  }

  return {
    virtualizer,
    totalMessages,
    setTotalMessages,
    resolveCompleteSearchMatch,
    revealEntry,
    revealMessageIndex,
    scrollToEnd,
  };
}
