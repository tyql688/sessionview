import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { SessionRef, SessionMeta, Message, MessageRole, TokenTotals } from "@/lib/types";
import {
  getSessionOpenWindow,
  getSessionTurnOutline,
  cancelSessionLoad,
  trashSession,
  resumeSession,
  isLoadCanceledError,
  type SessionTurnOutlineEntry,
} from "@/lib/tauri";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useI18n } from "@/i18n/index";
import { MessageBubble } from "@/features/session/MessageBubble";
import { MergedToolRow } from "@/features/session/MergedToolRow";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ExportDialog } from "@/features/session/ExportDialog";
import { setFocusMode, useFocusMode, useTerminalApp } from "@/stores/settings";
import { toast, toastError } from "@/stores/toast";
import { errorMessage } from "@/lib/errors";
import { estimateEntryHeight, isSearchableRole, processMessages } from "@/features/session/hooks";
import { SessionToolbar } from "@/features/session/SessionToolbar";
import { SessionSearch } from "@/features/session/SessionSearch";
import { TimelineMinimap, activeTurnIndex } from "@/features/session/TimelineMinimap";
import { activeMatchTarget, paintVisibleHighlights, scrollRangeIntoView } from "@/features/session/search-utils";
import { useFavoriteSync } from "@/features/session/useFavoriteSync";
import { useSessionCommandEvents } from "@/features/session/useSessionCommandEvents";
import { useRoleFilter } from "@/features/session/createRoleFilter";
import { useSessionSearch } from "@/features/session/createSessionSearch";
import { useSessionPagination, INITIAL_TAIL } from "@/features/session/createSessionPagination";

/** Each role keeps its own pressed color so the filter reads at a glance. */
const ROLE_TOGGLE_COLORS: Record<MessageRole, string> = {
  user: "data-pressed:bg-brand-soft data-pressed:text-brand",
  assistant: "data-pressed:bg-[color-mix(in_srgb,var(--success)_12%,transparent)] data-pressed:text-success",
  tool: "data-pressed:bg-[color-mix(in_srgb,var(--text-warning)_14%,transparent)] data-pressed:text-warning",
  system:
    "data-pressed:bg-[color-mix(in_srgb,var(--accent-secondary)_12%,transparent)] data-pressed:text-(--accent-secondary)",
};

