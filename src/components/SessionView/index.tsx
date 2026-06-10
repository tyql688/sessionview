import {
  createSignal,
  createEffect,
  createMemo,
  For,
  Show,
  on,
  onMount,
  onCleanup,
} from "solid-js";
import type {
  SessionRef,
  SessionMeta,
  Message,
  MessageRole,
  TokenTotals,
} from "../../lib/types";
import {
  getSessionMeta,
  getSessionMessagesWindow,
  cancelSessionLoad,
  trashSession,
  resumeSession,
  isLoadCanceledError,
} from "../../lib/tauri";
import { useI18n } from "../../i18n/index";
import { MessageBubble } from "../MessageBubble";
import { MergedToolRow } from "../MergedToolRow";
import { ConfirmDialog } from "../ConfirmDialog";
import { ExportDialog } from "../ExportDialog";
import { terminalApp } from "../../stores/settings";
import { toast, toastError } from "../../stores/toast";
import { errorMessage } from "../../lib/errors";
import { isSearchableRole, processMessages } from "./hooks";
import { SessionToolbar } from "./SessionToolbar";
import { SessionSearch } from "./SessionSearch";
import { TimelineMinimap } from "./TimelineMinimap";
import { useLiveWatch } from "./useLiveWatch";
import { useFavoriteSync } from "./useFavoriteSync";
import { useAutoLoad } from "./useAutoLoad";
import { createRoleFilter } from "./createRoleFilter";
import { createSessionSearch } from "./createSessionSearch";
import {
  createSessionPagination,
  BATCH_SIZE,
  LOAD_MORE_THRESHOLD,
  MINIMAP_JUMP_BATCH,
  INITIAL_TAIL,
} from "./createSessionPagination";

