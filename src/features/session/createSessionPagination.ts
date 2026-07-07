import { useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { flushSync } from "react-dom";
import { useVirtualizer, type Virtualizer } from "@tanstack/react-virtual";
import { cancelSessionLoad, getSessionMessagesWindow, isLoadCanceledError } from "@/lib/tauri";
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
  /** Number of currently loaded messages (window end = start + count). */
  loadedCount: number;
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
 * over IPC), in BOTH directions: older pages prepend as the viewport nears
 * the top of the loaded window, newer pages append as it nears the bottom.
 * A jump far outside the window (minimap tick, first-turn reveal) does NOT
 * load everything in between — it re-centers the window around the target,
 * which costs the same as opening the session. Entry keys are built on
 * absolute message indices, and the virtualizer keys its measurements by
 * entry key, so a prepend shifts indices without invalidating any measured
 * height; the scroll offset is compensated by the spacer growth in the same
 * synchronous flush. Only a committed in-session search loads the complete
 * session (counting must cover every message).
 */
export function useSessionPagination(opts: CreateSessionPaginationOptions): CreateSessionPaginationResult {
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
  const loadedCountRef = useRef(opts.loadedCount);
  loadedCountRef.current = opts.loadedCount;
  const scrollElementRef = useRef(opts.scrollElement);
  scrollElementRef.current = opts.scrollElement;

  const windowFetchInFlightRef = useRef(false);
  const windowRequestSeqRef = useRef(0);
  const activeWindowRequestRef = useRef<{
    sessionId: string;
    requestId: string;
  } | null>(null);
  // Edge prefetch is armed only after the view has been positioned
  // (scroll-to-end on open, or a reveal jump). Before that, the virtualizer
  // renders from offset 0 and the "near the top" check would fire a spurious
  // older-page fetch on every open.
  const positionedRef = useRef(false);
  useEffect(() => {
    positionedRef.current = false;
  }, [opts.sessionId]);

  useEffect(() => {
    return () => {
      const request = activeWindowRequestRef.current;
      if (request) {
        void cancelSessionLoad(request.sessionId, request.requestId).catch((error) => {
          console.warn("cancelSessionLoad failed:", error);
        });
      }
    };
  }, [opts.sessionId]);

  function beginWindowRequest(kind: string): {
    sessionId: string;
    requestId: string;
  } {
    const sessionId = sessionIdRef.current;
    const requestId = `${sessionId}:window:${kind}:${++windowRequestSeqRef.current}`;
    activeWindowRequestRef.current = { sessionId, requestId };
    return { sessionId, requestId };
  }

  function finishWindowRequest(requestId: string) {
    if (activeWindowRequestRef.current?.requestId === requestId) {
      activeWindowRequestRef.current = null;
    }
  }

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
  function prependMessages(older: { messages: Message[]; start: number; total: number; token_totals: TokenTotals }) {
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
    if (windowFetchInFlightRef.current || windowStartRef.current <= 0) {
      return false;
    }
    const request = beginWindowRequest("older");
    windowFetchInFlightRef.current = true;
    const newStart = Math.max(0, windowStartRef.current - TAIL_BATCH);
    const span = windowStartRef.current - newStart;
    try {
      const older = await getSessionMessagesWindow(request.sessionId, newStart, span, request.requestId);
      if (request.sessionId !== sessionIdRef.current) return false;
      prependMessages(older);
      return older.messages.length > 0;
    } catch (e) {
      if (isLoadCanceledError(e)) return false;
      console.warn("load older messages failed:", e);
      return false;
    } finally {
      finishWindowRequest(request.requestId);
      windowFetchInFlightRef.current = false;
    }
  }

  async function loadNewerTail(): Promise<boolean> {
    const end = windowStartRef.current + loadedCountRef.current;
    if (windowFetchInFlightRef.current || end >= totalMessagesRef.current) {
      return false;
    }
    const request = beginWindowRequest("newer");
    windowFetchInFlightRef.current = true;
    try {
      const newer = await getSessionMessagesWindow(request.sessionId, end, TAIL_BATCH, request.requestId);
      if (request.sessionId !== sessionIdRef.current) return false;
      // Appending below the viewport never moves visible content in a
      // top-down layout — no flushSync, no scroll compensation needed.
      opts.setMeta((prev) => opts.withTokenTotals(prev, newer.token_totals));
      opts.setMessages((prev) => [...prev, ...newer.messages]);
      setTotalMessages(newer.total);
      return newer.messages.length > 0;
    } catch (e) {
      if (isLoadCanceledError(e)) return false;
      console.warn("load newer messages failed:", e);
      return false;
    } finally {
      finishWindowRequest(request.requestId);
      windowFetchInFlightRef.current = false;
    }
  }

  // Prefetch: when the rendered window nears either edge of the loaded
  // messages, page in the next batch on that side. Runs off the
  // virtualizer's own render output — no scroll listener, no geometry reads.
  const virtualItems = virtualizer.getVirtualItems();
  const firstRenderedIndex = virtualItems[0]?.index ?? 0;
  const lastRenderedIndex = virtualItems[virtualItems.length - 1]?.index ?? 0;
  const entryCount = opts.filteredEntries.length;
  useEffect(() => {
    if (!positionedRef.current) return;
    if (windowStart <= 0) return;
    if (firstRenderedIndex > PREFETCH_ROW_RUNWAY) return;
    void loadOlderTail();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [firstRenderedIndex, windowStart]);
  useEffect(() => {
    if (!positionedRef.current) return;
    if (windowStart + opts.loadedCount >= totalMessages) return;
    if (entryCount - 1 - lastRenderedIndex > PREFETCH_ROW_RUNWAY) return;
    void loadNewerTail();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lastRenderedIndex, entryCount, windowStart, opts.loadedCount, totalMessages]);

  function revealEntry(entryIndex: number) {
    const total = filteredEntriesRef.current.length;
    if (entryIndex < 0 || entryIndex >= total) return;
    positionedRef.current = true;
    virtualizer.scrollToIndex(entryIndex, { align: "center" });
  }

  /** Re-center the loaded window around a target message, REPLACING the
   * current window. A far jump (minimap tick near the top of a 13k-message
   * session) must not page in everything in between — re-centering costs the
   * same IPC as opening the session, and the discarded rows reload on demand
   * if the user scrolls back. */
  async function recenterWindowAround(messageIndex: number): Promise<boolean> {
    if (windowFetchInFlightRef.current) return false;
    const request = beginWindowRequest("recenter");
    windowFetchInFlightRef.current = true;
    try {
      const start = Math.max(0, messageIndex - Math.floor(INITIAL_TAIL / 2));
      const window = await getSessionMessagesWindow(request.sessionId, start, INITIAL_TAIL, request.requestId);
      if (request.sessionId !== sessionIdRef.current) return false;
      // flushSync so the entry lookup below sees the new window.
      flushSync(() => {
        opts.setMeta((prev) => opts.withTokenTotals(prev, window.token_totals));
        opts.setMessages(window.messages);
        setWindowStart(window.start);
        setTotalMessages(window.total);
      });
      return true;
    } catch (e) {
      if (isLoadCanceledError(e)) return false;
      console.warn("recenter window failed:", e);
      return false;
    } finally {
      finishWindowRequest(request.requestId);
      windowFetchInFlightRef.current = false;
    }
  }

  async function revealMessageIndex(messageIndex: number): Promise<boolean> {
    if (messageIndex < 0 || messageIndex >= totalMessagesRef.current) {
      return false;
    }
    positionedRef.current = true;

    const start = windowStartRef.current;
    const end = start + loadedCountRef.current;
    if (messageIndex < start || messageIndex >= end) {
      const recentered = await recenterWindowAround(messageIndex);
      if (!recentered) return false;
    }

    const entries = filteredEntriesRef.current;
    let entryIndex = entries.findIndex((entry) => entry.type === "message" && entry.messageIndex === messageIndex);
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

  /** Load whatever the window is missing so search covers every message.
   * The common shape (window is the newest tail) prepends the older part,
   * preserving the viewport; a re-centered window missing BOTH sides is
   * replaced with the full session, compensating the scroll offset by where
   * the previously-first row lands in the new entry list. */
  async function ensureCompleteWindowForSearch(): Promise<boolean> {
    if (windowFetchInFlightRef.current) return false;
    const start = windowStartRef.current;
    const end = start + loadedCountRef.current;
    const total = totalMessagesRef.current;
    if (start <= 0 && end >= total) return true;

    const request = beginWindowRequest("search");
    windowFetchInFlightRef.current = true;
    try {
      if (end >= total) {
        const older = await getSessionMessagesWindow(request.sessionId, 0, start, request.requestId);
        if (request.sessionId !== sessionIdRef.current) return false;
        prependMessages(older);
        return older.start === 0;
      }

      const complete = await getSessionMessagesWindow(request.sessionId, 0, total, request.requestId);
      if (request.sessionId !== sessionIdRef.current) return false;
      const scroller = scrollElementRef.current;
      const prevFirstKey = filteredEntriesRef.current[0]?.key;
      const prevScrollTop = scroller?.scrollTop ?? 0;
      flushSync(() => {
        opts.setMeta((prev) => opts.withTokenTotals(prev, complete.token_totals));
        opts.setMessages(complete.messages);
        setWindowStart(complete.start);
        setTotalMessages(complete.total);
      });
      if (scroller && prevFirstKey !== undefined) {
        const anchorIndex = filteredEntriesRef.current.findIndex((entry) => entry.key === prevFirstKey);
        if (anchorIndex > 0) {
          virtualizer.getTotalSize(); // force a fresh measurements pass
          const anchorStart = virtualizer.measurementsCache[anchorIndex]?.start ?? anchorIndex * ESTIMATED_ROW_PX;
          scroller.scrollTop = prevScrollTop + anchorStart;
        }
      }
      return complete.start === 0;
    } catch (e) {
      if (isLoadCanceledError(e)) return false;
      console.warn("load complete session for search failed:", e);
      return false;
    } finally {
      finishWindowRequest(request.requestId);
      windowFetchInFlightRef.current = false;
    }
  }

  async function resolveCompleteSearchMatch(term: string): Promise<number | null> {
    if (windowStartRef.current > 0 || windowStartRef.current + loadedCountRef.current < totalMessagesRef.current) {
      const loadedCompleteWindow = await ensureCompleteWindowForSearch();
      if (!loadedCompleteWindow) return null;
    }

    const matchIndex = findFirstMatchingEntryIndex(filteredEntriesRef.current, term);
    return matchIndex >= 0 ? matchIndex : null;
  }

  function scrollToEnd() {
    positionedRef.current = true;
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
