import { createMemo, createSignal } from "solid-js";
import type { Accessor, Setter } from "solid-js";
import { getSessionMessagesWindow, isLoadCanceledError } from "../../lib/tauri";
import type { Message, SessionMeta, TokenTotals } from "../../lib/types";
import type { ProcessedEntry } from "./hooks";
import { searchWindowBounds } from "./search-utils";

export const BATCH_SIZE = 80;
export const LOAD_MORE_THRESHOLD = 1;
export const MINIMAP_JUMP_BATCH = 1200;
export const INITIAL_TAIL = 300;
const TAIL_BATCH = 600;

export interface CreateSessionPaginationOptions {
  /** Current session id (guards stale async results). */
  sessionId: Accessor<string>;
  /** Role-filtered entries the render window slices over. */
  filteredEntries: Accessor<ProcessedEntry[]>;
  /** Loaded messages (read by `hasMore`). */
  messages: Accessor<Message[]>;
  /** Lazy ref getter — the messages container may not exist yet. */
  getMessagesRef: () => HTMLDivElement | undefined;
  /** In-session search focus index (drives the search render window). */
  searchFocusEntryIndex: Accessor<number | null>;
  /** Active in-session search term (drives the search render window). */
  activeSessionSearch: Accessor<string>;
  setMessages: Setter<Message[]>;
  setMeta: Setter<SessionMeta>;
  /** Apply fresh token totals onto a meta object. */
  withTokenTotals: (metaData: SessionMeta, totals: TokenTotals) => SessionMeta;
  /** Register the older-load debounce timer for cleanup by the component. */
  registerDebounce: (clear: () => void) => void;
}

export interface CreateSessionPaginationResult {
  visibleCount: Accessor<number>;
  setVisibleCount: Setter<number>;
  windowStart: Accessor<number>;
  setWindowStart: Setter<number>;
  totalMessages: Accessor<number>;
  setTotalMessages: Setter<number>;
  visibleEntries: Accessor<ProcessedEntry[]>;
  hasMore: Accessor<boolean>;
  loadOlderEntries: () => void;
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
 */
export function createSessionPagination(
  opts: CreateSessionPaginationOptions,
): CreateSessionPaginationResult {
  const [visibleCount, setVisibleCount] = createSignal(BATCH_SIZE);

  // Reversed for column-reverse layout: newest first in DOM = visually at bottom.
  // Search keeps the existing render window and expands only enough to reveal
  // the first match. Rendering every entry on each input stalls large sessions.
  const visibleEntries = createMemo(() => {
    const all = opts.filteredEntries();
    const focusedIndex = opts.searchFocusEntryIndex();
    if (opts.activeSessionSearch().trim() && focusedIndex !== null) {
      const bounds = searchWindowBounds(all.length, focusedIndex);
      if (bounds) {
        return all.slice(bounds.start, bounds.end).reverse();
      }
    }
    const count = visibleCount();
    const start = count >= all.length ? 0 : all.length - count;
    return all.slice(start).reverse();
  });
  // Streaming pagination state — declared before `hasMore` since it's
  // read inside that memo.
  const [totalMessages, setTotalMessages] = createSignal(0);
  const [windowStart, setWindowStart] = createSignal(0);

  // We have more to render if either the in-memory window has unrendered
  // entries OR the backend still holds older messages we haven't fetched.
  const hasMore = createMemo(
    () =>
      visibleCount() < opts.filteredEntries().length ||
      (windowStart() > 0 && opts.messages().length < totalMessages()),
  );

  let loadOlderDebounce: ReturnType<typeof setTimeout> | undefined;
  let olderFetchInFlight = false;
  opts.registerDebounce(() => clearTimeout(loadOlderDebounce));

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

  async function loadOlderTail() {
    if (olderFetchInFlight || windowStart() <= 0) return;
    const sessionId = opts.sessionId();
    olderFetchInFlight = true;
    const newStart = Math.max(0, windowStart() - TAIL_BATCH);
    const span = windowStart() - newStart;
    const scrollBefore = opts.getMessagesRef()?.scrollTop ?? 0;
    try {
      const older = await getSessionMessagesWindow(sessionId, newStart, span);
      if (sessionId !== opts.sessionId()) return;
      opts.setMeta((prev) => opts.withTokenTotals(prev, older.token_totals));
      // Prepend the newly fetched older messages and grow `visibleCount`
      // by the same amount so the just-fetched entries actually become
      // visible at the top of the viewport (column-reverse layout).
      // Without the bump, `visibleEntries` slices from the newer end and
      // the user sees no change after the round trip.
      opts.setMessages((prev) => [...older.messages, ...prev]);
      setWindowStart(newStart);
      setTotalMessages(older.total);
      setVisibleCount((count) => count + older.messages.length);
      pinScrollAfterPrepend(scrollBefore);
    } catch (e) {
      if (isLoadCanceledError(e)) return;
      console.warn("load older messages failed:", e);
    } finally {
      olderFetchInFlight = false;
    }
  }

  function loadOlderEntries() {
    const messagesRef = opts.getMessagesRef();
    if (!messagesRef || !hasMore()) return;
    // column-reverse: older entries append at the end of the DOM (visual top).
    // First exhaust the in-memory window via visibleCount, then page in
    // older messages from the backend cache.
    if (visibleCount() < opts.filteredEntries().length) {
      const scrollBefore = messagesRef.scrollTop;
      setVisibleCount((count) => count + BATCH_SIZE);
      pinScrollAfterPrepend(scrollBefore);
      return;
    }
    if (windowStart() > 0) {
      void loadOlderTail();
    }
  }

  function handleMessagesScroll(e: Event) {
    const target = e.currentTarget as HTMLDivElement;
    clearTimeout(loadOlderDebounce);

    // column-reverse: scrollTop=0 is bottom (newest). User scrolls up -> scrollTop
    // goes negative. We want to load more when user reaches the visual top.
    // Visual top = max negative scrollTop = -(scrollHeight - clientHeight).
    const atVisualTop =
      target.scrollHeight + target.scrollTop - target.clientHeight <=
      LOAD_MORE_THRESHOLD;

    if (atVisualTop) {
      loadOlderDebounce = setTimeout(() => {
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
    windowStart,
    setWindowStart,
    totalMessages,
    setTotalMessages,
    visibleEntries,
    hasMore,
    loadOlderEntries,
    handleMessagesScroll,
  };
}
