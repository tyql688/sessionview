import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Crosshair, Folder, Layers, PanelLeftClose } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type React from "react";
import type { SessionRef, TreeNode } from "@/lib/types";
import {
  getResumeCommand,
  isFavorite,
  resumeSession,
  exportSessionsBatch,
  toggleFavorite,
  renameSession,
  invokeWithFallback,
} from "@/lib/tauri";
import { save } from "@tauri-apps/plugin-dialog";
import { downloadSessionsBatchExport } from "@/lib/headless-download";
import { isTauriRuntime } from "@/lib/runtime";
import { useI18n } from "@/i18n/index";
import {
  useTerminalApp,
  useShowOrphans,
  useBlockedFolders,
  useExplorerGrouping,
  setExplorerGrouping,
  addBlockedFolder,
} from "@/stores/settings";
import { ContextMenu } from "@/components/ContextMenu";
import { InputDialog } from "@/components/InputDialog";
import { TreeNodeComponent, type MenuAnchorEvent } from "@/features/explorer/TreeNode";
import { useIsCompact } from "@/stores/viewport";
import {
  toggleSelected,
  clearSelection,
  selectionCount,
  useSelectionCount,
  useSelectionStore,
} from "@/features/explorer/selection";
import { toast, toastError } from "@/stores/toast";
import { errorMessage } from "@/lib/errors";
import { preferredScrollBehavior } from "@/lib/motion";
import {
  filterBlockedFolders,
  filterOrphanSubagents,
  groupTreeByDirectory,
  buildSessionRef,
} from "@/features/explorer/hooks";
import { buildSessionMenuItems, buildSelectionMenuItems, buildNodeMenuItems } from "@/features/explorer/ContextMenus";

