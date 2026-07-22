import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { SessionRef, SessionMeta, Message, MessageRole, TokenTotals } from "@/lib/types";
import {
  getSessionOpenWindow,
  getSessionTurnOutline,
  cancelSessionLoad,
  resumeSession,
  isLoadCanceledError,
  type SessionRoleCounts,
  type SessionTurnOutlineEntry,
} from "@/lib/tauri";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useI18n } from "@/i18n/index";
import { markdownChunkReady } from "@/features/session/MessageBubble";
import { TimelineList } from "@/features/session/TimelineList";
import { ExportDialog } from "@/features/session/ExportDialog";
import { SessionAnalyticsDialog } from "@/features/session/SessionAnalyticsDialog";
import { setFocusMode, useFocusMode, useTerminalApp } from "@/stores/settings";
import { toast, toastError } from "@/stores/toast";
import { errorMessage } from "@/lib/errors";
import { processMessages } from "@/features/session/hooks";
import { SessionToolbar } from "@/features/session/SessionToolbar";
import { SessionSearch } from "@/features/session/SessionSearch";
import { TimelineMinimapDriver } from "@/features/session/TimelineMinimap";
import { activeMatchTarget, paintVisibleHighlights, scrollRangeIntoView } from "@/features/session/search-utils";
import { useFavoriteSync } from "@/features/session/useFavoriteSync";
import { useSessionCommandEvents } from "@/features/session/useSessionCommandEvents";
import { useRoleFilter } from "@/features/session/createRoleFilter";
import { useSessionSearch } from "@/features/session/createSessionSearch";
import { useSessionPagination, INITIAL_TAIL } from "@/features/session/createSessionPagination";
import {
  distanceToNewest,
  distanceToOldest,
  rowAtEntryIndex,
  viewportBottom,
} from "@/features/session/timelineGeometry";

/** Each role keeps its own pressed color so the filter reads at a glance. */
const ROLE_TOGGLE_COLORS: Record<MessageRole, string> = {
  user: "data-pressed:bg-brand-soft data-pressed:text-brand",
  assistant: "data-pressed:bg-[color-mix(in_srgb,var(--success)_12%,transparent)] data-pressed:text-success",
  tool: "data-pressed:bg-[color-mix(in_srgb,var(--text-warning)_14%,transparent)] data-pressed:text-warning",
  system:
    "data-pressed:bg-[color-mix(in_srgb,var(--accent-secondary)_12%,transparent)] data-pressed:text-(--accent-secondary)",
};

