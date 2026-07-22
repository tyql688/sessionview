import { type DragEvent as ReactDragEvent, Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { SessionMeta, SessionRef, TreeNode } from "@/lib/types";
import { getChildSessionCounts, invokeWithFallback, listRecentSessions } from "@/lib/tauri";
import { isPathBlocked } from "@/stores/settings";
import { errorMessage } from "@/lib/errors";
import {
  useGroups,
  useActiveGroupId,
  getGroups,
  focusGroup,
  setGroupFlexBasis,
  createGroupFromDrop,
  mergeAllGroups,
} from "@/features/editor/editorGroups";
import { EditorArea } from "@/features/editor/EditorArea";
import { SplitHandle } from "@/features/editor/SplitHandle";
import { useIsCompact } from "@/stores/viewport";

export function EditorGroupsContainer(props: {
  onTabSelect: (groupId: string, tabId: string) => void;
  onTabClose: (sessionId: string) => void;
  onCloseAllTabs: () => void;
  onCloseOtherTabs: (keepId: string) => void;
  onCloseTabsToRight: (fromId: string) => void;
  onSplitToRight: (sessionId: string) => void;
  onPinTab: (sessionId: string) => void;
  tree: TreeNode[];
  onOpenSession: (session: SessionRef) => void;
}) {
  const groups = useGroups();
  const activeGroupId = useActiveGroupId();
  const isCompact = useIsCompact();
  const [dropActive, setDropActive] = useState(false);

  // Compact layout has no room for split view — collapse splits into a single
  // group whenever the viewport crosses into compact OR a split appears while
  // compact (any path that slips past the disabled split entry points).
  // Layout effect: the merge must land before paint, or two half-width panes
  // flash for a frame.
  const groupCount = groups.length;
  useLayoutEffect(() => {
    if (isCompact && groupCount > 1) mergeAllGroups();
  }, [isCompact, groupCount]);
  const [recentVersion, setRecentVersion] = useState(0);
  const [recentSessions, setRecentSessions] = useState<SessionMeta[] | undefined>(undefined);
  const [recentSessionsLoading, setRecentSessionsLoading] = useState(true);
  const [recentSessionsErrorRaw, setRecentSessionsErrorRaw] = useState<unknown>(null);
  const [childCounts, setChildCounts] = useState<Record<string, number>>({});

  // The welcome page shows WELCOME_RECENT_ROWS sessions, but blocked-path and
  // sidechain filtering happens client-side — oversample the fetch so the list
  // still fills up when many of the newest sessions get filtered out.
  const WELCOME_RECENT_ROWS = 10;
  const RECENT_FETCH_OVERSAMPLE = 100;

  useEffect(() => {
    let cancelled = false;
    setRecentSessionsLoading(true);
    setRecentSessionsErrorRaw(null);
    listRecentSessions(RECENT_FETCH_OVERSAMPLE)
      .then((list) => {
        if (cancelled) return;
        setRecentSessions(
          list.filter((s) => !isPathBlocked(s.project_path) && !s.is_sidechain).slice(0, WELCOME_RECENT_ROWS),
        );
        setRecentSessionsLoading(false);
      })
      .catch((error) => {
        if (cancelled) return;
        setRecentSessionsErrorRaw(error);
        setRecentSessionsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [recentVersion]);

  const recentSessionsError = recentSessionsErrorRaw ? errorMessage(recentSessionsErrorRaw) : null;

  useEffect(() => {
    const sessions = recentSessions;
    if (!sessions || sessions.length === 0) {
      setChildCounts({});
      return;
    }
    let cancelled = false;
    invokeWithFallback(
      getChildSessionCounts(sessions.map((session) => session.id)),
      {},
      "load child session counts",
    ).then((counts) => {
      if (!cancelled) setChildCounts(counts);
    });
    return () => {
      cancelled = true;
    };
  }, [recentSessions]);

  // Skip the initial run and only bump the recent-sessions version when the
  // tree actually changes.
  const treeMounted = useRef(false);
  useEffect(() => {
    if (!treeMounted.current) {
      treeMounted.current = true;
      return;
    }
    setRecentVersion((version) => version + 1);
  }, [props.tree]);

  function handleResize(leftIdx: number, deltaX: number) {
    const gs = getGroups();
    const left = gs[leftIdx];
    const right = gs[leftIdx + 1];
    if (!left || !right) return;

    const container = document.querySelector(".editor-groups-container") as HTMLElement;
    if (!container) return;
    const totalWidth = container.clientWidth;
    const deltaPct = (deltaX / totalWidth) * 100;

    // Clamp delta so neither side goes below 15%, preserving total
    const sum = left.flexBasis + right.flexBasis;
    const maxDelta = right.flexBasis - 15;
    const minDelta = -(left.flexBasis - 15);
    const clamped = Math.max(minDelta, Math.min(maxDelta, deltaPct));
    setGroupFlexBasis(left.id, left.flexBasis + clamped);
    setGroupFlexBasis(right.id, sum - (left.flexBasis + clamped));
  }

  function equalizeWidths() {
    const gs = getGroups();
    const basis = 100 / gs.length;
    for (const g of gs) {
      setGroupFlexBasis(g.id, basis);
    }
  }

  function handleDragOver(e: ReactDragEvent<HTMLDivElement>) {
    const container = e.currentTarget as HTMLElement;
    const rect = container.getBoundingClientRect();
    const inDropZone = e.clientX > rect.right - 40;
    setDropActive(inDropZone);
    if (inDropZone) {
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    }
  }

  function handleDrop(e: ReactDragEvent<HTMLDivElement>) {
    if (!dropActive) return;
    e.preventDefault();
    setDropActive(false);
    try {
      const data: unknown = JSON.parse(e.dataTransfer?.getData("text/plain") ?? "{}");
      const payload = data as { sessionId?: unknown };
      if (typeof payload.sessionId === "string") {
        createGroupFromDrop(payload.sessionId);
      }
    } catch (error) {
      console.warn("Failed to parse split-drop payload:", error);
    }
  }

  function handleDragLeave() {
    setDropActive(false);
  }

  return (
    <div
      className="editor-groups-container"
      onDragOver={isCompact ? undefined : handleDragOver}
      onDrop={isCompact ? undefined : handleDrop}
      onDragLeave={isCompact ? undefined : handleDragLeave}
    >
      {groups.map((group, idx) => (
        <Fragment key={group.id}>
          {idx > 0 && !isCompact && (
            <SplitHandle onResize={(dx) => handleResize(idx - 1, dx)} onDoubleClick={equalizeWidths} />
          )}
          <EditorArea
            groupId={group.id}
            tabs={group.tabs}
            activeTabId={group.activeTabId}
            previewTabId={group.previewTabId}
            isFocused={group.id === activeGroupId}
            flexBasis={group.flexBasis}
            onFocus={() => focusGroup(group.id)}
            onTabSelect={(tabId) => props.onTabSelect(group.id, tabId)}
            onTabClose={props.onTabClose}
            onCloseAllTabs={props.onCloseAllTabs}
            onCloseOtherTabs={props.onCloseOtherTabs}
            onCloseTabsToRight={props.onCloseTabsToRight}
            onSplitToRight={props.onSplitToRight}
            onPinTab={props.onPinTab}
            onOpenSession={props.onOpenSession}
            recentSessions={recentSessions}
            recentSessionsLoading={recentSessionsLoading}
            recentSessionsError={recentSessionsError}
            childCounts={childCounts}
          />
        </Fragment>
      ))}
      {!isCompact && <div className={`editor-groups-drop-right${dropActive ? " active" : ""}`} />}
    </div>
  );
}