export function SessionView(props: {
  session: SessionRef;
  active: boolean;
  onRefreshTree: () => void;
  onCloseTab: (id: string) => void;
}) {
  const { t } = useI18n();
  const [messages, setMessages] = createSignal<Message[]>([]);
  const processedEntries = createMemo(() => processMessages(messages()));

  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal<string | null>(null);
  const [parseWarningCount, setParseWarningCount] = createSignal(0);
  const [meta, setMeta] = createSignal<SessionMeta>({
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
  });
  let loadVersion = 0;
  let messagesRef: HTMLDivElement | undefined;
  let loadOlderDebounce: (() => void) | undefined;
  let sessionSearchDebounce: (() => void) | undefined;
  let prevSessionId: string | null = null;

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
    createRoleFilter(processedEntries);

  // In-session search slice: query/active/focus signals + the pending-consume
  // and debounce effects. The displayed match total comes from the rendered
  // `<mark>` count inside SessionSearch (same source as Next/Prev navigation).
  const {
    sessionSearch,
    setSessionSearch,
    activeSessionSearch,
    searchFocusEntryIndex,
    searchBarOpen,
    setSearchBarOpen,
    searchMatchIdx,
    setSearchMatchIdx,
  } = createSessionSearch({
    filteredEntries,
    getMessagesRef: () => messagesRef,
    loading,
    sessionId: () => props.session.id,
    registerDebounce: (clear) => {
      sessionSearchDebounce = clear;
    },
  });

  // Windowed-loading slice: visibleCount/windowStart/totalMessages signals,
  // visibleEntries/hasMore memos, and the scroll-driven older-page fetch.
  const {
    visibleCount,
    setVisibleCount,
    setWindowStart,
    setTotalMessages,
    visibleEntries,
    hasMore,
    loadOlderEntries,
    handleMessagesScroll,
  } = createSessionPagination({
    sessionId: () => props.session.id,
    filteredEntries,
    messages,
    getMessagesRef: () => messagesRef,
    searchFocusEntryIndex,
    activeSessionSearch,
    setMessages,
    setMeta,
    withTokenTotals,
    registerDebounce: (clear) => {
      loadOlderDebounce = clear;
    },
  });

  createEffect(
    on(
      () => props.session.id,
      async (sessionId) => {
        const version = ++loadVersion;
        // Best-effort cancel of the previously in-flight load so the
        // backend parser can bail out instead of running to completion.
        if (prevSessionId && prevSessionId !== sessionId) {
          void cancelSessionLoad(prevSessionId).catch((err) => {
            console.warn("cancelSessionLoad failed:", err);
          });
        }
        prevSessionId = sessionId;

        setLoading(true);
        setError(null);
        setMessages([]);
        setParseWarningCount(0);
        setVisibleCount(BATCH_SIZE);
        setTotalMessages(0);
        setWindowStart(0);

        try {
          // Meta first — fast.
          const metaData = await getSessionMeta(sessionId);
          if (version !== loadVersion) return;
          setMeta(metaData);
          // Newest tail next — backend caches the parsed messages so
          // subsequent older-page reads are O(1) slicing.
          const tail = await getSessionMessagesWindow(
            sessionId,
            -INITIAL_TAIL,
            INITIAL_TAIL,
          );
          if (version !== loadVersion) return;
          setMeta(withTokenTotals(metaData, tail.token_totals));
          setMessages(tail.messages);
          setParseWarningCount(tail.parse_warning_count ?? 0);
          setTotalMessages(tail.total);
          setWindowStart(tail.start);
        } catch (e) {
          if (version !== loadVersion) return;
          if (isLoadCanceledError(e)) return; // user navigated away
          setError(errorMessage(e));
        } finally {
          if (version === loadVersion) setLoading(false);
        }
      },
    ),
  );

  // Cancel server-side parse when this view goes away.
  onCleanup(() => {
    if (prevSessionId) {
      void cancelSessionLoad(prevSessionId).catch((err) => {
        console.warn("cancelSessionLoad failed:", err);
      });
    }
  });

  // Global keyboard shortcut listeners — must be inside lifecycle hooks
  const onResume = () => {
    if (props.active) void handleResume();
  };
  const onExport = () => {
    if (props.active) setShowExportDialog(true);
  };
  const onFavorite = () => {
    if (props.active) void handleToggleFavorite();
  };
  const onWatch = () => {
    if (props.active) setWatching((v) => !v);
  };
  const onDelete = () => {
    if (props.active) setShowDeleteConfirm(true);
  };
  const onSessionSearch = () => {
    if (!props.active) return;
    setSearchBarOpen(true);
    requestAnimationFrame(() => {
      (
        document.querySelector(".session-search-input") as HTMLInputElement
      )?.focus();
    });
  };

  onMount(() => {
    document.addEventListener("cc-session:resume", onResume);
    document.addEventListener("cc-session:export", onExport);
    document.addEventListener("cc-session:favorite", onFavorite);
    document.addEventListener("cc-session:watch", onWatch);
    document.addEventListener("cc-session:delete", onDelete);
    document.addEventListener("cc-session:session-search", onSessionSearch);
  });

  onCleanup(() => {
    loadOlderDebounce?.();
    sessionSearchDebounce?.();
    document.removeEventListener("cc-session:resume", onResume);
    document.removeEventListener("cc-session:export", onExport);
    document.removeEventListener("cc-session:favorite", onFavorite);
    document.removeEventListener("cc-session:watch", onWatch);
    document.removeEventListener("cc-session:delete", onDelete);
    document.removeEventListener("cc-session:session-search", onSessionSearch);
  });

  // column-reverse: scrollTop=0 naturally shows newest messages. No scroll-to-bottom needed.

  useAutoLoad({
    visibleEntries,
    loading,
    hasMore,
    getMessagesRef: () => messagesRef,
    loadMore: loadOlderEntries,
    threshold: LOAD_MORE_THRESHOLD,
  });

  const [showDeleteConfirm, setShowDeleteConfirm] = createSignal(false);
  const [showExportDialog, setShowExportDialog] = createSignal(false);
  const [watching, setWatching] = createSignal(false);

  // Stable memos so the live-watch effect only re-runs when these values
  // actually change, not on every reloadSession() → setMeta() cycle.
  const watchProvider = createMemo(() => meta().provider);
  const watchSourcePath = createMemo(
    () => meta().source_path || props.session.source_path || "",
  );

  async function reloadSession() {
    try {
      // Refresh meta + tail. Backend cache compares mtime so an actual
      // file change forces a re-parse; otherwise this is O(1) slicing.
      const sessionId = props.session.id;
      const oldCount = messages().length;
      const [metaData, tail] = await Promise.all([
        getSessionMeta(sessionId),
        getSessionMessagesWindow(sessionId, -INITIAL_TAIL, INITIAL_TAIL),
      ]);
      if (sessionId !== props.session.id) return;
      setMeta(withTokenTotals(metaData, tail.token_totals));
      setMessages(tail.messages);
      setParseWarningCount(tail.parse_warning_count ?? 0);
      setTotalMessages(tail.total);
      setWindowStart(tail.start);
      // Auto-scroll to newest if new messages arrived (column-reverse: bottom = scrollTop 0)
      if (tail.messages.length > oldCount) {
        requestAnimationFrame(() => {
          messagesRef?.scrollTo({ top: 0, behavior: "smooth" });
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
    () => props.session.id,
  );

  // Sync title from props when it changes (e.g. after rename via syncTabsWithTree)
  createEffect(
    on(
      () => props.session.title,
      (newTitle) => {
        setMeta((prev) => ({ ...prev, title: newTitle }));
      },
    ),
  );

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
      await resumeSession(props.session.id, terminalApp());
      toast(t("toast.resumed"));
    } catch (_e) {
      toastError(t("toast.resumeFailed"));
    }
  };

  return (
    <div class="session-view">
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
      <div class="filter-toolbar">
        <For
          each={(
            ["user", "assistant", "tool", "system"] as MessageRole[]
          ).filter((r) => (roleCounts()[r] || 0) > 0)}
        >
          {(role) => (
            <button
              class={`filter-btn${hiddenRoles().has(role) ? "" : " active"}`}
              onClick={() => toggleRole(role)}
            >
              {role === "user"
                ? t("session.filterUser")
                : role === "assistant"
                  ? t("session.filterAssistant")
                  : role === "tool"
                    ? t("session.filterTool")
                    : t("session.filterSystem")}{" "}
              ({roleCounts()[role]})
            </button>
          )}
        </For>
      </div>

      {/* In-session search bar */}
      <Show when={searchBarOpen()}>
        <SessionSearch
          sessionSearch={sessionSearch}
          activeSessionSearch={activeSessionSearch}
          setSessionSearch={setSessionSearch}
          searchMatchIdx={searchMatchIdx}
          setSearchMatchIdx={setSearchMatchIdx}
          setSearchBarOpen={setSearchBarOpen}
          messagesRef={() => messagesRef}
        />
      </Show>

      {/* Content */}
      <Show when={loading()}>
        <div class="session-loading">
          <div class="spinner" />
          <span>{t("session.loading")}</span>
        </div>
      </Show>

      <Show when={error()}>
        <div class="session-error">{error()}</div>
      </Show>

      <Show when={!loading() && !error()}>
        <div class="session-messages-container">
          <div
            class="session-messages"
            ref={messagesRef}
            onScroll={handleMessagesScroll}
          >
            <For each={visibleEntries()}>
              {(entry) => {
                if (entry.type === "time-sep") {
                  return (
                    <div class="session-entry" data-entry-key={entry.key}>
                      <div class="msg-time-separator">{entry.time}</div>
                    </div>
                  );
                }
                if (entry.type === "merged-tools") {
                  return (
                    <div class="session-entry" data-entry-key={entry.key}>
                      <MergedToolRow
                        tools={entry.tools}
                        messages={entry.messages}
                        provider={meta().provider}
                        parentSessionId={props.session.id}
                        highlightTerm=""
                      />
                    </div>
                  );
                }
                return (
                  <div class="session-entry" data-entry-key={entry.key}>
                    <MessageBubble
                      message={entry.msg}
                      provider={meta().provider}
                      parentSessionId={props.session.id}
                      highlightTerm={
                        isSearchableRole(entry.msg.role)
                          ? activeSessionSearch()
                          : ""
                      }
                    />
                  </div>
                );
              }}
            </For>
            <Show when={messages().length === 0}>
              <div class="session-empty-messages">
                {t("session.noMessages")}
              </div>
            </Show>
          </div>
          <TimelineMinimap
            entries={filteredEntries()}
            messagesRef={messagesRef}
            onScrollToFraction={(fraction) => {
              const total = filteredEntries().length;
              const targetCount = Math.min(
                total,
                Math.ceil(total * (1 - fraction)) + BATCH_SIZE,
              );
              if (targetCount > visibleCount()) {
                setVisibleCount((current) =>
                  Math.min(
                    total,
                    Math.min(targetCount, current + MINIMAP_JUMP_BATCH),
                  ),
                );
              }
              // fraction: 0=top(oldest), 1=bottom(newest)
              // column-reverse: scrollTop=0 is bottom, negative is up
              requestAnimationFrame(() => {
                requestAnimationFrame(() => {
                  if (!messagesRef) return;
                  const maxScroll =
                    messagesRef.scrollHeight - messagesRef.clientHeight;
                  messagesRef.scrollTop = -(1 - fraction) * maxScroll;
                });
              });
            }}
          />
        </div>
      </Show>

      <ConfirmDialog
        open={showDeleteConfirm()}
        title={t("confirm.deleteTitle")}
        message={t("confirm.deleteMsg")}
        confirmLabel={t("confirm.confirm")}
        onConfirm={handleDelete}
        onCancel={() => setShowDeleteConfirm(false)}
        danger={true}
      />

      <ExportDialog
        open={showExportDialog()}
        session={meta()}
        onClose={() => setShowExportDialog(false)}
      />
    </div>
  );
}
