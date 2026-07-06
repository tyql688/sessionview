import { useEffect, useMemo, useRef, useState } from "react";
import type {
  SessionRef,
  SessionMeta,
  Message,
  MessageRole,
  TokenTotals,
} from "../../lib/types";
import {
  getSessionOpenWindow,
  getSessionTurnOutline,
  cancelSessionLoad,
  trashSession,
  resumeSession,
  isLoadCanceledError,
  type SessionTurnOutlineEntry,
} from "../../lib/tauri";
import { useI18n } from "../../i18n/index";
import { MessageBubble } from "../MessageBubble";
import { MergedToolRow } from "../MergedToolRow";
import { ConfirmDialog } from "../ConfirmDialog";
import { ExportDialog } from "../ExportDialog";
import { useTerminalApp } from "../../stores/settings";
import { toast, toastError } from "../../stores/toast";
import { errorMessage } from "../../lib/errors";
import { isSearchableRole, processMessages } from "./hooks";
import { SessionToolbar } from "./SessionToolbar";
import { SessionSearch } from "./SessionSearch";
import { TimelineMinimap } from "./TimelineMinimap";
import { useLiveWatch } from "./useLiveWatch";
import { useFavoriteSync } from "./useFavoriteSync";
import { useAutoLoad } from "./useAutoLoad";
import { useSessionCommandEvents } from "./useSessionCommandEvents";
import { useRoleFilter } from "./createRoleFilter";
import { useSessionSearch } from "./createSessionSearch";
import {
  useSessionPagination,
  BATCH_SIZE,
  LOAD_MORE_THRESHOLD,
  INITIAL_TAIL,
} from "./createSessionPagination";

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
  const processedEntries = useMemo(() => processMessages(messages), [messages]);

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
  // State twin of messagesRef: children that need to re-run effects when the
  // scroll container (un)mounts must receive the element as a prop — a bare
  // ref mutation never re-renders, so a snapshot prop goes stale.
  const [messagesEl, setMessagesEl] = useState<HTMLDivElement | null>(null);
  const loadOlderDebounceRef = useRef<(() => void) | undefined>(undefined);
  const sessionSearchDebounceRef = useRef<(() => void) | undefined>(undefined);
  const prevSessionIdRef = useRef<string | null>(null);
  // Latest-value mirror of `messages` so reloadSession — captured by the
  // live-watch effect — reads the current length instead of a stale closure.
  const messagesStateRef = useRef<Message[]>(messages);
  messagesStateRef.current = messages;

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
  const userTurnByMessageIndex = useMemo(() => {
    const turns = new Map<number, number>();
    for (const entry of outline) {
      turns.set(entry.message_index, entry.ordinal);
    }
    return turns;
  }, [outline]);

  // Windowed-loading slice: visibleCount/windowStart/totalMessages signals,
  // visibleEntries/hasMore memos, and the scroll-driven older-page fetch.
  const {
    setVisibleCount,
    setWindowStart,
    setTotalMessages,
    visibleEntries,
    hasMore,
    loadOlderEntries,
    resolveCompleteSearchMatch,
    revealEntry,
    revealMessageIndex,
    handleMessagesScroll,
  } = useSessionPagination({
    sessionId: props.session.id,
    filteredEntries,
    messages,
    getMessagesRef: () => messagesRef.current ?? undefined,
    setMessages,
    setMeta,
    withTokenTotals,
    registerDebounce: (clear) => {
      loadOlderDebounceRef.current = clear;
    },
  });

  // In-session search slice: query/active signals + the pending-consume
  // and debounce effects. Search reveals entries through normal pagination;
  // it does not replace the session's scroll window.
  const {
    sessionSearch,
    setSessionSearch,
    activeSessionSearch,
    searchBarOpen,
    setSearchBarOpen,
    searchMatchIdx,
    setSearchMatchIdx,
  } = useSessionSearch({
    filteredEntries,
    getMessagesRef: () => messagesRef.current ?? undefined,
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
    // Best-effort cancel of the previously in-flight load so the
    // backend parser can bail out instead of running to completion.
    if (prevSessionIdRef.current && prevSessionIdRef.current !== sessionId) {
      void cancelSessionLoad(prevSessionIdRef.current).catch((err) => {
        console.warn("cancelSessionLoad failed:", err);
      });
    }
    prevSessionIdRef.current = sessionId;

    setLoading(true);
    setError(null);
    setMessages([]);
    setOutline([]);
    setParseWarningCount(0);
    setVisibleCount(BATCH_SIZE);
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
        if (version === loadVersionRef.current) setLoading(false);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.session.id]);

  // Cancel server-side parse when this view goes away.
  useEffect(() => {
    return () => {
      if (prevSessionIdRef.current) {
        void cancelSessionLoad(prevSessionIdRef.current).catch((err) => {
          console.warn("cancelSessionLoad failed:", err);
        });
      }
    };
  }, []);

  useEffect(() => {
    return () => {
      loadOlderDebounceRef.current?.();
      sessionSearchDebounceRef.current?.();
    };
  }, []);

  // column-reverse: scrollTop=0 naturally shows newest messages. No scroll-to-bottom needed.

  useAutoLoad({
    visibleEntries,
    loading,
    hasMore,
    getMessagesRef: () => messagesRef.current ?? undefined,
    loadMore: loadOlderEntries,
    threshold: LOAD_MORE_THRESHOLD,
  });

  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [showExportDialog, setShowExportDialog] = useState(false);
  const [watching, setWatching] = useState(false);

  // Stable memos so the live-watch effect only re-runs when these values
  // actually change, not on every reloadSession() → setMeta() cycle.
  const watchProvider = meta.provider;
  const watchSourcePath = meta.source_path || props.session.source_path || "";

  async function refreshOutline(sessionId: string, version: number) {
    try {
      const nextOutline = await getSessionTurnOutline(sessionId);
      if (version !== loadVersionRef.current || sessionId !== props.session.id)
        return;
      setOutline(nextOutline);
    } catch (e) {
      if (isLoadCanceledError(e)) return;
      console.warn("load session outline failed:", e);
    }
  }

  async function reloadSession() {
    try {
      // Refresh meta + tail. Backend cache compares mtime so an actual
      // file change forces a re-parse; otherwise this is O(1) slicing.
      const sessionId = props.session.id;
      const oldCount = messagesStateRef.current.length;
      const open = await getSessionOpenWindow(
        sessionId,
        -INITIAL_TAIL,
        INITIAL_TAIL,
      );
      if (sessionId !== props.session.id) return;
      const tail = open.window;
      setMeta(open.meta);
      setMessages(tail.messages);
      setParseWarningCount(tail.parse_warning_count ?? 0);
      setTotalMessages(tail.total);
      setWindowStart(tail.start);
      void refreshOutline(sessionId, loadVersionRef.current);
      // Auto-scroll to newest if new messages arrived (column-reverse: bottom = scrollTop 0)
      if (tail.messages.length > oldCount) {
        requestAnimationFrame(() => {
          messagesRef.current?.scrollTo({ top: 0, behavior: "smooth" });
        });
      }
    } catch (e) {
      console.error("live watch reload failed:", e);
      toastError(`${t("toast.reloadFailed")}: ${errorMessage(e)}`);
    }
  }

  useLiveWatch({
    watching,
    provider: watchProvider,
    sourcePath: watchSourcePath,
    reload: reloadSession,
  });

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
    onWatch: () => setWatching((v) => !v),
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

  return (
    <div className="session-view">
      <SessionToolbar
        meta={meta}
        messages={messages}
        processedEntries={processedEntries}
        watching={watching}
        starred={starred}
        parseWarningCount={parseWarningCount}
        onToggleWatch={() => setWatching((v) => !v)}
        onToggleFavorite={handleToggleFavorite}
        onResume={handleResume}
        onExport={() => setShowExportDialog(true)}
        onDelete={() => setShowDeleteConfirm(true)}
      />

      {/* Filter toolbar — only show roles that have messages */}
      <div className="filter-toolbar">
        {(["user", "assistant", "tool", "system"] as MessageRole[])
          .filter((r) => (roleCounts[r] || 0) > 0)
          .map((role) => (
            <button
              key={role}
              className={`filter-btn${hiddenRoles.has(role) ? "" : " active"}`}
              onClick={() => toggleRole(role)}
            >
              {role === "user"
                ? t("session.filterUser")
                : role === "assistant"
                  ? t("session.filterAssistant")
                  : role === "tool"
                    ? t("session.filterTool")
                    : t("session.filterSystem")}{" "}
              ({roleCounts[role]})
            </button>
          ))}
      </div>

      {/* In-session search bar */}
      {searchBarOpen && (
        <SessionSearch
          sessionSearch={sessionSearch}
          activeSessionSearch={activeSessionSearch}
          setSessionSearch={setSessionSearch}
          searchMatchIdx={searchMatchIdx}
          setSearchMatchIdx={setSearchMatchIdx}
          setSearchBarOpen={setSearchBarOpen}
          messagesRef={() => messagesRef.current ?? undefined}
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
            onScroll={(e) => handleMessagesScroll(e.nativeEvent)}
          >
            {visibleEntries.map((entry) => {
              if (entry.type === "time-sep") {
                return (
                  <div
                    className="session-entry"
                    data-entry-key={entry.key}
                    key={entry.key}
                  >
                    <div className="msg-time-separator">{entry.time}</div>
                  </div>
                );
              }
              if (entry.type === "merged-tools") {
                return (
                  <div
                    className="session-entry"
                    data-entry-key={entry.key}
                    key={entry.key}
                  >
                    <MergedToolRow
                      tools={entry.tools}
                      messages={entry.messages}
                      provider={meta.provider}
                      parentSessionId={props.session.id}
                      highlightTerm=""
                    />
                  </div>
                );
              }
              return (
                <div
                  className="session-entry"
                  data-entry-key={entry.key}
                  data-turn={userTurnByMessageIndex.get(entry.messageIndex)}
                  key={entry.key}
                >
                  <MessageBubble
                    message={entry.msg}
                    provider={meta.provider}
                    parentSessionId={props.session.id}
                    highlightTerm={
                      isSearchableRole(entry.msg.role)
                        ? activeSessionSearch
                        : ""
                    }
                  />
                </div>
              );
            })}
            {messages.length === 0 && (
              <div className="session-empty-messages">
                {t("session.noMessages")}
              </div>
            )}
          </div>
          <TimelineMinimap
            outline={outline}
            messagesRef={messagesEl}
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
