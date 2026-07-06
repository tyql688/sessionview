import { useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { flushSync } from "react-dom";
import { getSessionMessagesWindow, isLoadCanceledError } from "../../lib/tauri";
import type { Message, SessionMeta, TokenTotals } from "../../lib/types";
import { findFirstMatchingEntryIndex } from "./search-utils";
import type { ProcessedEntry } from "./hooks";

export const BATCH_SIZE = 80;
export const LOAD_MORE_THRESHOLD = 1;
export const MINIMAP_JUMP_BATCH = 1200;
export const INITIAL_TAIL = 300;
const TAIL_BATCH = 600;

export interface CreateSessionPaginationOptions {
  /** Current session id (guards stale async results). */
  sessionId: string;
  /** Role-filtered entries the render window slices over. */
  filteredEntries: ProcessedEntry[];
  /** Loaded messages (read by `hasMore`). */
  messages: Message[];
  /** Absolute session index of messages[0] — owned by the component because
   * `processMessages` needs it before this hook can run. */
  windowStart: number;
  setWindowStart: Dispatch<SetStateAction<number>>;
  /** Lazy ref getter — the messages container may not exist yet. */
  getMessagesRef: () => HTMLDivElement | undefined;
  setMessages: Dispatch<SetStateAction<Message[]>>;
  setMeta: Dispatch<SetStateAction<SessionMeta>>;
  /** Apply fresh token totals onto a meta object. */
  withTokenTotals: (metaData: SessionMeta, totals: TokenTotals) => SessionMeta;
  /** Register the older-load debounce timer for cleanup by the component. */
  registerDebounce: (clear: () => void) => void;
}

export interface CreateSessionPaginationResult {
  visibleCount: number;
  setVisibleCount: Dispatch<SetStateAction<number>>;
  totalMessages: number;
  setTotalMessages: Dispatch<SetStateAction<number>>;
  visibleEntries: ProcessedEntry[];
  hasMore: boolean;
  loadOlderEntries: () => void;
  resolveCompleteSearchMatch: (term: string) => Promise<number | null>;
  revealEntry: (entryIndex: number) => void;
  revealMessageIndex: (messageIndex: number) => Promise<boolean>;
  handleMessagesScroll: (e: Event) => void;
}

/**
 * Owns the windowed-loading slice of SessionView: the `visibleCount`,
 * `windowStart`, and `totalMessages` signals; the `visibleEntries`/`hasMore`
 * memos; and the scroll-driven older-page fetch + scroll-pin machinery. Bodies
 * are moved verbatim from the inline component so dependency tracking, the
 * column-reverse window math, and the scroll-pin timing are unchanged.
 *
 * The `messages`/`meta` signals stay owned by the component (the initial load
 * and live-watch reload write them too); their setters + `withTokenTotals` are
 * threaded in so `loadOlderTail` can prepend without owning those signals.
 *
 * Now a React hook: call it at the top level of a component. The async
 * callbacks below read the current `filteredEntries`/`sessionId`/window state
 * through latest-value refs (React re-renders don't reach a closure created in
 * an earlier render). The three prepend paths that read `filteredEntries`
 * immediately after `setMessages` wrap the writes in `flushSync`, replacing
 * Solid's synchronous memo recompute so the post-prepend lookup sees the newly
 * loaded entries.
 */
export function useSessionPagination(
  opts: CreateSessionPaginationOptions,
): CreateSessionPaginationResult {
  const [visibleCount, setVisibleCount] = useState(BATCH_SIZE);

  // Reversed for column-reverse layout: newest first in DOM = visually at bottom.
  const visibleEntries = useMemo(() => {
    const all = opts.filteredEntries;
    const count = visibleCount;
    const start = count >= all.length ? 0 : all.length - count;
    return all.slice(start).reverse();
  }, [opts.filteredEntries, visibleCount]);
  // Streaming pagination state — declared before `hasMore` since it's
  // read inside that memo. `windowStart` lives in the component (see opts).
  const [totalMessages, setTotalMessages] = useState(0);
  const { windowStart, setWindowStart } = opts;

  // We have more to render if either the in-memory window has unrendered
  // entries OR the backend still holds older messages we haven't fetched.
  const hasMore = useMemo(
    () =>
      visibleCount < opts.filteredEntries.length ||
      (windowStart > 0 && opts.messages.length < totalMessages),
    [
      visibleCount,
      opts.filteredEntries,
      windowStart,
      opts.messages,
      totalMessages,
    ],
  );

  // Latest-value refs so the async/scroll callbacks below read the current
  // reactive values (matching Solid's accessor reads) even across awaits and
  // re-renders, instead of a value captured when the closure was created.
  const filteredEntriesRef = useRef(opts.filteredEntries);
  filteredEntriesRef.current = opts.filteredEntries;
  const sessionIdRef = useRef(opts.sessionId);
  sessionIdRef.current = opts.sessionId;
  const visibleCountRef = useRef(visibleCount);
  visibleCountRef.current = visibleCount;
  const windowStartRef = useRef(windowStart);
  windowStartRef.current = windowStart;
  const totalMessagesRef = useRef(totalMessages);
  totalMessagesRef.current = totalMessages;
  const hasMoreRef = useRef(hasMore);
  hasMoreRef.current = hasMore;

  const loadOlderDebounceRef = useRef<
    ReturnType<typeof setTimeout> | undefined
  >(undefined);
  const olderFetchInFlightRef = useRef(false);
  useEffect(() => {
    opts.registerDebounce(() => clearTimeout(loadOlderDebounceRef.current));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function revealEntry(entryIndex: number) {
    const total = filteredEntriesRef.current.length;
    if (entryIndex < 0 || entryIndex >= total) return;
    const requiredCount = total - entryIndex;
    if (requiredCount > visibleCountRef.current) {
      setVisibleCount(requiredCount);
    }
  }

  async function revealMessageIndex(messageIndex: number): Promise<boolean> {
    if (messageIndex < 0 || messageIndex >= totalMessagesRef.current)
      return false;

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
        // flushSync so `filteredEntriesRef` reflects the prepend synchronously
        // before the entry lookup below (Solid recomputed the memo inline).
        flushSync(() => {
          opts.setMeta((prev) =>
            opts.withTokenTotals(prev, older.token_totals),
          );
          opts.setMessages((prev) => [...older.messages, ...prev]);
          setWindowStart(older.start);
          setTotalMessages(older.total);
        });
      } catch (e) {
        if (isLoadCanceledError(e)) return false;
        console.warn("reveal message failed:", e);
        return false;
      } finally {
        olderFetchInFlightRef.current = false;
      }
    }

    const entryIndex = filteredEntriesRef.current.findIndex((entry) => {
      if (entry.type !== "message") return false;
      return entry.messageIndex === messageIndex;
    });
    if (entryIndex < 0) return false;
    revealEntry(entryIndex);
    return true;
  }

  // Re-pin the scroll position to where the user was looking right
  // before we grew the visible-entries list. In column-reverse, new
  // rows are appended to the DOM but appear visually *above* the
  // existing content. Browsers usually preserve `scrollTop` across
  // this kind of growth, but Solid's `<For>` reconciliation can move
  // DOM nodes when the array shape changes and Chrome's scroll
  // anchoring will sometimes shift the viewport away from the
  // captured row. Snapping `scrollTop` back to the captured value
  // after two paint frames keeps the user on the row they were
  // reading.
  function pinScrollAfterPrepend(scrollBefore: number) {
    const ref = opts.getMessagesRef();
    if (!ref) return;
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (ref === opts.getMessagesRef() && ref.scrollTop !== scrollBefore) {
          ref.scrollTop = scrollBefore;
        }
      });
    });
  }

  async function loadOlderTail(options: {
    revealLoadedEntries: boolean;
    pinScroll: boolean;
  }): Promise<boolean> {
    if (olderFetchInFlightRef.current || windowStartRef.current <= 0)
      return false;
    const sessionId = sessionIdRef.current;
    olderFetchInFlightRef.current = true;
    const newStart = Math.max(0, windowStartRef.current - TAIL_BATCH);
    const span = windowStartRef.current - newStart;
    const scrollBefore = opts.getMessagesRef()?.scrollTop ?? 0;
    try {
      const older = await getSessionMessagesWindow(sessionId, newStart, span);
      if (sessionId !== sessionIdRef.current) return false;
      opts.setMeta((prev) => opts.withTokenTotals(prev, older.token_totals));
      // Prepend the newly fetched older messages and grow `visibleCount`
      // by the same amount so the just-fetched entries actually become
      // visible at the top of the viewport (column-reverse layout).
      // Without the bump, `visibleEntries` slices from the newer end and
      // the user sees no change after the round trip.
      opts.setMessages((prev) => [...older.messages, ...prev]);
      setWindowStart(newStart);
      setTotalMessages(older.total);
      if (options.revealLoadedEntries) {
        setVisibleCount((count) => count + older.messages.length);
      }
      if (options.pinScroll) {
        pinScrollAfterPrepend(scrollBefore);
      }
      return older.messages.length > 0;
    } catch (e) {
      if (isLoadCanceledError(e)) return false;
      console.warn("load older messages failed:", e);
      return false;
    } finally {
      olderFetchInFlightRef.current = false;
    }
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
      // flushSync so `filteredEntriesRef` reflects the prepend synchronously
      // before `resolveCompleteSearchMatch` scans it (Solid recomputed inline).
      flushSync(() => {
        opts.setMeta((prev) => opts.withTokenTotals(prev, older.token_totals));
        opts.setMessages((prev) => [...older.messages, ...prev]);
        setWindowStart(older.start);
        setTotalMessages(older.total);
      });
      return older.start === 0;
    } catch (e) {
      if (isLoadCanceledError(e)) return false;
      console.warn("load complete session for search failed:", e);
      return false;
    } finally {
      olderFetchInFlightRef.current = false;
    }
  }

  function loadOlderEntries() {
    const messagesRef = opts.getMessagesRef();
    if (!messagesRef || !hasMoreRef.current) return;
    // column-reverse: older entries append at the end of the DOM (visual top).
    // First exhaust the in-memory window via visibleCount, then page in
    // older messages from the backend cache.
    if (visibleCountRef.current < filteredEntriesRef.current.length) {
      const scrollBefore = messagesRef.scrollTop;
      setVisibleCount((count) => count + BATCH_SIZE);
      pinScrollAfterPrepend(scrollBefore);
      return;
    }
    if (windowStartRef.current > 0) {
      void loadOlderTail({ revealLoadedEntries: true, pinScroll: true });
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

  function handleMessagesScroll(e: Event) {
    const target = e.currentTarget as HTMLDivElement;
    clearTimeout(loadOlderDebounceRef.current);

    // column-reverse: scrollTop=0 is bottom (newest). User scrolls up -> scrollTop
    // goes negative. We want to load more when user reaches the visual top.
    // Visual top = max negative scrollTop = -(scrollHeight - clientHeight).
    const atVisualTop =
      target.scrollHeight + target.scrollTop - target.clientHeight <=
      LOAD_MORE_THRESHOLD;

    if (atVisualTop) {
      loadOlderDebounceRef.current = setTimeout(() => {
        const messagesRef = opts.getMessagesRef();
        if (!messagesRef) return;
        const stillAtTop =
          messagesRef.scrollHeight +
            messagesRef.scrollTop -
            messagesRef.clientHeight <=
          LOAD_MORE_THRESHOLD;
        if (stillAtTop) {
          loadOlderEntries();
        }
      }, 80);
    }
  }

  return {
    visibleCount,
    setVisibleCount,
    totalMessages,
    setTotalMessages,
    visibleEntries,
    hasMore,
    loadOlderEntries,
    resolveCompleteSearchMatch,
    revealEntry,
    revealMessageIndex,
    handleMessagesScroll,
  };
}