function ExplorerSkeleton() {
  return (
    <div className="skeleton-wrapper">
      {Array.from({ length: 3 }).map((_, i) => (
        <div key={i}>
          <div className="skeleton-tree-item">
            <div className="skeleton skeleton-tree-dot" />
            <div className="skeleton skeleton-tree-text skeleton-tree-text-sm" />
          </div>
          {Array.from({ length: 4 }).map((_, j) => (
            <div key={j} className="skeleton-tree-item skeleton-tree-item-indent">
              <div className="skeleton skeleton-tree-text" />
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}

interface ExplorerProps {
  tree: TreeNode[];
  activeSessionId: string | null;
  onOpenSession: (session: SessionRef) => void;
  onPreviewSession: (session: SessionRef) => void;
  onRefreshTree?: () => void;
  onRefreshProvider?: (provider: SessionRef["provider"]) => void;
  onCollapse?: () => void;
  isLoading?: boolean;
}

export function Explorer(props: ExplorerProps) {
  const { t } = useI18n();
  const showOrphans = useShowOrphans();
  const grouping = useExplorerGrouping();
  const blockedFolders = useBlockedFolders();
  const terminalApp = useTerminalApp();
  const selCount = useSelectionCount();

  const displayTree = useMemo(() => {
    let tree = filterBlockedFolders(props.tree);
    if (!showOrphans) tree = filterOrphanSubagents(tree);
    if (grouping === "directory") {
      tree = groupTreeByDirectory(tree, t("explorer.noProject"));
    }
    return tree;
    // `blockedFolders` drives filterBlockedFolders' internal getBlockedFolders(),
    // so it's a real dep even though not textually referenced here.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.tree, showOrphans, grouping, blockedFolders, t]);

  // O(1) session ID → project path lookup, rebuilt when props.tree changes
  const sessionProjectPathMap = useMemo(() => {
    const map = new Map<string, string>();
    function walk(nodes: TreeNode[], providerHint: string, projectHint: string) {
      for (const node of nodes) {
        if (node.node_type === "session") {
          map.set(node.id, projectHint);
        }
        if (node.children && node.children.length > 0) {
          const nextProvider = node.node_type === "provider" ? (node.provider ?? node.id) : providerHint;
          const nextProject =
            node.node_type === "project" && !providerHint
              ? ""
              : node.node_type === "project" && providerHint && !projectHint
                ? (node.project_path ?? "")
                : projectHint;
          walk(node.children, nextProvider, nextProject);
        }
      }
    }
    walk(props.tree, "", "");
    return map;
  }, [props.tree]);
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set());
  const [focusedNodeId, setFocusedNodeId] = useState<string | null>(null);
  const visibleTreeIds = useMemo(() => {
    const ids = new Set<string>();
    const pending = [...displayTree];
    while (pending.length > 0) {
      const node = pending.pop();
      if (!node) continue;
      ids.add(node.id);
      if (expandedIds.has(node.id)) pending.push(...node.children);
    }
    return ids;
  }, [displayTree, expandedIds]);
  const [initialized, setInitialized] = useState(false);

  // Context menu positions — each stores {x,y} or null
  const [sessionMenu, setSessionMenu] = useState<{
    pos: { x: number; y: number };
    node: TreeNode;
    projectLabel: string;
    resumeCommand: string | null;
    favorite: boolean | null;
  } | null>(null);
  const [nodeMenu, setNodeMenu] = useState<{
    pos: { x: number; y: number };
    node: TreeNode;
  } | null>(null);
  const [selectionMenu, setSelectionMenu] = useState<{
    x: number;
    y: number;
  } | null>(null);
  const [renameTarget, setRenameTarget] = useState<{
    id: string;
    label: string;
  } | null>(null);

  // Auto-expand providers on first load
  useEffect(() => {
    if (props.tree.length > 0 && !initialized) {
      setExpandedIds(new Set(props.tree.map((n) => n.id)));
      setInitialized(true);
    }
  }, [props.tree, initialized]);

  useEffect(() => {
    setFocusedNodeId((current) => {
      if (current && visibleTreeIds.has(current)) return current;
      if (props.activeSessionId && visibleTreeIds.has(props.activeSessionId)) return props.activeSessionId;
      return displayTree[0]?.id ?? null;
    });
  }, [displayTree, props.activeSessionId, visibleTreeIds]);

  // Reveal active session on demand: expand ancestors and scroll into view.
  function revealActiveSession() {
    const sessionId = props.activeSessionId;
    const tree = displayTree;
    if (!sessionId || tree.length === 0) return;

    function findPath(nodes: TreeNode[], target: string): string[] | null {
      for (const node of nodes) {
        if (node.id === target) return [];
        const sub = findPath(node.children, target);
        if (sub !== null) return [node.id, ...sub];
      }
      return null;
    }

    const path = findPath(tree, sessionId);
    if (!path) return;

    setExpandedIds((prev) => {
      const next = new Set(prev);
      for (const id of path) next.add(id);
      return next;
    });
    requestAnimationFrame(() => {
      const el = document.querySelector(`[data-session-id="${sessionId}"]`);
      el?.scrollIntoView({ block: "nearest", behavior: preferredScrollBehavior() });
    });
  }

  function toggleExpanded(nodeId: string) {
    setExpandedIds((prev) => {
      const next = new Set(prev);
      if (next.has(nodeId)) next.delete(nodeId);
      else next.add(nodeId);
      return next;
    });
  }

  function isNodeExpanded(nodeId: string): boolean {
    return expandedIds.has(nodeId);
  }

  function closeAllMenus() {
    setSessionMenu(null);
    setNodeMenu(null);
    setSelectionMenu(null);
  }

  // --- Click handlers ---

  function handleSessionClick(e: React.MouseEvent, node: TreeNode, parentProjectLabel: string) {
    if (e.metaKey || e.ctrlKey) {
      toggleSelected(node.id);
      return;
    }
    clearSelection();
    props.onPreviewSession(buildSessionRef(node, parentProjectLabel));
  }

  function handleSessionDblClick(_e: React.MouseEvent, node: TreeNode, parentProjectLabel: string) {
    props.onOpenSession(buildSessionRef(node, parentProjectLabel));
  }

  // Persist command lookups across renders.
  const resumeCommandCacheRef = useRef(new Map<string, string | null>());

  async function handleSessionContextMenu(e: MenuAnchorEvent, node: TreeNode, parentProjectLabel: string) {
    setNodeMenu(null);
    setSelectionMenu(null);
    const resumeCommandCache = resumeCommandCacheRef.current;
    const sel = useSelectionStore.getState().selectedIds;
    if (sel.size > 1 && sel.has(node.id)) {
      setSessionMenu(null);
      setSelectionMenu({ x: e.clientX, y: e.clientY });
      return;
    }
    let resumeCommand = resumeCommandCache.get(node.id) ?? null;
    if (!resumeCommandCache.has(node.id)) {
      resumeCommand = await invokeWithFallback(
        getResumeCommand(node.id),
        null,
        `load resume command for session ${node.id}`,
      );
      resumeCommandCache.set(node.id, resumeCommand);
    }
    const favorite = await invokeWithFallback(isFavorite(node.id), null, `check favorite state for session ${node.id}`);
    setSessionMenu({
      pos: { x: e.clientX, y: e.clientY },
      node,
      projectLabel: parentProjectLabel,
      resumeCommand,
      favorite,
    });
  }

  function handleNodeContextMenu(e: MenuAnchorEvent, node: TreeNode) {
    setSessionMenu(null);
    // If there are selected sessions, show selection menu instead of node menu
    if (selectionCount() > 0) {
      setNodeMenu(null);
      setSelectionMenu({ x: e.clientX, y: e.clientY });
      return;
    }
    setSelectionMenu(null);
    setNodeMenu({ pos: { x: e.clientX, y: e.clientY }, node });
  }

  // --- Batch operations ---

  function findSessionProjectPath(sessionId: string): string {
    return sessionProjectPathMap.get(sessionId) ?? "";
  }

  async function exportSelectedBatch() {
    const sessionIds = [...useSelectionStore.getState().selectedIds];
    if (sessionIds.length === 0) return;
    try {
      if (!isTauriRuntime) {
        // Browser shell: stream the zip as a download instead of writing to
        // a native-save-dialog path.
        await downloadSessionsBatchExport(sessionIds, "json");
        toast(t("toast.sessionsExported").replace("{count}", String(sessionIds.length)));
        return;
      }
      const outputPath = await save({
        defaultPath: "sessions-export.zip",
        filters: [{ name: "ZIP Archive", extensions: ["zip"] }],
      });
      if (!outputPath) return;

      await exportSessionsBatch(sessionIds, "json", outputPath);
      toast(t("toast.sessionsExported").replace("{count}", String(sessionIds.length)));
    } catch (e) {
      toastError(errorMessage(e));
    }
  }

  // --- Menu item builders ---

  function sessionMenuItems() {
    const m = sessionMenu;
    if (!m) return [];
    return buildSessionMenuItems({
      node: m.node,
      sessionProjectPath: findSessionProjectPath(m.node.id),
      resumeCommand: m.resumeCommand,
      favorite: m.favorite,
      t,
      terminalApp,
      resumeSession,
      toggleFavorite,
      setRenameTarget,
    });
  }

  function selectionMenuItems() {
    return buildSelectionMenuItems({ t, exportSelectedBatch });
  }

  function nodeMenuItems() {
    const m = nodeMenu;
    if (!m) return [];
    return buildNodeMenuItems({
      node: m.node,
      t,
      collapseAllChildren,
      expandAllChildren,
      collapseNode: (nodeId: string) => {
        setExpandedIds((prev) => {
          const next = new Set(prev);
          next.delete(nodeId);
          return next;
        });
      },
      onRefreshTree: props.onRefreshTree,
      onRefreshProvider: props.onRefreshProvider,
      addBlockedFolder,
    });
  }

  function collapseAllChildren(node: TreeNode) {
    setExpandedIds((prev) => {
      const next = new Set(prev);
      function removeAll(n: TreeNode) {
        for (const child of n.children) {
          next.delete(child.id);
          removeAll(child);
        }
      }
      removeAll(node);
      return next;
    });
  }

  function expandAllChildren(node: TreeNode) {
    setExpandedIds((prev) => {
      const next = new Set(prev);
      next.add(node.id);
      for (const child of node.children) {
        next.add(child.id);
      }
      return next;
    });
  }

  // Drag-to-resize handle
  const explorerRef = useRef<HTMLElement>(null);
  const resizeHandleRef = useRef<HTMLHRElement>(null);
  const isCompact = useIsCompact();
  const clampExplorerWidth = (width: number) => Math.max(220, Math.min(width, window.innerWidth * 0.5));
  const applyExplorerWidth = (width?: number) => {
    const explorer = explorerRef.current;
    if (!explorer) return;
    if (width === undefined) explorer.style.removeProperty("width");
    else explorer.style.width = `${width}px`;
    resizeHandleRef.current?.setAttribute("aria-valuenow", String(Math.round(width ?? 280)));
  };

  useEffect(() => {
    if (isCompact) {
      explorerRef.current?.style.removeProperty("width");
      resizeHandleRef.current?.setAttribute("aria-valuenow", "280");
    }
  }, [isCompact]);

  function onResizeStart(event: React.MouseEvent) {
    event.preventDefault();
    const explorer = explorerRef.current;
    if (!explorer) return;
    const startX = event.clientX;
    const startWidth = explorer.offsetWidth;
    const handle = event.currentTarget as HTMLElement;
    handle.classList.add("active");

    function onMove(moveEvent: MouseEvent) {
      applyExplorerWidth(clampExplorerWidth(startWidth + moveEvent.clientX - startX));
    }
    function onUp() {
      handle.classList.remove("active");
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
    }
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }

  function onResizeKeyDown(event: React.KeyboardEvent<HTMLHRElement>) {
    if (event.key === "Enter") {
      applyExplorerWidth();
      event.preventDefault();
      return;
    }
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    const step = event.shiftKey ? 32 : 8;
    const current = explorerRef.current?.offsetWidth ?? 280;
    applyExplorerWidth(clampExplorerWidth(current + (event.key === "ArrowLeft" ? -step : step)));
    event.preventDefault();
  }

  return (
    <aside className="explorer" ref={explorerRef} aria-label={t("explorer.title")}>
      <hr
        ref={resizeHandleRef}
        className="explorer-resize-handle"
        aria-label={t("explorer.resize")}
        aria-orientation="vertical"
        aria-valuemin={220}
        aria-valuemax={Math.max(220, Math.round(window.innerWidth * 0.5))}
        aria-valuenow={280}
        tabIndex={0}
        onFocus={(event) => {
          event.currentTarget.setAttribute("aria-valuemax", String(Math.max(220, Math.round(window.innerWidth * 0.5))));
        }}
        onKeyDown={onResizeKeyDown}
        onMouseDown={onResizeStart}
        onDoubleClick={() => applyExplorerWidth()}
      />
      <div className="explorer-header">
        <span>{t("explorer.title")}</span>
        {selCount > 0 && (
          <span className="count-badge-accent">
            {selCount} {t("explorer.selected")}
          </span>
        )}
        <span className="explorer-header-actions flex items-center gap-0.5">
          <ToggleGroup
            size="sm"
            value={[grouping]}
            onValueChange={(next) => {
              const value = next[0];
              if (value === "provider" || value === "directory") {
                setExplorerGrouping(value);
              }
            }}
          >
            <ToggleGroupItem
              value="provider"
              className="size-6 p-0 text-muted-foreground data-pressed:bg-brand-soft data-pressed:text-brand"
              title={t("explorer.groupByProvider")}
              aria-label={t("explorer.groupByProvider")}
            >
              <Layers className="size-3" aria-hidden="true" />
            </ToggleGroupItem>
            <ToggleGroupItem
              value="directory"
              className="size-6 p-0 text-muted-foreground data-pressed:bg-brand-soft data-pressed:text-brand"
              title={t("explorer.groupByDirectory")}
              aria-label={t("explorer.groupByDirectory")}
            >
              <Folder className="size-3" aria-hidden="true" />
            </ToggleGroupItem>
          </ToggleGroup>
          {props.activeSessionId && (
            <Button
              variant="ghost"
              size="icon-xs"
              title={t("explorer.locateSession")}
              aria-label={t("explorer.locateSession")}
              onClick={revealActiveSession}
            >
              <Crosshair className="size-3.5" aria-hidden="true" />
            </Button>
          )}
          {props.onCollapse && (
            <Button
              variant="ghost"
              size="icon-xs"
              title={t("explorer.hideExplorer")}
              aria-label={t("explorer.hideExplorer")}
              onClick={() => props.onCollapse?.()}
            >
              <PanelLeftClose className="size-3.5" aria-hidden="true" />
            </Button>
          )}
        </span>
      </div>
      <div className="explorer-tree" role="tree" aria-label={t("explorer.sessionsTree")} aria-multiselectable="true">
        {props.isLoading && props.tree.length === 0 && <ExplorerSkeleton />}
        {displayTree.map((node) => (
          <TreeNodeComponent
            key={node.id}
            node={node}
            depth={0}
            activeSessionId={props.activeSessionId}
            focusedNodeId={focusedNodeId}
            onNodeFocus={setFocusedNodeId}
            isNodeExpanded={isNodeExpanded}
            toggleExpanded={toggleExpanded}
            onSessionContextMenu={handleSessionContextMenu}
            onNodeContextMenu={handleNodeContextMenu}
            onSessionClick={handleSessionClick}
            onSessionDblClick={handleSessionDblClick}
            sessionProviderDot={grouping === "directory"}
            directoryGrouping={grouping === "directory"}
          />
        ))}
      </div>

      <ContextMenu items={sessionMenuItems()} position={sessionMenu?.pos ?? null} onClose={closeAllMenus} />
      <ContextMenu items={selectionMenuItems()} position={selectionMenu} onClose={closeAllMenus} />
      <ContextMenu items={nodeMenuItems()} position={nodeMenu?.pos ?? null} onClose={closeAllMenus} />

      <InputDialog
        open={renameTarget !== null}
        title={t("contextMenu.rename")}
        label={t("inputDialog.newTitle")}
        defaultValue={renameTarget?.label ?? ""}
        confirmLabel={t("inputDialog.rename")}
        maxLength={200}
        onConfirm={async (newTitle) => {
          const target = renameTarget;
          if (target) {
            await renameSession(target.id, newTitle);
            setRenameTarget(null);
            props.onRefreshTree?.();
          }
        }}
        onCancel={() => setRenameTarget(null)}
      />
    </aside>
  );
}
