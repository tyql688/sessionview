import {
  Index,
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
  on,
  onCleanup,
  onMount,
} from "solid-js";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SessionRef, TreeNode } from "../../lib/types";
import {
  getChildSessionCounts,
  invokeWithFallback,
  listRecentSessions,
} from "../../lib/tauri";
import { isPathBlocked } from "../../stores/settings";
import { errorMessage } from "../../lib/errors";
import {
  groups,
  activeGroupId,
  focusGroup,
  setGroupFlexBasis,
  createGroupFromDrop,
} from "../../stores/editorGroups";
import { EditorArea } from "./EditorArea";
import { SplitHandle } from "./SplitHandle";

export function EditorGroupsContainer(props: {
  onTabSelect: (groupId: string, tabId: string) => void;
  onTabClose: (sessionId: string) => void;
  onCloseAllTabs: () => void;
  onCloseOtherTabs: (keepId: string) => void;
  onCloseTabsToRight: (fromId: string) => void;
  onSplitToRight: (sessionId: string) => void;
  onPinTab: (sessionId: string) => void;
  onRefreshTree: () => void;
  tree: TreeNode[];
  onOpenSession: (session: SessionRef) => void;
}) {
  const [dropActive, setDropActive] = createSignal(false);
  const [recentVersion, setRecentVersion] = createSignal(0);
  const [recentSessions] = createResource(recentVersion, async () => {
    const list = await listRecentSessions(100);
    return list
      .filter((s) => !isPathBlocked(s.project_path) && !s.is_sidechain)
      .slice(0, 10);
  });
  const recentSessionsError = createMemo(() =>
    recentSessions.error ? errorMessage(recentSessions.error) : null,
  );
  const [childCounts, setChildCounts] = createSignal<Record<string, number>>(
    {},
  );

  createEffect(
    on(
      () => recentSessions(),
      async (sessions) => {
        if (!sessions || sessions.length === 0) {
          setChildCounts({});
          return;
        }
        const counts = await invokeWithFallback(
          getChildSessionCounts(sessions.map((session) => session.id)),
          {},
          "load child session counts",
        );
        setChildCounts(counts);
      },
      { defer: true },
    ),
  );

  createEffect(
    on(
      () => props.tree,
      () => setRecentVersion((version) => version + 1),
      { defer: true },
    ),
  );

  onMount(() => {
    let unlisten: UnlistenFn | undefined;
    listen<void>("sessions-changed", () =>
      setRecentVersion((version) => version + 1),
    ).then((fn) => {
      unlisten = fn;
    });
    onCleanup(() => unlisten?.());
  });

  function handleResize(leftIdx: number, deltaX: number) {
    const gs = groups();
    const left = gs[leftIdx];
    const right = gs[leftIdx + 1];
    if (!left || !right) return;

    const container = document.querySelector(
      ".editor-groups-container",
    ) as HTMLElement;
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
    const gs = groups();
    const basis = 100 / gs.length;
    for (const g of gs) {
      setGroupFlexBasis(g.id, basis);
    }
  }

  function handleDragOver(e: DragEvent) {
    const container = e.currentTarget as HTMLElement;
    const rect = container.getBoundingClientRect();
    const inDropZone = e.clientX > rect.right - 40;
    setDropActive(inDropZone);
    if (inDropZone) {
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    }
  }

  function handleDrop(e: DragEvent) {
    if (!dropActive()) return;
    e.preventDefault();
    setDropActive(false);
    try {
      const data: unknown = JSON.parse(
        e.dataTransfer?.getData("text/plain") ?? "{}",
      );
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
      class="editor-groups-container"
      onDragOver={handleDragOver}
      onDrop={handleDrop}
      onDragLeave={handleDragLeave}
    >
      <Index each={groups()}>
        {(group, idx) => (
          <>
            <Show when={idx > 0}>
              <SplitHandle
                onResize={(dx) => handleResize(idx - 1, dx)}
                onDoubleClick={equalizeWidths}
              />
            </Show>
            <EditorArea
              groupId={group().id}
              tabs={group().tabs}
              activeTabId={group().activeTabId}
              previewTabId={group().previewTabId}
              isFocused={group().id === activeGroupId()}
              flexBasis={group().flexBasis}
              onFocus={() => focusGroup(group().id)}
              onTabSelect={(tabId) => props.onTabSelect(group().id, tabId)}
              onTabClose={props.onTabClose}
              onCloseAllTabs={props.onCloseAllTabs}
              onCloseOtherTabs={props.onCloseOtherTabs}
              onCloseTabsToRight={props.onCloseTabsToRight}
              onSplitToRight={props.onSplitToRight}
              onPinTab={props.onPinTab}
              onRefreshTree={props.onRefreshTree}
              onOpenSession={props.onOpenSession}
              recentSessions={recentSessions()}
              recentSessionsLoading={recentSessions.loading}
              recentSessionsError={recentSessionsError()}
              childCounts={childCounts()}
            />
          </>
        )}
      </Index>
      <div class={`editor-groups-drop-right${dropActive() ? " active" : ""}`} />
    </div>
  );
}
