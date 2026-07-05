import {
  createSignal,
  createMemo,
  onMount,
  onCleanup,
  Show,
  ErrorBoundary,
} from "solid-js";
import { listenBackendEvent, type UnlistenFn } from "../lib/backend-events";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ActivityBar } from "../components/ActivityBar";
import { Explorer } from "../components/Explorer";
import { EditorGroupsContainer } from "../components/Editor/EditorGroupsContainer";
import { StatusBar } from "../components/StatusBar";
import { SearchOverlay } from "../components/SearchOverlay";
import { SettingsPanel } from "../components/SettingsPanel";
import { TrashView } from "../components/TrashView";

import { FavoritesView } from "../components/FavoritesView";
import { BlockedView } from "../components/BlockedView";
import { UsagePanel } from "../components/UsagePanel";
import { KeyboardOverlay } from "../components/KeyboardOverlay";
import { ToastContainer } from "../components/ToastContainer";
import {
  trashSession,
  getChildSessions,
  startRebuildIndex,
  getIndexStats,
  getTodayCost,
  getTodayTokens,
  invokeWithFallback,
} from "../lib/tauri";
import { isMac, isWindows } from "../lib/platform";
import { disabledProviders } from "../stores/settings";
import { loadProviderSnapshots } from "../stores/providerSnapshots";
import { toast, toastError, toastInfo } from "../stores/toast";
import { checkForUpdate } from "../stores/updater";
import {
  groups,
  activeGroup,
  openSession,
  openPreview,
  pinTab,
  closeTab,
  closeAllTabs,
  closeOtherTabs,
  closeTabsToRight,
  splitToRight,
  setActiveTabInGroup,
  focusGroup,
  focusAdjacentGroup,
  syncAllTabTitles,
} from "../stores/editorGroups";
import type { TreeNode, Provider } from "../lib/types";
import { useI18n } from "../i18n";
import { createKeyboardHandler } from "./KeyboardShortcuts";
import { createSyncManager } from "./SyncManager";
import { createOpenSubagentHandler } from "./SubagentOpen";
import { TitleBar } from "./TitleBar";
import "../styles/index.css";

// Linux derived locally (platform.ts is intentionally minimal). On Linux the
// app hides native decorations like Windows, so it needs the same custom
// min/max/close window controls.
const isLinux =
  typeof navigator !== "undefined" && /Linux/.test(navigator.platform);
const showWindowControls = isWindows || isLinux;

