import { useCallback, useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { flushSync } from "react-dom";
import { cancelSessionLoad, getSessionMessagesWindow, isLoadCanceledError } from "@/lib/tauri";
import type { Message, SessionMeta, TokenTotals } from "@/lib/types";
import { findFirstMatchingEntryIndex } from "@/features/session/search-utils";
import {
  overscrolledBottom,
  overscrolledTop,
  rowAtEntryIndex,
  settleFrames,
} from "@/features/session/timelineGeometry";
import type { ProcessedEntry } from "@/features/session/hooks";

/** Backend page sizes: how many messages the initial open fetches and how
 * many each older-page fetch adds — they bound a single IPC payload and how
 * many messages sit in memory. */
export const INITIAL_TAIL = 300;
const TAIL_BATCH = 600;
/** Chunked landing (prependMessages): messages per commit, and how much of a
 * frame one landing may consume before yielding. Time-budgeted because
 * per-message mount cost varies wildly with content. */
const LAND_STEP = 15;
const LAND_FRAME_BUDGET_MS = 8;

export interface CreateSessionPaginationOptions {
  /** Current session id (guards stale async results). */
  sessionId: string;
  /** Role-filtered entries the timeline renders. */
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
  /** Scroll a row (by index in `filteredEntries`) into view. */
  scrollToItem: (index: number, align: "start" | "center" | "end") => void;
  /** Scroll to the newest row. */
  scrollToBottom: () => void;
}

export interface CreateSessionPaginationResult {
  totalMessages: number;
  setTotalMessages: Dispatch<SetStateAction<number>>;
  resolveCompleteSearchMatch: (term: string) => Promise<number | null>;
  revealEntry: (entryIndex: number) => void;
  revealMessageIndex: (messageIndex: number) => Promise<boolean>;
  revealNewest: () => Promise<boolean>;
  scrollToEnd: () => void;
  /** Page in the previous/next batch — wired to the edge-prefetch scroll handler. */
  loadOlder: () => void;
  loadNewer: () => void;
}

/**
 * Owns the windowed-loading + navigation slice of SessionView.
 *
 * Rendering is content-visibility inside a column-reverse scroller: every
 * loaded row is real DOM in normal flow and the browser skips paint/layout
 * for off-screen rows. Message *loading* stays windowed (the backend pages
 * the parsed session over IPC), in BOTH directions: older pages land at the
 * DOM end — outside the bottom-anchored scroll coordinate space, so the
 * viewport never moves and prepends need no compensation at all — and newer
 * pages land at the DOM start with a measured scrollTop compensation. A jump
 * far outside the window (minimap tick, reveal) re-centers the window around
 * the target instead of paging everything in between, then scrolls the target
 * row into view via the DOM. Only a committed in-session search loads the
 * complete session (counting must cover every message).
 *
 * Every returned callback is referentially stable (empty-dep useCallback over
 * latest-value refs): SessionView's per-frame scroll handler depends on
 * loadOlder/loadNewer, and an identity change there would recreate the handler
 * each render and defeat TimelineList's memo boundary.
 */
export function useSessionPagination(opts: CreateSessionPaginationOptions): CreateSessionPaginationResult {
  const [totalMessages, setTotalMessages] = useState(0);
  const { windowStart } = opts;

  // Latest-value refs so async callbacks read current values across awaits
  // instead of the values captured when the closure was created.
  const optsRef = useRef(opts);
  optsRef.current = opts;
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
  // Edge prefetch is armed only after the view has been positioned (scroll-to-
  // end on open, or a reveal jump). Before that the scroller sits at offset 0
  // and the "near the top" check would fire a spurious older-page fetch.
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

  const beginWindowRequest = useCallback((kind: string): { sessionId: string; requestId: string } => {
    const sessionId = sessionIdRef.current;
    const requestId = `${sessionId}:window:${kind}:${++windowRequestSeqRef.current}`;
    activeWindowRequestRef.current = { sessionId, requestId };
    return { sessionId, requestId };
  }, []);

  const finishWindowRequest = useCallback((requestId: string) => {
    if (activeWindowRequestRef.current?.requestId === requestId) {
      activeWindowRequestRef.current = null;
    }
  }, []);

  /** Wait out a bottom-edge rubber-band bounce: appending newer messages
   * writes scrollTop, and writes mid-bounce fight the engine animation. */
  const settleBottomOverscroll = useCallback(async (el: HTMLElement): Promise<void> => {
    if (!overscrolledBottom(el)) return;
    let lastTop = el.scrollTop;
    await settleFrames(
      () => {
        const top = el.scrollTop;
        const stable = !overscrolledBottom(el) && Math.abs(top - lastTop) < 1;
        lastTop = top;
        return stable;
      },
      3,
      2000,
    );
  }, []);

  // Bumped by recenterWindowAround to abort an in-progress chunked landing:
  // a navigation jump replaces the window wholesale, so finishing the landing
  // would waste main-thread time and (worse) hold the fetch lock against it.
  const applyGenerationRef = useRef(0);
  // The latest navigation owns both its replacement window and final
  // alignment. Without this guard, a slower earlier jump can snap back after
  // a later click has already landed.
  const navigationGenerationRef = useRef(0);

  /** Prepend an older page. History lands at the DOM end — outside the
   * bottom-anchored scroll coordinate space — so the viewport never moves and
   * no scroll compensation exists here. The page lands in small commits
   * spread across frames (nearest-to-viewport first, bounded per-frame by
   * time, paused during a rubber-band bounce) so mounting hundreds of bubbles
   * never freezes a frame right as the user reaches the edge. */
  const prependMessages = useCallback(
    async (older: { messages: Message[]; start: number; total: number; token_totals: TokenTotals }) => {
      const sessionAtStart = sessionIdRef.current;
      const generationAtStart = applyGenerationRef.current;
      const superseded = () =>
        sessionIdRef.current !== sessionAtStart || applyGenerationRef.current !== generationAtStart;
      let remaining = older.messages.length;
      while (remaining > 0) {
        if (superseded()) return;
        const el = scrollElementRef.current;
        if (el && overscrolledTop(el)) {
          await settleFrames(() => !overscrolledTop(el), 1, 1500);
          if (superseded()) return;
        }
        const frameStart = performance.now();
        while (remaining > 0 && performance.now() - frameStart < LAND_FRAME_BUDGET_MS) {
          const chunkStart = Math.max(0, remaining - LAND_STEP);
          const chunk = older.messages.slice(chunkStart, remaining);
          const current = optsRef.current;
          flushSync(() => {
            current.setMessages((prev) => [...chunk, ...prev]);
            current.setWindowStart(older.start + chunkStart);
            if (chunkStart === 0) {
              current.setMeta((prev) => current.withTokenTotals(prev, older.token_totals));
              setTotalMessages(older.total);
            }
          });
          remaining = chunkStart;
        }
        if (remaining > 0) await new Promise(requestAnimationFrame);
      }
    },
    [],
  );

  const loadOlderTail = useCallback(async (): Promise<boolean> => {
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
      await prependMessages(older);
      return older.messages.length > 0;
    } catch (e) {
      if (isLoadCanceledError(e)) return false;
      console.warn("load older messages failed:", e);
      return false;
    } finally {
      finishWindowRequest(request.requestId);
      windowFetchInFlightRef.current = false;
    }
  }, [beginWindowRequest, finishWindowRequest, prependMessages]);

  const loadNewerTail = useCallback(async (): Promise<boolean> => {
    const end = windowStartRef.current + loadedCountRef.current;
    if (windowFetchInFlightRef.current || end >= totalMessagesRef.current) {
      return false;
    }
    const request = beginWindowRequest("newer");
    windowFetchInFlightRef.current = true;
    try {
      const newer = await getSessionMessagesWindow(request.sessionId, end, TAIL_BATCH, request.requestId);
      if (request.sessionId !== sessionIdRef.current) return false;
      // Newer messages land at the DOM start — INSIDE the bottom-anchored
      // coordinate space — which shifts the viewport by the inserted height
      // (measured: anchor moved by exactly the inserted px). Compensate with
      // a synchronous measure-and-adjust, waiting out any bottom-edge bounce
      // first so the write doesn't fight the engine animation.
      const el = scrollElementRef.current;
      if (el) await settleBottomOverscroll(el);
      if (request.sessionId !== sessionIdRef.current) return false;
      const beforeHeight = el?.scrollHeight ?? 0;
      const beforeTop = el?.scrollTop ?? 0;
      flushSync(() => {
        const current = optsRef.current;
        current.setMeta((prev) => current.withTokenTotals(prev, newer.token_totals));
        current.setMessages((prev) => [...prev, ...newer.messages]);
        setTotalMessages(newer.total);
      });
      if (el) {
        // This path only runs on a truncated window (end < total), so even
        // scrollTop 0 means "bottom of the LOADED window", never the true
        // newest — always keep the read position.
        const grew = el.scrollHeight - beforeHeight;
        if (grew !== 0) el.scrollTop = beforeTop - grew;
      }
      return newer.messages.length > 0;
    } catch (e) {
      if (isLoadCanceledError(e)) return false;
      console.warn("load newer messages failed:", e);
      return false;
    } finally {
      finishWindowRequest(request.requestId);
      windowFetchInFlightRef.current = false;
    }
  }, [beginWindowRequest, finishWindowRequest, settleBottomOverscroll]);

  // Edge prefetch fires only once the view has been positioned, so the open
  // scroll-to-bottom doesn't trigger a spurious fetch.
  const loadOlder = useCallback(() => {
    if (!positionedRef.current) return;
    void loadOlderTail();
  }, [loadOlderTail]);
  const loadNewer = useCallback(() => {
    if (!positionedRef.current) return;
    void loadNewerTail();
  }, [loadNewerTail]);

  const revealEntry = useCallback((entryIndex: number) => {
    const total = filteredEntriesRef.current.length;
    if (entryIndex < 0 || entryIndex >= total) return;
    positionedRef.current = true;
    optsRef.current.scrollToItem(entryIndex, "center");
  }, []);

  /** Re-center the loaded window around a target message, REPLACING the current
   * window. A far jump (minimap tick near the top of a 13k-message session)
   * must not page in everything in between — re-centering costs the same IPC as
   * opening the session, and discarded rows reload on demand if scrolled back. */
  const recenterWindowAround = useCallback(
    async (messageIndex: number, navigationGeneration: number): Promise<boolean> => {
      // Navigation wins over prefetch: abort any chunked landing in progress
      // and wait for it to release the fetch lock instead of dropping the jump.
      applyGenerationRef.current += 1;
      await settleFrames(
        () => navigationGenerationRef.current !== navigationGeneration || !windowFetchInFlightRef.current,
        1,
        2000,
      );
      if (navigationGenerationRef.current !== navigationGeneration || windowFetchInFlightRef.current) return false;
      const request = beginWindowRequest("recenter");
      windowFetchInFlightRef.current = true;
      try {
        // End-align the window when the target sits near the newest message:
        // a centered window would leave the tail unloaded, and the browser
        // then clamps "align turn to viewport top" against a fake bottom —
        // the jump lands mid-air with more content still streaming in below.
        const start = Math.max(
          0,
          Math.min(messageIndex - Math.floor(INITIAL_TAIL / 2), totalMessagesRef.current - INITIAL_TAIL),
        );
        const window = await getSessionMessagesWindow(request.sessionId, start, INITIAL_TAIL, request.requestId);
        if (request.sessionId !== sessionIdRef.current || navigationGenerationRef.current !== navigationGeneration) {
          return false;
        }
        // flushSync so the entry lookup below sees the new window.
        flushSync(() => {
          const current = optsRef.current;
          current.setMeta((prev) => current.withTokenTotals(prev, window.token_totals));
          current.setMessages(window.messages);
          current.setWindowStart(window.start);
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
    },
    [beginWindowRequest, finishWindowRequest],
  );

  const revealMessageIndex = useCallback(
    async (messageIndex: number): Promise<boolean> => {
      if (messageIndex < 0 || messageIndex >= totalMessagesRef.current) {
        return false;
      }
      const revealGeneration = ++navigationGenerationRef.current;
      positionedRef.current = true;

      const start = windowStartRef.current;
      const end = start + loadedCountRef.current;
      // Also recenter when the target sits near the loaded window's end while
      // more messages exist beyond it: "align to viewport top" clamps against
      // the loaded bottom, and a truncated tail makes that clamp land mid-air.
      const nearTruncatedEnd = end < totalMessagesRef.current && messageIndex >= end - Math.floor(INITIAL_TAIL / 4);
      if (messageIndex < start || messageIndex >= end || nearTruncatedEnd) {
        const recentered = await recenterWindowAround(messageIndex, revealGeneration);
        if (!recentered || navigationGenerationRef.current !== revealGeneration) return false;
      }

      const entries = filteredEntriesRef.current;
      let entryIndex = entries.findIndex((entry) => entry.type === "message" && entry.messageIndex === messageIndex);
      if (entryIndex < 0) {
        // The exact message may be folded into a merged-tool row or filtered out;
        // land on the first entry at or after it instead of failing.
        entryIndex = entries.findIndex((entry) => {
          if (entry.type === "message") return entry.messageIndex >= messageIndex;
          if (entry.type === "merged-tools") {
            return entry.messageIndices.some((index) => index >= messageIndex);
          }
          return false;
        });
      }
      if (entryIndex < 0) return false;

      const row = scrollElementRef.current ? rowAtEntryIndex(scrollElementRef.current, entryIndex) : null;
      if (!row) return false;

      // content-visibility initially lays out a newly revealed window from its
      // intrinsic size estimates. Real heights arrive in waves: each alignment
      // can paint another band of rows and move the target thousands of pixels
      // while scrollTop stays fixed. Re-align until one pass survives a short
      // quiet period. ponytail: five bounded passes cover a wholly new 300-row
      // page; raise the cap only if runtime evidence shows residual drift.
      for (let attempt = 0; attempt < 5; attempt += 1) {
        row.scrollIntoView({ block: "start" });
        const settleStartedAt = performance.now();
        let previousTop = row.getBoundingClientRect().top;
        await settleFrames(
          () => {
            if (navigationGenerationRef.current !== revealGeneration || !row.isConnected) return true;
            const top = row.getBoundingClientRect().top;
            const stable = Math.abs(top - previousTop) < 1;
            previousTop = top;
            return performance.now() - settleStartedAt >= 80 && stable;
          },
          3,
          500,
        );

        const root = scrollElementRef.current;
        if (navigationGenerationRef.current !== revealGeneration || !row.isConnected || !root?.contains(row)) {
          return false;
        }
        // The scroller's 18px top padding is the smallest possible offset for
        // the oldest row, so anything within 24px is correctly aligned.
        if (Math.abs(row.getBoundingClientRect().top - root.getBoundingClientRect().top) <= 24) return true;
      }
      return false;
    },
    [recenterWindowAround],
  );

  /** Load whatever the window is missing so search covers every message.
   * Applied in ONE synchronous commit, not the chunked landing: the user just
   * committed a search and expects the pause, and dripping a 13k-message
   * history at landing pace would stall the search for many seconds. The
   * common shape (window is the newest tail) prepends history — free in the
   * bottom-anchored scroller; a window missing BOTH sides is replaced whole,
   * restoring the viewport by re-anchoring the previously-top row. */
  const ensureCompleteWindowForSearch = useCallback(async (): Promise<boolean> => {
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
        flushSync(() => {
          const current = optsRef.current;
          current.setMeta((prev) => current.withTokenTotals(prev, older.token_totals));
          current.setMessages((prev) => [...older.messages, ...prev]);
          current.setWindowStart(older.start);
          setTotalMessages(older.total);
        });
        return older.start === 0;
      }

      const complete = await getSessionMessagesWindow(request.sessionId, 0, total, request.requestId);
      if (request.sessionId !== sessionIdRef.current) return false;
      // Replacing the window inserts newer content INSIDE the bottom-anchored
      // coordinate space, which would shift the view (and a zero-match search
      // never re-scrolls). Re-anchor on the row that was at the viewport top.
      const el = scrollElementRef.current;
      const anchorRow = el
        ? (document
            .elementFromPoint(el.getBoundingClientRect().left + 24, el.getBoundingClientRect().top + 24)
            ?.closest(".session-entry") ?? null)
        : null;
      const anchorKey = anchorRow?.getAttribute("data-entry-key") ?? null;
      const anchorTop = anchorRow?.getBoundingClientRect().top ?? 0;
      flushSync(() => {
        const current = optsRef.current;
        current.setMeta((prev) => current.withTokenTotals(prev, complete.token_totals));
        current.setMessages(complete.messages);
        current.setWindowStart(complete.start);
        setTotalMessages(complete.total);
      });
      if (el && anchorKey !== null) {
        const restored = el.querySelector(`[data-entry-key="${CSS.escape(anchorKey)}"]`);
        if (restored) el.scrollTop += restored.getBoundingClientRect().top - anchorTop;
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
  }, [beginWindowRequest, finishWindowRequest]);

  const resolveCompleteSearchMatch = useCallback(
    async (term: string): Promise<number | null> => {
      if (windowStartRef.current > 0 || windowStartRef.current + loadedCountRef.current < totalMessagesRef.current) {
        const loadedCompleteWindow = await ensureCompleteWindowForSearch();
        if (!loadedCompleteWindow) return null;
      }

      const matchIndex = findFirstMatchingEntryIndex(filteredEntriesRef.current, term);
      return matchIndex >= 0 ? matchIndex : null;
    },
    [ensureCompleteWindowForSearch],
  );

  const scrollToEnd = useCallback(() => {
    positionedRef.current = true;
    optsRef.current.scrollToBottom();
  }, []);

  /** Jump to the newest message (last minimap tick / End): make sure the tail
   * is loaded, then pin the bottom-anchored scroller to 0. Aligning the last
   * turn's FIRST message to the viewport top (the generic reveal) would strand
   * the actual newest messages below the fold. */
  const revealNewest = useCallback(async (): Promise<boolean> => {
    const navigationGeneration = ++navigationGenerationRef.current;
    positionedRef.current = true;
    const total = totalMessagesRef.current;
    if (total > 0 && windowStartRef.current + loadedCountRef.current < total) {
      const recentered = await recenterWindowAround(total - 1, navigationGeneration);
      if (!recentered) return false;
    }
    if (navigationGenerationRef.current !== navigationGeneration) return false;
    optsRef.current.scrollToBottom();
    return true;
  }, [recenterWindowAround]);

  return {
    totalMessages,
    setTotalMessages,
    resolveCompleteSearchMatch,
    revealEntry,
    revealMessageIndex,
    revealNewest,
    scrollToEnd,
    loadOlder,
    loadNewer,
  };
}