export function SessionView(props: {
  session: SessionRef;
  active: boolean;
  onRefreshTree: () => void;
  onCloseTab: (id: string) => void;
}) {
  const { t } = useI18n();
  const terminalApp = useTerminalApp();
  const focusMode = useFocusMode();
  const [messages, setMessages] = useState<Message[]>([]);
  const [outline, setOutline] = useState<SessionTurnOutlineEntry[]>([]);
  // Absolute session index of messages[0]. Owned here (not by the pagination
  // hook) because processMessages needs it to emit absolute message indices.
  const [windowStart, setWindowStart] = useState(0);
  const processedEntries = useMemo(() => processMessages(messages, windowStart), [messages, windowStart]);

  const [loading, setLoading] = useState(true);
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
  // State twin of messagesRef: the virtualizer and child effects must re-run
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

  // Rendering is content-visibility: every loaded row is real DOM in normal
  // document flow, and the browser skips layout/paint for off-screen rows
  // (`content-visibility: auto` in session.css). Because rows keep their real
  // heights and normal-flow position, the browser's native scroll anchoring
  // absorbs every height correction (a code block painting taller than its
  // reserved estimate, an older page prepending above) — so the read position
  // never jumps, unlike an absolutely-positioned JS virtualizer. Navigation is
  // driven through these plain DOM scroll callbacks.
  const scrollToItem = useCallback((index: number, align: "start" | "center" | "end") => {
    const rows = messagesRef.current?.querySelectorAll<HTMLElement>(".session-entry");
    rows?.[index]?.scrollIntoView({ block: align === "center" ? "center" : align });
  }, []);
  const scrollToBottom = useCallback(() => {
    const align = () => {
      const el = messagesRef.current;
      if (!el) return;
      // Align the last row's bottom edge to the viewport bottom directly, so an
      // estimate-vs-real height gap on the newest rows can't leave a sliver
      // below it. Re-run next frame after those rows paint at real height.
      const rows = el.querySelectorAll<HTMLElement>(".session-entry");
      const last = rows[rows.length - 1];
      if (last) last.scrollIntoView({ block: "end" });
      else el.scrollTo({ top: el.scrollHeight });
    };
    align();
    requestAnimationFrame(align);
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
          scroller.scrollTo({ top: 0 });
          break;
        case "End":
          scrollToEnd();
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

  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [showExportDialog, setShowExportDialog] = useState(false);

  async function refreshOutline(sessionId: string, version: number, attempt = 0) {
    try {
      const nextOutline = await getSessionTurnOutline(sessionId);
      if (version !== loadVersionRef.current || sessionId !== props.session.id) return;
      setOutline(nextOutline);
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

  const handleDelete = async () => {
    try {
      await trashSession(props.session.id);
      setShowDeleteConfirm(false);
      props.onCloseTab(props.session.id);
      props.onRefreshTree();
      toast(t("toast.trashed"));
    } catch (_e) {
      setShowDeleteConfirm(false);
      toastError(t("toast.trashFailed"));
    }
  };

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
    onDelete: () => setShowDeleteConfirm(true),
    onSessionSearch: () => {
      setSearchBarOpen(true);
      requestAnimationFrame(() => {
        document.querySelector<HTMLInputElement>(".session-search-input")?.focus();
      });
    },
  });

  // Minimap active-turn + scroll state, driven by a throttled scroll handler on
  // the real scroll container. The top-visible row (by hit-testing the viewport
  // top) picks the active turn; nearing an edge prefetches the next page; a
  // short idle timer drives the minimap wave. `data-entry-key` on each row maps
  // the DOM node back to a `filteredEntries` position.
  const [topVisibleKey, setTopVisibleKey] = useState<string | null>(null);
  const [lastRowVisible, setLastRowVisible] = useState(false);
  const [timelineScrolling, setTimelineScrolling] = useState(false);
  const scrollRafRef = useRef(0);
  const scrollIdleRef = useRef(0);
  const handleTimelineScroll = useCallback(() => {
    if (scrollRafRef.current) return;
    scrollRafRef.current = requestAnimationFrame(() => {
      scrollRafRef.current = 0;
      const el = messagesRef.current;
      if (!el) return;
      const rect = el.getBoundingClientRect();
      // Hit-test a few points down from the top edge so the container's
      // top padding (which has no row under it) doesn't yield a null key —
      // otherwise the active turn falls back to the last turn at scrollTop 0.
      let row: Element | null = null;
      for (const dy of [6, 24, 48]) {
        const hit = document.elementFromPoint(rect.left + 24, rect.top + dy);
        row = hit instanceof Element ? hit.closest(".session-entry") : null;
        if (row) break;
      }
      // Still nothing (very top padding): the first rendered row is the top one.
      if (!row) row = el.querySelector(".session-entry");
      setTopVisibleKey(row?.getAttribute("data-entry-key") ?? null);
      setLastRowVisible(el.scrollHeight - el.scrollTop - el.clientHeight < 4);
      // Edge prefetch: load the adjacent page well before the viewport reaches
      // the end, so the runway stays ahead of the read position.
      if (el.scrollTop < 1500) loadOlder();
      if (el.scrollHeight - el.scrollTop - el.clientHeight < 1500) loadNewer();
      setTimelineScrolling(true);
      window.clearTimeout(scrollIdleRef.current);
      scrollIdleRef.current = window.setTimeout(() => setTimelineScrolling(false), 160);
    });
  }, [loadOlder, loadNewer]);
  useEffect(() => () => cancelAnimationFrame(scrollRafRef.current), []);
  // Paint-ahead band: `content-visibility: auto` skips off-screen paint, which
  // flashes blank on fast scroll. A wide IntersectionObserver rootMargin
  // force-paints rows several screens ahead of the viewport (and lets their
  // estimate→real height correction settle off-screen), so a row is ready before
  // it scrolls in. Re-observes when the row set changes (pagination / filter).
  useEffect(() => {
    const root = messagesEl;
    if (!root) return;
    const io = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          (entry.target as HTMLElement).classList.toggle("paint-ahead", entry.isIntersecting);
        }
      },
      { root, rootMargin: "5000px 0px 5000px 0px" },
    );
    for (const row of root.querySelectorAll(".session-entry")) io.observe(row);
    return () => io.disconnect();
  }, [messagesEl, filteredEntries]);
  const topVisibleMessageIndex = useMemo(() => {
    if (!topVisibleKey) return null;
    const start = filteredEntries.findIndex((entry) => entry.key === topVisibleKey);
    for (let i = Math.max(0, start); i < filteredEntries.length; i += 1) {
      const entry = filteredEntries[i];
      if (entry?.type === "message") return entry.messageIndex;
      if (entry?.type === "merged-tools") return entry.messageIndices[0];
    }
    return null;
  }, [topVisibleKey, filteredEntries]);
  const activeTurn = activeTurnIndex(outline, topVisibleMessageIndex, lastRowVisible);

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
        onResume={handleResume}
        onExport={() => setShowExportDialog(true)}
        onDelete={() => setShowDeleteConfirm(true)}
      />

      {/* Filter toolbar — only show roles that have messages */}
      <div className="flex items-center gap-1 border-b border-border-subtle px-5 py-1.5">
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
            (role) => (roleCounts[role] || 0) > 0 && !hiddenRoles.has(role),
          )}
          onValueChange={(next) => {
            for (const role of ["user", "assistant", "tool", "system"] as MessageRole[]) {
              if ((roleCounts[role] || 0) === 0) continue;
              const visible = !hiddenRoles.has(role);
              const wanted = next.includes(role);
              if (visible !== wanted) toggleRole(role);
            }
          }}
        >
          {(["user", "assistant", "tool", "system"] as MessageRole[])
            .filter((role) => (roleCounts[role] || 0) > 0)
            .map((role) => (
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
                <span className="text-xs opacity-60 tabular-nums">{roleCounts[role]}</span>
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
      {loading && (
        <div className="session-loading">
          <div className="spinner" />
          <span>{t("session.loading")}</span>
        </div>
      )}

      {error && <div className="session-error">{error}</div>}

      {!loading && !error && (
        <div className="session-messages-container">
          {messages.length === 0 ? (
            <div className="session-messages">
              <div className="session-empty-messages">{t("session.noMessages")}</div>
            </div>
          ) : (
            <div className="session-messages" ref={handleScrollerRef} onScroll={handleTimelineScroll} tabIndex={-1}>
              {filteredEntries.map((entry) => (
                <div
                  key={entry.key}
                  className="session-entry"
                  data-entry-key={entry.key}
                  data-searchable={entry.type === "message" && isSearchableRole(entry.msg.role) ? "" : undefined}
                  // Per-row reserved height so revealing off-screen rows doesn't shift scroll.
                  style={{ containIntrinsicSize: `auto ${estimateEntryHeight(entry)}px` }}
                >
                  {entry.type === "time-sep" ? (
                    <div className="msg-time-separator">{entry.time}</div>
                  ) : entry.type === "merged-tools" ? (
                    <MergedToolRow
                      tools={entry.tools}
                      messages={entry.messages}
                      provider={meta.provider}
                      parentSessionId={props.session.id}
                    />
                  ) : (
                    <MessageBubble message={entry.msg} provider={meta.provider} parentSessionId={props.session.id} />
                  )}
                </div>
              ))}
            </div>
          )}
          <TimelineMinimap
            outline={outline}
            activeIndex={activeTurn}
            scrolling={timelineScrolling}
            onWheelScroll={(deltaY) => {
              messagesRef.current?.scrollBy({ top: deltaY });
            }}
            onRevealMessage={revealMessageIndex}
          />
        </div>
      )}

      <ConfirmDialog
        open={showDeleteConfirm}
        title={t("confirm.deleteTitle")}
        message={t("confirm.deleteMsg")}
        confirmLabel={t("confirm.confirm")}
        onConfirm={handleDelete}
        onCancel={() => setShowDeleteConfirm(false)}
        danger={true}
      />

      <ExportDialog open={showExportDialog} session={meta} onClose={() => setShowExportDialog(false)} />
    </div>
  );
}
