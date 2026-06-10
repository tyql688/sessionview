import {
  Show,
  For,
  Index,
  createMemo,
  createSignal,
  createEffect,
  on,
} from "solid-js";
import type { SessionMeta, SessionRef } from "../../lib/types";
import { useI18n } from "../../i18n/index";
import { groups } from "../../stores/editorGroups";
import { formatTimestamp } from "../../lib/formatters";
import { TabBar } from "./TabBar";
import { SessionView } from "../SessionView";
import { ProviderIcon } from "../icons";
import { isMac } from "../../lib/platform";

export function EditorArea(props: {
  groupId: string;
  tabs: SessionRef[];
  activeTabId: string | null;
  previewTabId: string | null;
  isFocused: boolean;
  flexBasis: number;
  onFocus: () => void;
  onTabSelect: (id: string) => void;
  onTabClose: (id: string) => void;
  onCloseAllTabs: () => void;
  onCloseOtherTabs: (keepId: string) => void;
  onCloseTabsToRight: (fromId: string) => void;
  onSplitToRight: (sessionId: string) => void;
  onPinTab: (sessionId: string) => void;
  onRefreshTree: () => void;
  onOpenSession: (session: SessionRef) => void;
  recentSessions: SessionMeta[] | undefined;
  recentSessionsLoading: boolean;
  recentSessionsError: string | null;
  childCounts: Record<string, number>;
}) {
  const { t, locale } = useI18n();
  const [activatedTabIds, setActivatedTabIds] = createSignal<Set<string>>(
    new Set(),
  );

  createEffect(() => {
    const activeId = props.activeTabId;
    if (!activeId) return;
    setActivatedTabIds((prev) => {
      if (prev.has(activeId)) return prev;
      const next = new Set(prev);
      next.add(activeId);
      return next;
    });
  });

  createEffect(
    on(
      () => props.tabs.map((tab) => tab.id).join("\0"),
      () => {
        const openIds = new Set(props.tabs.map((tab) => tab.id));
        setActivatedTabIds((prev) => {
          const next = new Set([...prev].filter((id) => openIds.has(id)));
          return next.size === prev.size ? prev : next;
        });
      },
    ),
  );

  const modKey = isMac ? "\u2318" : "Ctrl+";

  return (
    <div
      class={`editor-area${props.isFocused ? " focused" : ""}`}
      style={{ "flex-basis": `${props.flexBasis}%` }}
      onClick={() => props.onFocus()}
    >
      <Show
        when={props.tabs.length > 0}
        fallback={
          <Show when={groups().length === 1}>
            <div class="editor-empty">
              <div class="editor-empty-icon">
                <svg
                  width="48"
                  height="48"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1"
                  viewBox="0 0 24 24"
                >
                  <path d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z" />
                </svg>
              </div>
              <Show
                when={props.recentSessions && props.recentSessions.length > 0}
              >
                <div class="editor-empty-recent">
                  <p class="editor-empty-label">{t("editor.recentSessions")}</p>
                  <For each={props.recentSessions}>
                    {(session) => (
                      <button
                        class="editor-empty-session"
                        onClick={() => props.onOpenSession(session)}
                      >
                        <span
                          class="provider-dot provider-logo"
                          style={{ color: `var(--${session.provider})` }}
                        >
                          <ProviderIcon provider={session.provider} />
                        </span>
                        <div class="editor-empty-session-info">
                          <span class="editor-empty-session-title">
                            {session.title}
                          </span>
                          <span class="editor-empty-session-meta">
                            <span class="editor-empty-session-path">
                              {session.project_name || ""}
                            </span>
                            <Show when={session.model}>
                              <span class="editor-empty-session-model">
                                {session.model}
                              </span>
                            </Show>
                            <Show when={props.childCounts[session.id]}>
                              <span class="editor-empty-session-agents">
                                🤖 {props.childCounts[session.id]}
                              </span>
                            </Show>
                          </span>
                        </div>
                        <span class="editor-empty-session-time">
                          {formatTimestamp(session.updated_at, locale())}
                        </span>
                      </button>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={props.recentSessionsError}>
                <p class="editor-empty-text">{props.recentSessionsError}</p>
              </Show>
              <Show
                when={
                  !props.recentSessionsLoading &&
                  !props.recentSessionsError &&
                  (!props.recentSessions || props.recentSessions.length === 0)
                }
              >
                <p class="editor-empty-text">{t("editor.emptyHint")}</p>
              </Show>
              <div class="editor-empty-shortcuts">
                <span class="editor-shortcut-hint">
                  <kbd>⇧{modKey}F</kbd> {t("keyboard.search")}
                </span>
                <span class="editor-shortcut-hint">
                  <kbd>{modKey}1-9</kbd> {t("keyboard.switchTab")}
                </span>
              </div>
            </div>
          </Show>
        }
      >
        <TabBar
          groupId={props.groupId}
          tabs={props.tabs}
          activeTabId={props.activeTabId}
          previewTabId={props.previewTabId}
          onTabSelect={props.onTabSelect}
          onTabClose={props.onTabClose}
          onCloseAllTabs={props.onCloseAllTabs}
          onCloseOtherTabs={props.onCloseOtherTabs}
          onCloseTabsToRight={props.onCloseTabsToRight}
          onSplitToRight={props.onSplitToRight}
          onPinTab={props.onPinTab}
        />
        <div class="editor-content">
          <Index each={props.tabs}>
            {(session) => {
              const isActive = createMemo(
                () => session().id === props.activeTabId,
              );
              const shouldMount = createMemo(
                () => isActive() || activatedTabIds().has(session().id),
              );
              return (
                <div
                  class="editor-tab-pane"
                  style={{
                    display: isActive() ? "flex" : "none",
                    flex: "1",
                    "flex-direction": "column",
                    "min-height": "0",
                  }}
                >
                  <Show when={shouldMount() && session().id} keyed>
                    {(_id) => (
                      <SessionView
                        session={session()}
                        active={isActive()}
                        onRefreshTree={props.onRefreshTree}
                        onCloseTab={props.onTabClose}
                      />
                    )}
                  </Show>
                </div>
              );
            }}
          </Index>
        </div>
      </Show>
    </div>
  );
}
