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
import {
  pendingSessionSearch,
  setPendingSessionSearch,
} from "../../stores/search";
import { processMessages } from "./hooks";
import { SessionToolbar } from "./SessionToolbar";
import { SessionSearch } from "./SessionSearch";
import { TimelineMinimap } from "./TimelineMinimap";
import { useLiveWatch } from "./useLiveWatch";
import { useFavoriteSync } from "./useFavoriteSync";
import { useAutoLoad } from "./useAutoLoad";
import {
  SESSION_SEARCH_DEBOUNCE_MS,
  countMatchingEntries,
  findNewestMatchingEntryIndex,
  searchWindowBounds,
} from "./search-utils";

export function SessionView(props: {
  session: SessionRef;
  active: boolean;
  onRefreshTree: () => void;
  onCloseTab: (id: string) => void;
}) {
  const { t } = useI18n();
  const [messages, setMessages] = createSignal<Message[]>([]);
  const processedEntries = createMemo(() => processMessages(messages()));
  const BATCH_SIZE = 80;
  const LOAD_MORE_THRESHOLD = 1;
  const MINIMAP_JUMP_BATCH = 1200;
  const [visibleCount, setVisibleCount] = createSignal(BATCH_SIZE);
  const [hiddenRoles, setHiddenRoles] = createSignal<Set<MessageRole>>(
    new Set(),
  );
  const [sessionSearch, setSessionSearch] = createSignal("");
  const [activeSessionSearch, setActiveSessionSearch] = createSignal("");
  const [searchFocusEntryIndex, setSearchFocusEntryIndex] = createSignal<
    number | null
  >(null);
  const [searchBarOpen, setSearchBarOpen] = createSignal(false);
  const [searchMatchIdx, setSearchMatchIdx] = createSignal(0);
  // Apply role filtering
  const filteredEntries = createMemo(() => {
    const hidden = hiddenRoles();
    if (hidden.size === 0) return processedEntries();
    return processedEntries().filter((e) => {
      if (e.type === "time-sep") return true;
      if (e.type === "merged-tools") return !hidden.has("tool");
      return !hidden.has(e.msg.role);
    });
  });

  // Role counts for filter toolbar
  const roleCounts = createMemo(() => {
    const counts: Record<string, number> = {
      user: 0,
      assistant: 0,
      tool: 0,
      system: 0,
    };
    for (const e of processedEntries()) {
      if (e.type === "message")
        counts[e.msg.role] = (counts[e.msg.role] || 0) + 1;
      else if (e.type === "merged-tools") counts.tool += e.messages.length;
    }
    return counts;
  });

  // Reversed for column-reverse layout: newest first in DOM = visually at bottom.
  // Search keeps the existing render window and expands only enough to reveal
  // the first match. Rendering every entry on each input stalls large sessions.
  const visibleEntries = createMemo(() => {
    const all = filteredEntries();
    const focusedIndex = searchFocusEntryIndex();
    if (activeSessionSearch().trim() && focusedIndex !== null) {
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
  const INITIAL_TAIL = 300;
  const TAIL_BATCH = 600;
  const [totalMessages, setTotalMessages] = createSignal(0);
  const [windowStart, setWindowStart] = createSignal(0);

  // We have more to render if either the in-memory window has unrendered
  // entries OR the backend still holds older messages we haven't fetched.
  const hasMore = createMemo(
    () =>
      visibleCount() < filteredEntries().length ||
      (windowStart() > 0 && messages().length < totalMessages()),
  );
  const searchMatchCount = createMemo(() =>
    countMatchingEntries(filteredEntries(), activeSessionSearch()),
  );
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
  let loadOlderDebounce: ReturnType<typeof setTimeout> | undefined;
  let sessionSearchDebounce: ReturnType<typeof setTimeout> | undefined;
  let suppressNextSearchEffect = false;
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

  // Consume a pending session search set by the global SearchOverlay.
  // Runs after the session finishes loading; applies the query, opens the
  // in-session search bar, and scrolls to the first match.
  createEffect(() => {
    const pending = pendingSessionSearch();
    if (!pending || loading()) return;
    if (pending.sessionId !== props.session.id) return;
    setPendingSessionSearch(null);

    suppressNextSearchEffect = true;
    setSessionSearch(pending.query);
    setSearchBarOpen(true);
    commitSessionSearch(pending.query);
  });

  function toggleRole(role: MessageRole) {
    setHiddenRoles((prev) => {
      const next = new Set(prev);
      if (next.has(role)) next.delete(role);
      else next.add(role);
      return next;
    });
  }

  function focusFirstRenderedSearchMatch() {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (!messagesRef) return;
        const first = messagesRef.querySelector("mark.search-highlight");
        if (!first) return;
        messagesRef
          .querySelector("mark.search-active")
          ?.classList.remove("search-active");
        first.classList.add("search-active");
        first.scrollIntoView({ behavior: "smooth", block: "center" });
      });
    });
  }

  function commitSessionSearch(raw: string) {
    const term = raw.trim();
    setSearchMatchIdx(0);
    if (!term) {
      setActiveSessionSearch("");
      setSearchFocusEntryIndex(null);
      return;
    }

    const entries = filteredEntries();
    const matchIdx = findNewestMatchingEntryIndex(entries, term);
    setSearchFocusEntryIndex(matchIdx >= 0 ? matchIdx : null);
    setActiveSessionSearch(term);
    focusFirstRenderedSearchMatch();
  }

  createEffect(
    on(sessionSearch, (raw) => {
      clearTimeout(sessionSearchDebounce);
      if (suppressNextSearchEffect) {
        suppressNextSearchEffect = false;
        return;
      }
      if (!raw.trim()) {
        commitSessionSearch("");
        return;
      }
      sessionSearchDebounce = setTimeout(
        () => commitSessionSearch(raw),
        SESSION_SEARCH_DEBOUNCE_MS,
      );
    }),
  );

  let olderFetchInFlight = false;

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
    const ref = messagesRef;
    if (!ref) return;
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (ref === messagesRef && ref.scrollTop !== scrollBefore) {
          ref.scrollTop = scrollBefore;
        }
      });
    });
  }

  async function loadOlderTail() {
    if (olderFetchInFlight || windowStart() <= 0) return;
    const sessionId = props.session.id;
    olderFetchInFlight = true;
    const newStart = Math.max(0, windowStart() - TAIL_BATCH);
    const span = windowStart() - newStart;
    const scrollBefore = messagesRef?.scrollTop ?? 0;
    try {
      const older = await getSessionMessagesWindow(sessionId, newStart, span);
      if (sessionId !== props.session.id) return;
      setMeta((prev) => withTokenTotals(prev, older.token_totals));
      // Prepend the newly fetched older messages and grow `visibleCount`
      // by the same amount so the just-fetched entries actually become
      // visible at the top of the viewport (column-reverse layout).
      // Without the bump, `visibleEntries` slices from the newer end and
      // the user sees no change after the round trip.
      setMessages((prev) => [...older.messages, ...prev]);
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
    if (!messagesRef || !hasMore()) return;
    // column-reverse: older entries append at the end of the DOM (visual top).
    // First exhaust the in-memory window via visibleCount, then page in
    // older messages from the backend cache.
    if (visibleCount() < filteredEntries().length) {
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
    clearTimeout(loadOlderDebounce);
    clearTimeout(sessionSearchDebounce);
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
          searchMatchCount={searchMatchCount}
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
                        highlightTerm={activeSessionSearch()}
                      />
                    </div>
                  );
                }
                return (
                  <div class="session-entry" data-entry-key={entry.key}>
                    <MessageBubble
                      message={entry.msg}
                      provider={meta().provider}
                      highlightTerm={activeSessionSearch()}
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
