import { useEffect, useMemo, useRef, useState } from "react";
import type {
  SessionRef,
  SessionMeta,
  Message,
  MessageRole,
  TokenTotals,
} from "@/lib/types";
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
import { useTerminalApp } from "@/stores/settings";
import { toast, toastError } from "@/stores/toast";
import { errorMessage } from "@/lib/errors";
import { isSearchableRole, processMessages } from "@/features/session/hooks";
import { SessionToolbar } from "@/features/session/SessionToolbar";
import { SessionSearch } from "@/features/session/SessionSearch";
import {
  TimelineMinimap,
  activeTurnIndex,
} from "@/features/session/TimelineMinimap";
import {
  activeMatchTarget,
  paintVisibleHighlights,
  scrollRangeIntoView,
} from "@/features/session/search-utils";
import { useFavoriteSync } from "@/features/session/useFavoriteSync";
import { useSessionCommandEvents } from "@/features/session/useSessionCommandEvents";
import { useRoleFilter } from "@/features/session/createRoleFilter";
import { useSessionSearch } from "@/features/session/createSessionSearch";
import {
  useSessionPagination,
  INITIAL_TAIL,
} from "@/features/session/createSessionPagination";

/** Each role keeps its own pressed color so the filter reads at a glance. */
const ROLE_TOGGLE_COLORS: Record<MessageRole, string> = {
  user: "data-pressed:bg-brand-soft data-pressed:text-brand",
  assistant:
    "data-pressed:bg-[color-mix(in_srgb,var(--success)_12%,transparent)] data-pressed:text-success",
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
  const [messages, setMessages] = useState<Message[]>([]);
  const [outline, setOutline] = useState<SessionTurnOutlineEntry[]>([]);
  // Absolute session index of messages[0]. Owned here (not by the pagination
  // hook) because processMessages needs it to emit absolute message indices.
  const [windowStart, setWindowStart] = useState(0);
  const processedEntries = useMemo(
    () => processMessages(messages, windowStart),
    [messages, windowStart],
  );

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

  function withTokenTotals(
    metaData: SessionMeta,
    totals: TokenTotals,
  ): SessionMeta {
    return {
      ...metaData,
      input_tokens: totals.input_tokens,
      output_tokens: totals.output_tokens,
      cache_read_tokens: totals.cache_read_tokens,
      cache_write_tokens: totals.cache_write_tokens,
    };
  }

  // Role-filter slice: hiddenRoles + filteredEntries + roleCounts.
  const { hiddenRoles, roleCounts, filteredEntries, toggleRole } =
    useRoleFilter(processedEntries);

  // Virtualized-scrolling slice: renders only the on-screen rows, pages
  // older messages in from the backend as the viewport nears the top.
  const {
    virtualizer,
    setTotalMessages,
    resolveCompleteSearchMatch,
    revealEntry,
    revealMessageIndex,
    scrollToEnd,
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
    const requestId = `${sessionId}:open:${version}`;
    const previousRequest = activeOpenRequestRef.current;
    if (previousRequest && previousRequest.sessionId !== sessionId) {
      void cancelSessionLoad(
        previousRequest.sessionId,
        previousRequest.requestId,
      ).catch((err) => {
        console.warn("cancelSessionLoad failed:", err);
      });
    }
    activeOpenRequestRef.current = { sessionId, requestId };

    setLoading(true);
    setError(null);
    setMessages([]);
    setOutline([]);
    setParseWarningCount(0);
    setTotalMessages(0);
    setWindowStart(0);

    void (async () => {
      try {
        // Initial open fetches meta + newest tail together so one backend
        // load guard owns the parse and one IPC hydrates the view.
        const open = await getSessionOpenWindow(
          sessionId,
          -INITIAL_TAIL,
          INITIAL_TAIL,
          requestId,
        );
        if (version !== loadVersionRef.current) return;
        const tail = open.window;
        setMeta(open.meta);
        setMessages(tail.messages);
        setParseWarningCount(tail.parse_warning_count ?? 0);
        setTotalMessages(tail.total);
        setWindowStart(tail.start);
        void refreshOutline(sessionId, version);
      } catch (e) {
        if (version !== loadVersionRef.current) return;
        if (isLoadCanceledError(e)) return; // user navigated away
        setError(errorMessage(e));
      } finally {
        if (activeOpenRequestRef.current?.requestId === requestId) {
          activeOpenRequestRef.current = null;
        }
        if (version === loadVersionRef.current) setLoading(false);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.session.id]);

  // A session opens at its newest messages. Depends on the scroll element
  // too: the messages container only mounts after loading flips false.
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
        void cancelSessionLoad(request.sessionId, request.requestId).catch(
          (err) => {
            console.warn("cancelSessionLoad failed:", err);
          },
        );
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

  async function refreshOutline(
    sessionId: string,
    version: number,
    attempt = 0,
  ) {
    try {
      const nextOutline = await getSessionTurnOutline(sessionId);
      if (version !== loadVersionRef.current || sessionId !== props.session.id)
        return;
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

  const { starred, toggleFavorite: handleToggleFavorite } = useFavoriteSync(
    props.session.id,
  );

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
        document
          .querySelector<HTMLInputElement>(".session-search-input")
          ?.focus();
      });
    },
  });

  const virtualItems = virtualizer.getVirtualItems();

  // Minimap position, computed from the virtualizer's rendered range — pure
  // data, no DOM measurement anywhere on the scroll path.
  const topVisibleMessageIndex = useMemo(() => {
    for (const item of virtualItems) {
      const entry = filteredEntries[item.index];
      if (!entry) continue;
      if (entry.type === "message") return entry.messageIndex;
      if (entry.type === "merged-tools") return entry.messageIndices[0];
    }
    return null;
  }, [virtualItems, filteredEntries]);
  const lastRowVisible =
    filteredEntries.length > 0 &&
    virtualItems.some((item) => item.index === filteredEntries.length - 1);
  const activeTurn = activeTurnIndex(
    outline,
    topVisibleMessageIndex,
    lastRowVisible,
  );

  // Repaint search highlights over the mounted rows whenever the rendered
  // window, the query, or the active match changes. Ranges live on real DOM
  // nodes, and virtual rows mount/unmount as the user scrolls — so painting
  // is a recurring pass, while counting stays data-level in the search hook.
  const renderedRangeSignature = `${virtualItems[0]?.key ?? ""}:${virtualItems.length}`;
  const lastScrolledMatchRef = useRef<string | null>(null);
  useEffect(() => {
    if (!activeSessionSearch || !messagesEl) return;
    const frame = requestAnimationFrame(() => {
      const target = activeMatchTarget(matchLocations, searchMatchIdx);
      const activeKey =
        target !== null ? filteredEntries[target.entryIndex]?.key : undefined;
      const activeRange = paintVisibleHighlights(
        messagesEl,
        activeSessionSearch,
        target !== null && activeKey !== undefined
          ? { entryKey: activeKey, occurrence: target.occurrence }
          : null,
      );
      // Center the active match once per navigation step — not on every
      // scroll repaint, which would hijack free scrolling.
      const scrollKey = `${activeSessionSearch}#${searchMatchIdx}`;
      if (activeRange && lastScrolledMatchRef.current !== scrollKey) {
        lastScrolledMatchRef.current = scrollKey;
        scrollRangeIntoView(activeRange);
      }
    });
    return () => cancelAnimationFrame(frame);
  }, [
    activeSessionSearch,
    searchMatchIdx,
    matchLocations,
    messagesEl,
    filteredEntries,
    renderedRangeSignature,
  ]);

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
        <ToggleGroup
          multiple
          size="sm"
          value={(
            ["user", "assistant", "tool", "system"] as MessageRole[]
          ).filter(
            (role) => (roleCounts[role] || 0) > 0 && !hiddenRoles.has(role),
          )}
          onValueChange={(next) => {
            for (const role of [
              "user",
              "assistant",
              "tool",
              "system",
            ] as MessageRole[]) {
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
              >
                {role === "user"
                  ? t("session.filterUser")
                  : role === "assistant"
                    ? t("session.filterAssistant")
                    : role === "tool"
                      ? t("session.filterTool")
                      : t("session.filterSystem")}
                <span className="text-xs opacity-60 tabular-nums">
                  {roleCounts[role]}
                </span>
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
          <div
            className="session-messages"
            ref={(el) => {
              messagesRef.current = el;
              setMessagesEl(el);
            }}
          >
            <div
              className="session-messages-inner"
              style={{ height: `${virtualizer.getTotalSize()}px` }}
            >
              {virtualItems.map((item) => {
                const entry = filteredEntries[item.index];
                if (!entry) return null;
                return (
                  <div
                    className="session-entry"
                    key={entry.key}
                    data-index={item.index}
                    data-entry-key={entry.key}
                    // DOM-level search painting (CSS Custom Highlight API)
                    // only scans subtrees tagged searchable: user +
                    // assistant dialogue.
                    data-searchable={
                      entry.type === "message" &&
                      isSearchableRole(entry.msg.role)
                        ? ""
                        : undefined
                    }
                    ref={virtualizer.measureElement}
                    style={{ transform: `translateY(${item.start}px)` }}
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
                      <MessageBubble
                        message={entry.msg}
                        provider={meta.provider}
                        parentSessionId={props.session.id}
                      />
                    )}
                  </div>
                );
              })}
            </div>
            {messages.length === 0 && (
              <div className="session-empty-messages">
                {t("session.noMessages")}
              </div>
            )}
          </div>
          <TimelineMinimap
            outline={outline}
            activeIndex={activeTurn}
            scrolling={virtualizer.isScrolling}
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

      <ExportDialog
        open={showExportDialog}
        session={meta}
        onClose={() => setShowExportDialog(false)}
      />
    </div>
  );
}