export function SessionView(props: { session: SessionRef; active: boolean }) {
  const { t } = useI18n();
  const terminalApp = useTerminalApp();
  const focusMode = useFocusMode();
  const [messages, setMessages] = useState<Message[]>([]);
  const [outline, setOutline] = useState<SessionTurnOutlineEntry[]>([]);
  // Session-WIDE role counts (from the outline's full parse). The loaded
  // window's own counts grow as pages land — showing those in the filter bar
  // reads as the numbers randomly inflating while you scroll. Null until the
  // full parse delivers; the bar hides counts (not the buttons) meanwhile.
  const [sessionRoleCounts, setSessionRoleCounts] = useState<SessionRoleCounts | null>(null);
  // Absolute session index of messages[0]. Owned here (not by the pagination
  // hook) because processMessages needs it to emit absolute message indices.
  const [windowStart, setWindowStart] = useState(0);
  const processedEntries = useMemo(() => processMessages(messages, windowStart), [messages, windowStart]);

  const [loading, setLoading] = useState(true);
  // One-time gate: hold the first paint until the markdown chunk is in, so
  // bubbles never paint the raw-text Suspense fallback and then reflow to
  // rendered markdown. Sync-true on every open after the first.
  const [markdownReady, setMarkdownReady] = useState(markdownChunkReady.loaded);
  useEffect(() => {
    if (markdownReady) return;
    let alive = true;
    void markdownChunkReady.promise.then(() => {
      if (alive) setMarkdownReady(true);
    });
    return () => {
      alive = false;
    };
  }, [markdownReady]);
  const [error, setError] = useState<string | null>(null);
  const [parseWarningCount, setParseWarningCount] = useState(0);
  const [meta, setMeta] = useState<SessionMeta>(() => ({
    ...props.session,
    source_path: props.session.source_path ?? "",
    project_path: props.session.project_path ?? "",
    created_at: 0,
    updated_at: 0,
    message_count: 0,
    file_size_bytes: 0,
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
  }));
  const loadVersionRef = useRef(0);
  const messagesRef = useRef<HTMLDivElement | null>(null);
  // State twin of messagesRef: pagination and child effects must re-run
  // when the scroll container (un)mounts — a bare ref mutation never
  // re-renders, so a snapshot prop goes stale.
  const [messagesEl, setMessagesEl] = useState<HTMLDivElement | null>(null);
  const sessionSearchDebounceRef = useRef<(() => void) | undefined>(undefined);
  const activeOpenRequestRef = useRef<{
    sessionId: string;
    requestId: string;
  } | null>(null);

  function withTokenTotals(metaData: SessionMeta, totals: TokenTotals): SessionMeta {
    return {
      ...metaData,
      input_tokens: totals.input_tokens,
      output_tokens: totals.output_tokens,
      cache_read_tokens: totals.cache_read_tokens,
      cache_write_tokens: totals.cache_write_tokens,
    };
  }

  // Role-filter slice: hiddenRoles + filteredEntries + roleCounts.
  const { hiddenRoles, roleCounts, filteredEntries, toggleRole } = useRoleFilter(processedEntries, focusMode);
  // A role's button shows if EITHER count source knows about it: the union
  // keeps a button present for rows already on screen even if the backend
  // mirror ever under-counts a role.
  const roleHasMessages = (role: MessageRole) => (roleCounts[role] ?? 0) > 0 || (sessionRoleCounts?.[role] ?? 0) > 0;

  // Navigation over the column-reverse scroller (coordinate model lives in
  // timelineGeometry.ts).
  const scrollToItem = useCallback((index: number, align: "start" | "center" | "end") => {
    const el = messagesRef.current;
    if (el) rowAtEntryIndex(el, index)?.scrollIntoView({ block: align === "center" ? "center" : align });
  }, []);
  const scrollToBottom = useCallback(() => {
    // scrollTop 0 IS the newest message — engine-anchored.
    messagesRef.current?.scrollTo({ top: 0 });
  }, []);
  // Stable ref callback: an inline arrow would change identity every render, so
  // React would re-invoke it (null → el) each time, toggling `messagesEl` and
  // re-firing the open scroll-to-bottom effect — which pins you to the bottom.
  const handleScrollerRef = useCallback((el: HTMLDivElement | null) => {
    messagesRef.current = el;
    setMessagesEl(el);
  }, []);

  const {
    setTotalMessages,
    resolveCompleteSearchMatch,
    revealEntry,
    revealMessageIndex,
    revealNewest,
    scrollToEnd,
    loadOlder,
    loadNewer,
  } = useSessionPagination({
    sessionId: props.session.id,
    filteredEntries,
    windowStart,
    setWindowStart,
    loadedCount: messages.length,
    scrollElement: messagesEl,
    setMessages,
    setMeta,
    withTokenTotals,
    scrollToItem,
    scrollToBottom,
  });

  // In-session search slice: query signals + data-level match locations.
  const {
    sessionSearch,
    setSessionSearch,
    activeSessionSearch,
    searchBarOpen,
    setSearchBarOpen,
    searchMatchIdx,
    matchLocations,
    navigateMatch,
  } = useSessionSearch({
    filteredEntries,
    loading,
    sessionId: props.session.id,
    resolveCompleteSearchMatch,
    revealEntry,
    registerDebounce: (clear) => {
      sessionSearchDebounceRef.current = clear;
    },
  });

  useEffect(() => {
    const sessionId = props.session.id;
    const version = ++loadVersionRef.current;
    const previousRequest = activeOpenRequestRef.current;
    if (previousRequest && previousRequest.sessionId !== sessionId) {
      void cancelSessionLoad(previousRequest.sessionId, previousRequest.requestId).catch((err) => {
        console.warn("cancelSessionLoad failed:", err);
      });
    }

    setLoading(true);
    setError(null);
    setMessages([]);
    setOutline([]);
    setParseWarningCount(0);
    setSessionRoleCounts(null);
    setTotalMessages(0);
    setWindowStart(0);

    void (async () => {
      try {
        for (let attempt = 0; ; attempt++) {
          const requestId = `${sessionId}:open:${version}${attempt ? `:retry${attempt}` : ""}`;
          activeOpenRequestRef.current = { sessionId, requestId };
          try {
            // Initial open fetches meta + newest tail together so one backend
            // load guard owns the parse and one IPC hydrates the view.
            const open = await getSessionOpenWindow(sessionId, -INITIAL_TAIL, INITIAL_TAIL, requestId);
            if (version !== loadVersionRef.current) return;
            const tail = open.window;
            setMeta(open.meta);
            setMessages(tail.messages);
            setParseWarningCount(tail.parse_warning_count ?? 0);
            setTotalMessages(tail.total);
            setWindowStart(tail.start);
            void refreshOutline(sessionId, version);
            return;
          } catch (e) {
            // Superseded: a newer load owns the view now — stay silent.
            if (version !== loadVersionRef.current) return;
            if (isLoadCanceledError(e)) {
              // Canceled while still the CURRENT load: nothing legitimately
              // superseded it, so silently returning would strand an empty
              // view. Retry once (a lost cancel race resolves instantly
              // against the warm cache), then surface the failure.
              if (attempt === 0) {
                console.warn(`current open request for ${sessionId} was canceled; retrying`);
                continue;
              }
              setError(t("session.loadInterrupted"));
              return;
            }
            setError(errorMessage(e));
            return;
          } finally {
            if (activeOpenRequestRef.current?.requestId === requestId) {
              activeOpenRequestRef.current = null;
            }
          }
        }
      } finally {
        if (version === loadVersionRef.current) setLoading(false);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.session.id]);

  // A session opens at its newest messages (and this arms edge prefetch).
  useEffect(() => {
    if (loading || error || !messagesEl) return;
    scrollToEnd();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loading, error, messagesEl, props.session.id]);

  // Keyboard paging. WebKit routes PageUp/PageDown/Home/End to the scroll
  // area under the last CLICK — DOM focus doesn't count — so a freshly
  // opened session ignores paging keys entirely until the user clicks into
  // the timeline. Handle them at the document level for the active tab.
  useEffect(() => {
    if (!props.active) return;
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        (target instanceof HTMLElement && target.isContentEditable)
      ) {
        return;
      }
      const scroller = messagesRef.current;
      if (!scroller) return;
      switch (e.key) {
        case "PageDown":
          scroller.scrollBy({ top: scroller.clientHeight * 0.9 });
          break;
        case "PageUp":
          scroller.scrollBy({ top: -scroller.clientHeight * 0.9 });
          break;
        case "Home":
          scroller.scrollTo({ top: -scroller.scrollHeight });
          break;
        case "End":
          // Not scrollToEnd: after a window recenter the newest messages may
          // not be loaded, and the bottom of the LOADED window is not the end.
          void revealNewest();
          break;
        default:
          return;
      }
      e.preventDefault();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.active]);

  // Cancel server-side parse when this view goes away.
  useEffect(() => {
    return () => {
      const request = activeOpenRequestRef.current;
      if (request) {
        void cancelSessionLoad(request.sessionId, request.requestId).catch((err) => {
          console.warn("cancelSessionLoad failed:", err);
        });
      }
    };
  }, []);

  useEffect(() => {
    return () => {
      sessionSearchDebounceRef.current?.();
    };
  }, []);

  const [showExportDialog, setShowExportDialog] = useState(false);
  const [showAnalyticsDialog, setShowAnalyticsDialog] = useState(false);

  async function refreshOutline(sessionId: string, version: number, attempt = 0) {
    try {
      const nextOutline = await getSessionTurnOutline(sessionId);
      if (version !== loadVersionRef.current || sessionId !== props.session.id) return;
      setOutline(nextOutline.turns);
      setSessionRoleCounts(nextOutline.role_counts);
    } catch (e) {
      if (isLoadCanceledError(e)) {
        // The backend load guard cancels per session id, so a concurrent
        // window fetch (search jump, minimap reveal) can knock out a slow
        // outline parse on huge sessions. The session is still open — retry
        // once things settle instead of silently dropping the minimap.
        if (attempt < 3 && version === loadVersionRef.current) {
          setTimeout(() => {
            if (version === loadVersionRef.current) {
              void refreshOutline(sessionId, version, attempt + 1);
            }
          }, 1200);
        }
        return;
      }
      console.warn("load session outline failed:", e);
    }
  }

  const { starred, toggleFavorite: handleToggleFavorite } = useFavoriteSync(props.session.id);

  // Sync title from props when it changes (e.g. after rename via syncTabsWithTree)
  useEffect(() => {
    setMeta((prev) => ({ ...prev, title: props.session.title }));
  }, [props.session.title]);

  const handleResume = async () => {
    try {
      await resumeSession(props.session.id, terminalApp);
      toast(t("toast.resumed"));
    } catch (_e) {
      toastError(t("toast.resumeFailed"));
    }
  };

  useSessionCommandEvents({
    active: props.active,
    onResume: () => void handleResume(),
    onExport: () => setShowExportDialog(true),
    onFavorite: () => void handleToggleFavorite(),
    onFindNext: () => navigateMatch(1),
    onFindPrev: () => navigateMatch(-1),
    onSessionSearch: () => {
      setSearchBarOpen(true);
      requestAnimationFrame(() => {
        document.querySelector<HTMLInputElement>(".session-search-input")?.focus();
      });
    },
  });

  // Edge prefetch: load the adjacent page well before the viewport reaches
  // the end, so the runway stays ahead of the read position. This is the ONLY
  // scroll work SessionView does — no state updates, so a scroll frame never
  // re-renders this component. Minimap scroll state lives in
  // TimelineMinimapDriver, which listens on the container itself.
  const scrollRafRef = useRef(0);
  const handleTimelineScroll = useCallback(() => {
    if (scrollRafRef.current) return;
    scrollRafRef.current = requestAnimationFrame(() => {
      scrollRafRef.current = 0;
      const el = messagesRef.current;
      if (!el) return;
      if (distanceToOldest(el) < 1500) loadOlder();
      if (distanceToNewest(el) < 1500) loadNewer();
    });
  }, [loadOlder, loadNewer]);
  useEffect(() => () => cancelAnimationFrame(scrollRafRef.current), []);
  // Row observers, fed incrementally by each row's ref callback (registerRow)
  // so a pagination chunk observes only its own rows — re-walking the whole
  // list per chunk was itself an O(all rows) cost on every landing.
  //
  // ResizeObserver — manual scroll anchoring for the NEWEST side. The
  // column-reverse scroller is engine-anchored at the bottom of the content:
  // height changes on the history side (visually above the viewport) never
  // move the view, but a change in a row between the viewport and the newest
  // message shifts the anchored coordinate space under the viewport by that
  // delta (WKWebView has no native anchoring to absorb it). The observer
  // fires after layout and before paint: subtract those deltas from scrollTop
  // in the same frame. A row's first observation only records its baseline
  // (inserts are the pagination code's business).
  //
  // IntersectionObserver — paint-ahead band: `content-visibility: auto` skips
  // off-screen paint, which flashes blank on fast scroll. The wide rootMargin
  // force-paints rows several screens ahead of the viewport (and lets their
  // estimate→real height correction settle off-screen), so a row is ready
  // before it scrolls in.
  const rowObserversRef = useRef<{ ro: ResizeObserver; io: IntersectionObserver } | null>(null);
  const pendingRowsRef = useRef(new Set<HTMLElement>());
  useEffect(() => {
    const root = messagesEl;
    if (!root) return;
    const heights = new WeakMap<Element, number>();
    const ro = new ResizeObserver((entries) => {
      let delta = 0;
      const bottomEdge = viewportBottom(root);
      for (const entry of entries) {
        const row = entry.target as HTMLElement;
        const newHeight = entry.borderBoxSize?.[0]?.blockSize ?? row.getBoundingClientRect().height;
        const oldHeight = heights.get(row);
        heights.set(row, newHeight);
        if (oldHeight === undefined || oldHeight === newHeight) continue;
        // Only rows fully on the newest side (below the viewport) shift the
        // bottom-anchored coordinate space under the view.
        if (row.offsetTop >= bottomEdge) delta += newHeight - oldHeight;
      }
      // Never fight a bottom-edge rubber-band bounce (scrollTop > 0).
      if (delta !== 0 && root.scrollTop <= 0) root.scrollTop -= delta;
    });
    const io = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          (entry.target as HTMLElement).classList.toggle("paint-ahead", entry.isIntersecting);
        }
      },
      { root, rootMargin: "5000px 0px 5000px 0px" },
    );
    rowObserversRef.current = { ro, io };
    // Rows mounted before this effect ran (refs fire before effects).
    for (const row of pendingRowsRef.current) {
      ro.observe(row);
      io.observe(row);
    }
    return () => {
      rowObserversRef.current = null;
      ro.disconnect();
      io.disconnect();
    };
  }, [messagesEl]);
  const registerRow = useCallback((el: HTMLDivElement | null) => {
    if (!el) return undefined;
    pendingRowsRef.current.add(el);
    rowObserversRef.current?.ro.observe(el);
    rowObserversRef.current?.io.observe(el);
    return () => {
      pendingRowsRef.current.delete(el);
      rowObserversRef.current?.ro.unobserve(el);
      rowObserversRef.current?.io.unobserve(el);
    };
  }, []);
  // Paint search highlights when the query, matches, or active match change.
  // Every row is real DOM (content-visibility rendering), so ranges are stable
  // and there is NO need to repaint on scroll — the browser applies the
  // highlight to each row as it paints. Counting stays data-level in the hook.
  const lastScrolledMatchRef = useRef<string | null>(null);
  useEffect(() => {
    if (!activeSessionSearch || !messagesEl) return;
    const frame = requestAnimationFrame(() => {
      const target = activeMatchTarget(matchLocations, searchMatchIdx);
      const activeKey = target !== null ? filteredEntries[target.entryIndex]?.key : undefined;
      const activeRange = paintVisibleHighlights(
        messagesEl,
        activeSessionSearch,
        target !== null && activeKey !== undefined ? { entryKey: activeKey, occurrence: target.occurrence } : null,
      );
      // Center the active match once per navigation step.
      const scrollKey = `${activeSessionSearch}#${searchMatchIdx}`;
      if (activeRange && lastScrolledMatchRef.current !== scrollKey) {
        lastScrolledMatchRef.current = scrollKey;
        scrollRangeIntoView(activeRange);
      }
    });
    return () => cancelAnimationFrame(frame);
  }, [activeSessionSearch, searchMatchIdx, matchLocations, messagesEl, filteredEntries]);

  return (
    <div className="session-view">
      <SessionToolbar
        meta={meta}
        messages={messages}
        starred={starred}
        parseWarningCount={parseWarningCount}
        onToggleFavorite={handleToggleFavorite}
        onAnalyze={() => setShowAnalyticsDialog(true)}
        onResume={handleResume}
        onExport={() => setShowExportDialog(true)}
      />

      {/* Filter toolbar — only show roles that have messages. Counts are the
          SESSION-WIDE numbers from the outline parse; until they arrive the
          buttons show without numbers (the loaded window's counts grow as
          pages land, which reads as the numbers inflating while scrolling). */}
      <div className="session-filter-bar flex items-center gap-1 border-b border-border-subtle px-5 py-1.5">
        <button
          type="button"
          className={`session-focus-toggle${focusMode ? " active" : ""}`}
          aria-pressed={focusMode}
          onClick={() => setFocusMode(!focusMode)}
        >
          {t("session.focus")}
        </button>
        <ToggleGroup
          multiple
          size="sm"
          value={(["user", "assistant", "tool", "system"] as MessageRole[]).filter(
            (role) => roleHasMessages(role) && !hiddenRoles.has(role),
          )}
          onValueChange={(next) => {
            for (const role of ["user", "assistant", "tool", "system"] as MessageRole[]) {
              if (!roleHasMessages(role)) continue;
              const visible = !hiddenRoles.has(role);
              const wanted = next.includes(role);
              if (visible !== wanted) toggleRole(role);
            }
          }}
        >
          {(["user", "assistant", "tool", "system"] as MessageRole[]).filter(roleHasMessages).map((role) => (
            <ToggleGroupItem
              key={role}
              value={role}
              className={`gap-1.5 text-muted-foreground ${ROLE_TOGGLE_COLORS[role]}`}
              disabled={focusMode && (role === "tool" || role === "system")}
            >
              {role === "user"
                ? t("session.filterUser")
                : role === "assistant"
                  ? t("session.filterAssistant")
                  : role === "tool"
                    ? t("session.filterTool")
                    : t("session.filterSystem")}
              {sessionRoleCounts && <span className="text-xs opacity-60 tabular-nums">{sessionRoleCounts[role]}</span>}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      </div>

      {/* In-session search bar */}
      {searchBarOpen && (
        <SessionSearch
          sessionSearch={sessionSearch}
          activeSessionSearch={activeSessionSearch}
          setSessionSearch={setSessionSearch}
          searchMatchIdx={searchMatchIdx}
          matchTotal={matchLocations.length}
          navigateMatch={navigateMatch}
          setSearchBarOpen={setSearchBarOpen}
        />
      )}

      {/* Content */}
      {(loading || !markdownReady) && !error && (
        <div className="session-loading">
          <div className="spinner" />
          <span>{t("session.loading")}</span>
        </div>
      )}

      {error && <div className="session-error">{error}</div>}

      {!loading && markdownReady && !error && (
        <div className="session-messages-container">
          {messages.length === 0 ? (
            <div className="session-messages">
              <div className="session-empty-messages">{t("session.noMessages")}</div>
            </div>
          ) : (
            <TimelineList
              entries={filteredEntries}
              provider={meta.provider}
              parentSessionId={props.session.id}
              registerRow={registerRow}
              scrollerRef={handleScrollerRef}
              onScroll={handleTimelineScroll}
            />
          )}
          <TimelineMinimapDriver
            scrollElement={messagesEl}
            outline={outline}
            entries={filteredEntries}
            onRevealMessage={revealMessageIndex}
          />
        </div>
      )}

      <ExportDialog open={showExportDialog} session={meta} onClose={() => setShowExportDialog(false)} />
      <SessionAnalyticsDialog
        open={showAnalyticsDialog}
        sessionId={props.session.id}
        meta={meta}
        onOpenChange={setShowAnalyticsDialog}
      />
    </div>
  );
}