export default function App() {
  const { t } = useI18n();
  const [tree, setTree] = createSignal<TreeNode[]>([]);
  const [sessionCount, setSessionCount] = createSignal(0);
  const [activeView, setActiveView] = createSignal("explorer");
  const [isLoading, setIsLoading] = createSignal(true);
  const [showKeyboardOverlay, setShowKeyboardOverlay] = createSignal(false);
  const [showSearchOverlay, setShowSearchOverlay] = createSignal(false);
  const [sidebarCollapsed, setSidebarCollapsed] = createSignal(false);
  const [isMaximized, setIsMaximized] = createSignal(false);
  const [lastScanTime, setLastScanTime] = createSignal<number | undefined>();
  const [todayCost, setTodayCost] = createSignal<number | undefined>();
  const [todayTokens, setTodayTokens] = createSignal<
    | { input: number; output: number; cache_read: number; cache_write: number }
    | undefined
  >();

  async function refreshStatusBarStats() {
    const [stats, cost, tokens] = await Promise.all([
      invokeWithFallback(
        getIndexStats(),
        undefined,
        "refresh status bar index stats",
      ),
      invokeWithFallback(getTodayCost(), undefined, "refresh today cost"),
      invokeWithFallback(getTodayTokens(), undefined, "refresh today tokens"),
    ]);

    const ts = stats?.last_index_time
      ? Number(stats.last_index_time)
      : undefined;
    setLastScanTime(ts);
    setTodayCost(cost);
    setTodayTokens(tokens);
  }

  const handleUsageChanged = () => void refreshStatusBarStats();

  const debouncedChangedPaths = new Set<string>();

  let unlistenWatcher: UnlistenFn | undefined;
  let unlistenMaintenance: UnlistenFn | undefined;
  let unlistenResized: UnlistenFn | undefined;
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;

  const sync = createSyncManager({
    setTree,
    setSessionCount,
    setIsLoading,
    syncTabsWithTree: (treeData: TreeNode[]) => {
      const titleMap = new Map<string, string>();
      function walk(node: TreeNode) {
        if (node.node_type === "session") titleMap.set(node.id, node.label);
        for (const child of node.children) walk(child);
      }
      for (const n of treeData) walk(n);
      syncAllTabTitles(titleMap);
    },
  });

  const handleGlobalKeyDown = createKeyboardHandler({
    activeTabId: () => activeGroup()?.activeTabId ?? null,
    openTabs: () => activeGroup()?.tabs ?? [],
    showKeyboardOverlay,
    setActiveTabId: (id: string | null) => {
      const g = activeGroup();
      if (g && id) setActiveTabInGroup(g.id, id);
    },
    setShowKeyboardOverlay,
    setShowSearchOverlay,
    setActiveView,
    closeTab,
    closeAllTabs,
    splitToRight,
    focusAdjacentGroup,
    startRebuildIndex: () => {
      void startRebuildIndex();
    },
    syncFromDisk: sync.syncFromDisk,
  });

  onMount(async () => {
    if (isMac) {
      document.documentElement.style.setProperty("--titlebar-inset", "78px");
    }

    // Track maximize state so the custom (Windows/Linux) maximize button can
    // swap to a "restore" glyph. macOS uses native traffic lights, so skip it.
    if (showWindowControls) {
      const win = getCurrentWindow();
      try {
        setIsMaximized(await win.isMaximized());
        unlistenResized = await win.onResized(async () => {
          try {
            setIsMaximized(await win.isMaximized());
          } catch (error) {
            console.error("Failed to read window maximize state:", error);
          }
        });
      } catch (error) {
        console.error("Failed to initialize window maximize tracking:", error);
      }
    }

    window.addEventListener("usage-data-changed", handleUsageChanged);
    void loadProviderSnapshots();
    void sync.coldStart();
    setTimeout(() => void checkForUpdate(), 2000);

    document.addEventListener("keydown", handleGlobalKeyDown);

    const handleOpenSubagent = createOpenSubagentHandler({
      getActiveParentSessionIds: () =>
        groups()
          .map((g) => g.activeTabId)
          .filter((id): id is string => id != null),
      getChildSessions,
      openSession,
      onLoadFailed: () => toastError(t("toast.subagentLoadFailed")),
      onNotFound: () => toastError(t("toast.subagentNotFound")),
      onChildSessionLoadError: (parentId, error) => {
        console.error(
          `Failed to load child sessions for parent ${parentId}:`,
          error,
        );
      },
    });
    window.addEventListener("open-subagent", handleOpenSubagent);

    unlistenWatcher = await listenBackendEvent(
      "sessions-changed",
      (payload) => {
        for (const path of payload ?? []) {
          if (path.length > 0) {
            debouncedChangedPaths.add(path);
          }
        }
        clearTimeout(debounceTimer);
        debounceTimer = setTimeout(() => {
          const changedPaths = [...debouncedChangedPaths];
          debouncedChangedPaths.clear();
          void sync.syncFromDisk({ changedPaths });
        }, 500);
      },
    );

    unlistenMaintenance = await listenBackendEvent(
      "maintenance-status",
      (payload) => {
        if (payload.phase === "started") {
          const message =
            payload.job === "refresh_usage"
              ? t("toast.refreshUsageStarted")
              : t("toast.rebuildStarted");
          toastInfo(message);
          return;
        }

        if (payload.phase === "failed") {
          toastError(payload.message || t("toast.rebuildFailed"));
          return;
        }

        if (payload.phase === "finished") {
          void sync.refreshTree();
          void loadProviderSnapshots(true);
          const message =
            payload.job === "refresh_usage"
              ? t("toast.refreshUsageOk")
              : t("toast.rebuildOk");
          toast(message);
        }
      },
    );
  });

  onCleanup(() => {
    document.removeEventListener("keydown", handleGlobalKeyDown);
    window.removeEventListener("usage-data-changed", handleUsageChanged);
    unlistenWatcher?.();
    unlistenMaintenance?.();
    unlistenResized?.();
    sync.stopPolling();
    clearTimeout(debounceTimer);
    debouncedChangedPaths.clear();
  });

  const filteredTree = createMemo(() =>
    tree().filter((node) => !disabledProviders().includes(node.id as Provider)),
  );
  const showExplorer = createMemo(() => {
    const v = activeView();
    return v !== "settings" && v !== "trash" && v !== "usage";
  });
  const showExplorerTree = createMemo(() => {
    if (sidebarCollapsed()) return false;
    const v = activeView();
    return (
      v !== "settings" &&
      v !== "trash" &&
      v !== "favorites" &&
      v !== "blocked" &&
      v !== "usage"
    );
  });

  return (
    <ErrorBoundary
      fallback={(err) => (
        <div
          style={{
            display: "flex",
            "flex-direction": "column",
            "align-items": "center",
            "justify-content": "center",
            height: "100vh",
            gap: "16px",
            padding: "24px",
            "text-align": "center",
            "font-family": "var(--font-family)",
            color: "var(--text-primary)",
            background: "var(--bg-primary)",
          }}
        >
          <h2>{t("error.title")}</h2>
          <p style={{ color: "var(--text-secondary)", "max-width": "500px" }}>
            {err?.message || t("error.message")}
          </p>
          <button
            onClick={() => window.location.reload()}
            style={{
              padding: "8px 16px",
              "border-radius": "6px",
              border: "1px solid var(--border-color)",
              background: "var(--bg-secondary)",
              color: "var(--text-primary)",
              cursor: "pointer",
            }}
          >
            {t("error.reload")}
          </button>
        </div>
      )}
    >
      <div class="app-layout">
        <TitleBar
          showWindowControls={showWindowControls}
          isMaximized={isMaximized()}
          onMinimize={() => void getCurrentWindow().minimize()}
          onToggleMaximize={() => void getCurrentWindow().toggleMaximize()}
          onClose={() => void getCurrentWindow().close()}
          onStartDragging={() => void getCurrentWindow().startDragging()}
        />
        <div class="main-layout">
          <ActivityBar
            activeView={activeView()}
            onViewChange={(v) => {
              setActiveView(v);
              if (v === "explorer") setSidebarCollapsed(false);
            }}
          />
          <Show when={showExplorerTree()}>
            <Explorer
              tree={filteredTree()}
              isLoading={isLoading()}
              activeSessionId={activeGroup()?.activeTabId ?? null}
              onOpenSession={openSession}
              onPreviewSession={openPreview}
              onRefreshTree={sync.refreshTree}
              onRefreshProvider={(provider) => {
                void sync
                  .syncProviders([provider])
                  .then(() => void loadProviderSnapshots(true));
              }}
              onCollapse={() => setSidebarCollapsed(true)}
              onDeleteSession={async (id: string) => {
                try {
                  await trashSession(id);
                  closeTab(id);
                  await sync.refreshTree();
                } catch (e) {
                  toastError(String(e));
                }
              }}
            />
          </Show>
          <Show when={activeView() === "settings"}>
            <SettingsPanel />
          </Show>
          <Show when={activeView() === "trash"}>
            <TrashView onRefreshTree={sync.refreshTree} />
          </Show>
          <Show when={activeView() === "favorites"}>
            <FavoritesView onOpenSession={openSession} />
          </Show>
          <Show when={activeView() === "blocked"}>
            <BlockedView onRefreshTree={sync.refreshTree} />
          </Show>
          <Show when={activeView() === "usage"}>
            <div
              style={{
                display: "flex",
                flex: "1",
                "min-width": "0",
              }}
            >
              <UsagePanel />
            </div>
          </Show>
          <Show when={showExplorer()}>
            <EditorGroupsContainer
              onTabSelect={(groupId, tabId) => {
                focusGroup(groupId);
                setActiveTabInGroup(groupId, tabId);
              }}
              onTabClose={closeTab}
              onCloseAllTabs={closeAllTabs}
              onCloseOtherTabs={closeOtherTabs}
              onCloseTabsToRight={closeTabsToRight}
              onSplitToRight={splitToRight}
              onPinTab={pinTab}
              onRefreshTree={sync.refreshTree}
              tree={filteredTree()}
              onOpenSession={openSession}
            />
          </Show>
        </div>
        <StatusBar
          sessionCount={sessionCount()}
          providerCount={filteredTree().length}
          isIndexing={isLoading()}
          lastScanTime={lastScanTime()}
          todayCost={todayCost()}
          todayTokens={todayTokens()}
        />
        <KeyboardOverlay
          show={showKeyboardOverlay()}
          onClose={() => setShowKeyboardOverlay(false)}
        />
        <SearchOverlay
          show={showSearchOverlay()}
          onClose={() => setShowSearchOverlay(false)}
          onOpenSession={(s) => {
            openSession(s);
            setShowSearchOverlay(false);
          }}
        />
        <ToastContainer />
      </div>
    </ErrorBoundary>
  );
}
