import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import type { SessionMeta, SessionRef } from "@/lib/types";
import { useI18n } from "@/i18n/index";
import { useGroups } from "@/features/editor/editorGroups";
import { formatTimestamp } from "@/lib/formatters";
import { TabBar } from "@/features/editor/TabBar";
import { SessionView } from "@/features/session";
import { ProviderIcon } from "@/components/icons";
import { isMac } from "@/lib/platform";

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
  const groups = useGroups();
  const [activatedTabIds, setActivatedTabIds] = useState<Set<string>>(new Set());

  useEffect(() => {
    const activeId = props.activeTabId;
    if (!activeId) return;
    setActivatedTabIds((prev) => {
      if (prev.has(activeId)) return prev;
      const next = new Set(prev);
      next.add(activeId);
      return next;
    });
  }, [props.activeTabId]);

  const tabIdsKey = props.tabs.map((tab) => tab.id).join("\0");
  useEffect(() => {
    const openIds = new Set(props.tabs.map((tab) => tab.id));
    setActivatedTabIds((prev) => {
      const next = new Set([...prev].filter((id) => openIds.has(id)));
      return next.size === prev.size ? prev : next;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabIdsKey]);

  const modKey = isMac ? "\u2318" : "Ctrl+";
  const shiftKey = isMac ? "\u21E7" : "Shift+";

  return (
    <div
      className={`editor-area${props.isFocused ? " focused" : ""}`}
      style={{ flexBasis: `${props.flexBasis}%` }}
      onClick={() => props.onFocus()}
    >
      {props.tabs.length > 0 ? (
        <>
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
          <div className="editor-content">
            {props.tabs.map((session) => {
              const isActive = session.id === props.activeTabId;
              const shouldMount = isActive || activatedTabIds.has(session.id);
              return (
                <div
                  key={session.id}
                  className="editor-tab-pane"
                  style={{
                    display: isActive ? "flex" : "none",
                    flex: "1",
                    flexDirection: "column",
                    minHeight: "0",
                  }}
                >
                  {shouldMount && session.id && (
                    <SessionView
                      key={session.id}
                      session={session}
                      // Session commands (resume/export/delete/...) are
                      // document-level events; only the focused group's active
                      // tab may respond, or split view double-fires them.
                      active={isActive && props.isFocused}
                      onRefreshTree={props.onRefreshTree}
                      onCloseTab={props.onTabClose}
                    />
                  )}
                </div>
              );
            })}
          </div>
        </>
      ) : (
        groups.length === 1 && (
          <div className="editor-empty">
            <div className="editor-empty-icon">
              <svg width="48" height="48" fill="none" stroke="currentColor" strokeWidth="1" viewBox="0 0 24 24">
                <path d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z" />
              </svg>
            </div>
            {props.recentSessions && props.recentSessions.length > 0 && (
              <div className="editor-empty-recent">
                <p className="editor-empty-label">{t("editor.recentSessions")}</p>
                {props.recentSessions.map((session) => (
                  <Button
                    key={session.id}
                    variant="ghost"
                    className="editor-empty-session justify-start whitespace-normal active:translate-y-0"
                    onClick={() => props.onOpenSession(session)}
                  >
                    <span className="provider-dot provider-logo" style={{ color: `var(--${session.provider})` }}>
                      <ProviderIcon provider={session.provider} />
                    </span>
                    <div className="editor-empty-session-info">
                      <span className="editor-empty-session-title" title={session.title}>
                        {session.title}
                      </span>
                      <span className="editor-empty-session-meta">
                        <span className="editor-empty-session-path">{session.project_name || ""}</span>
                        {session.model && <span className="editor-empty-session-model">{session.model}</span>}
                        {props.childCounts[session.id] ? (
                          <span className="editor-empty-session-agents">🤖 {props.childCounts[session.id]}</span>
                        ) : null}
                      </span>
                    </div>
                    <span className="editor-empty-session-time">{formatTimestamp(session.updated_at, locale)}</span>
                  </Button>
                ))}
              </div>
            )}
            {props.recentSessionsError && <p className="editor-empty-text">{props.recentSessionsError}</p>}
            {!props.recentSessionsLoading &&
              !props.recentSessionsError &&
              (!props.recentSessions || props.recentSessions.length === 0) && (
                <p className="editor-empty-text">{t("editor.emptyHint")}</p>
              )}
            <div className="editor-empty-shortcuts">
              <span className="editor-shortcut-hint">
                <kbd>
                  {modKey}
                  {shiftKey}F
                </kbd>{" "}
                {t("keyboard.search")}
              </span>
              <span className="editor-shortcut-hint">
                <kbd>{modKey}1-9</kbd> {t("keyboard.switchTab")}
              </span>
            </div>
          </div>
        )
      )}
    </div>
  );
}
