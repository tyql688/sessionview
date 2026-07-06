import { Component, lazy, Suspense, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { listenBackendEvent, type UnlistenFn } from "@/lib/backend-events";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ActivityBar } from "@/app/ActivityBar";
import { Explorer } from "@/features/explorer";
import { EditorGroupsContainer } from "@/features/editor/EditorGroupsContainer";
import { StatusBar } from "@/app/StatusBar";
import { SearchOverlay } from "@/features/search/SearchOverlay";

// Side panels load on first open — none of them belong in the startup chunk
// (the usage panel alone pulls the chart/heatmap stack).
const SettingsPanel = lazy(() =>
  import("@/features/settings/SettingsPanel").then((m) => ({
    default: m.SettingsPanel,
  })),
);
const TrashView = lazy(() =>
  import("@/features/trash").then((m) => ({ default: m.TrashView })),
);
const FavoritesView = lazy(() =>
  import("@/features/favorites/FavoritesView").then((m) => ({
    default: m.FavoritesView,
  })),
);
const BlockedView = lazy(() =>
  import("@/features/settings/BlockedView").then((m) => ({
    default: m.BlockedView,
  })),
);
const UsagePanel = lazy(() =>
  import("@/features/usage").then((m) => ({ default: m.UsagePanel })),
);
import { KeyboardOverlay } from "@/app/KeyboardOverlay";
import { Toaster } from "@/components/ui/sonner";
import {
  trashSession,
  getChildSessions,
  startRebuildIndex,
  getIndexStats,
  getTodayCost,
  getTodayTokens,
  invokeWithFallback,
} from "@/lib/tauri";
import { isMac, isWindows } from "@/lib/platform";
import { useDisabledProviders } from "@/stores/settings";
import { loadProviderSnapshots } from "@/stores/providerSnapshots";
import { toast, toastError, toastInfo } from "@/stores/toast";
import { checkForUpdate } from "@/features/updater/updater";
import {
  getGroups,
  activeGroup,
  useActiveGroup,
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
} from "@/features/editor/editorGroups";
import type { TreeNode, Provider } from "@/lib/types";
import { useI18n } from "@/i18n";
import { createKeyboardHandler } from "@/app/KeyboardShortcuts";
import { createSyncManager } from "@/app/SyncManager";
import { createOpenSubagentHandler } from "@/app/SubagentOpen";
import { TitleBar } from "@/app/TitleBar";
import "@/styles/index.css";

// Linux derived locally (platform.ts is intentionally minimal). On Linux the
// app hides native decorations like Windows, so it needs the same custom
// min/max/close window controls.
const isLinux =
  typeof navigator !== "undefined" && /Linux/.test(navigator.platform);
const showWindowControls = isWindows || isLinux;

interface ErrorBoundaryProps {
  fallback: (error: Error) => ReactNode;
  children: ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
}

class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: unknown): ErrorBoundaryState {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  componentDidCatch(error: unknown) {
    console.error("App render error:", error);
  }

  render(): ReactNode {
    if (this.state.error) {
      return this.props.fallback(this.state.error);
    }
    return this.props.children;
  }
}

export default function App() {
  const { t } = useI18n();
  const [tree, setTree] = useState<TreeNode[]>([]);
  const [sessionCount, setSessionCount] = useState(0);
  const [activeView, setActiveView] = useState("explorer");
  const [isLoading, setIsLoading] = useState(true);
  const [showKeyboardOverlay, setShowKeyboardOverlay] = useState(false);
  const [showSearchOverlay, setShowSearchOverlay] = useState(false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [isMaximized, setIsMaximized] = useState(false);
  const [lastScanTime, setLastScanTime] = useState<number | undefined>(
    undefined,
  );
  const [todayCost, setTodayCost] = useState<number | undefined>(undefined);
  const [todayTokens, setTodayTokens] = useState<
    | { input: number; output: number; cache_read: number; cache_write: number }
    | undefined
  >(undefined);

  const disabledProviders = useDisabledProviders();
  const activeGrp = useActiveGroup();

  // The sync manager owns long-lived timers/queues; create it once so its
  // internal state survives re-renders.
  const syncRef = useRef<ReturnType<typeof createSyncManager> | null>(null);
  if (syncRef.current === null) {
    syncRef.current = createSyncManager({
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
  }
  const sync = syncRef.current;

  // Latest-value refs so the once-created keydown/event handlers read fresh
  // state without re-subscribing on every change.
  const showKeyboardOverlayRef = useRef(showKeyboardOverlay);
  const tRef = useRef(t);
  useEffect(() => {
    showKeyboardOverlayRef.current = showKeyboardOverlay;
    tRef.current = t;
  });

  useEffect(() => {
    const sync = syncRef.current;
    if (!sync) return;

    let disposed = false;
    const debouncedChangedPaths = new Set<string>();
    let unlistenWatcher: UnlistenFn | undefined;
    let unlistenMaintenance: UnlistenFn | undefined;
    let unlistenResized: UnlistenFn | undefined;
    let debounceTimer: ReturnType<typeof setTimeout> | undefined;

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

    const handleGlobalKeyDown = createKeyboardHandler({
      activeTabId: () => activeGroup()?.activeTabId ?? null,
      openTabs: () => activeGroup()?.tabs ?? [],
      showKeyboardOverlay: () => showKeyboardOverlayRef.current,
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

    const handleOpenSubagent = createOpenSubagentHandler({
      getActiveParentSessionIds: () =>
        getGroups()
          .map((g) => g.activeTabId)
          .filter((id): id is string => id != null),
      getChildSessions,
      openSession,
      onLoadFailed: () => toastError(tRef.current("toast.subagentLoadFailed")),
      onNotFound: () => toastError(tRef.current("toast.subagentNotFound")),
      onChildSessionLoadError: (parentId, error) => {
        console.error(
          `Failed to load child sessions for parent ${parentId}:`,
          error,
        );
      },
    });

    if (isMac) {
      document.documentElement.style.setProperty("--titlebar-inset", "78px");
    }

    window.addEventListener("usage-data-changed", handleUsageChanged);
    window.addEventListener("open-subagent", handleOpenSubagent);
    document.addEventListener("keydown", handleGlobalKeyDown);

    void loadProviderSnapshots();
    void sync.coldStart();
    // Warm the markdown engine (streamdown + shiki) while the shell is idle,
    // so the first session open doesn't pay the chunk-load + highlighter
    // initialization on the critical path.
    // WKWebView (Safari engine) has never shipped requestIdleCallback,
    // despite lib.dom typing it — feature-detect and fall back to a timer.
    const warmMarkdown = () => {
      void import("@/features/session/timeline/Markdown");
    };
    const cancelWarmup =
      typeof window.requestIdleCallback === "function"
        ? (() => {
            const handle = window.requestIdleCallback(warmMarkdown, {
              timeout: 3000,
            });
            return () => window.cancelIdleCallback(handle);
          })()
        : (() => {
            const handle = window.setTimeout(warmMarkdown, 1500);
            return () => window.clearTimeout(handle);
          })();
    const updateTimer = setTimeout(() => void checkForUpdate(), 2000);

    async function setup() {
      // Track maximize state so the custom (Windows/Linux) maximize button can
      // swap to a "restore" glyph. macOS uses native traffic lights, so skip it.
      if (showWindowControls) {
        const win = getCurrentWindow();
        try {
          setIsMaximized(await win.isMaximized());
          const un = await win.onResized(async () => {
            try {
              setIsMaximized(await win.isMaximized());
            } catch (error) {
              console.error("Failed to read window maximize state:", error);
            }
          });
          if (disposed) un();
          else unlistenResized = un;
        } catch (error) {
          console.error(
            "Failed to initialize window maximize tracking:",
            error,
          );
        }
      }

      const uw = await listenBackendEvent("sessions-changed", (payload) => {
        for (const path of payload ?? []) {
          if (path.length > 0) {
            debouncedChangedPaths.add(path);
          }
        }
        clearTimeout(debounceTimer);
        debounceTimer = setTimeout(() => {
          const changedPaths = [...debouncedChangedPaths];
          debouncedChangedPaths.clear();
          // sync is non-null (guarded at effect top); assertion needed because
          // TS drops the guard's narrowing inside this nested setTimeout callback.
          void sync!.syncFromDisk({ changedPaths });
        }, 500);
      });
      if (disposed) uw();
      else unlistenWatcher = uw;

      const um = await listenBackendEvent("maintenance-status", (payload) => {
        if (payload.phase === "started") {
          const message =
            payload.job === "refresh_usage"
              ? tRef.current("toast.refreshUsageStarted")
              : tRef.current("toast.rebuildStarted");
          toastInfo(message);
          return;
        }

        if (payload.phase === "failed") {
          toastError(payload.message || tRef.current("toast.rebuildFailed"));
          return;
        }

        if (payload.phase === "finished") {
          // sync non-null (guarded at effect top); assertion needed inside this
          // event-listener callback where TS drops the guard's narrowing.
          void sync!.refreshTree();
          void loadProviderSnapshots(true);
          const message =
            payload.job === "refresh_usage"
              ? tRef.current("toast.refreshUsageOk")
              : tRef.current("toast.rebuildOk");
          toast(message);
        }
      });
      if (disposed) um();
      else unlistenMaintenance = um;
    }
    void setup();

    return () => {
      disposed = true;
      document.removeEventListener("keydown", handleGlobalKeyDown);
      window.removeEventListener("usage-data-changed", handleUsageChanged);
      window.removeEventListener("open-subagent", handleOpenSubagent);
      unlistenWatcher?.();
      unlistenMaintenance?.();
      unlistenResized?.();
      sync.stopPolling();
      cancelWarmup();
      clearTimeout(updateTimer);
      clearTimeout(debounceTimer);
      debouncedChangedPaths.clear();
    };
  }, []);

  const filteredTree = tree.filter(
    (node) => !disabledProviders.includes(node.id as Provider),
  );
  const showExplorer =
    activeView !== "settings" &&
    activeView !== "trash" &&
    activeView !== "usage";
  const showExplorerTree =
    !sidebarCollapsed &&
    activeView !== "settings" &&
    activeView !== "trash" &&
    activeView !== "favorites" &&
    activeView !== "blocked" &&
    activeView !== "usage";

  return (
    <ErrorBoundary
      fallback={(err) => (
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            height: "100vh",
            gap: "16px",
            padding: "24px",
            textAlign: "center",
            fontFamily: "var(--font-family)",
            color: "var(--text-primary)",
            background: "var(--bg-primary)",
          }}
        >
          <h2>{t("error.title")}</h2>
          <p style={{ color: "var(--text-secondary)", maxWidth: "500px" }}>
            {err?.message || t("error.message")}
          </p>
          <button
            onClick={() => window.location.reload()}
            style={{
              padding: "8px 16px",
              borderRadius: "6px",
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
      <div className="app-layout">
        <TitleBar
          showWindowControls={showWindowControls}
          isMaximized={isMaximized}
          onMinimize={() => void getCurrentWindow().minimize()}
          onToggleMaximize={() => void getCurrentWindow().toggleMaximize()}
          onClose={() => void getCurrentWindow().close()}
          onStartDragging={() => void getCurrentWindow().startDragging()}
        />
        <div className="main-layout">
          <ActivityBar
            activeView={activeView}
            onViewChange={(v) => {
              setActiveView(v);
              if (v === "explorer") setSidebarCollapsed(false);
            }}
          />
          {showExplorerTree && (
            <Explorer
              tree={filteredTree}
              isLoading={isLoading}
              activeSessionId={activeGrp?.activeTabId ?? null}
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
          )}
          <Suspense fallback={null}>
            {activeView === "settings" && <SettingsPanel />}
            {activeView === "trash" && (
              <TrashView onRefreshTree={sync.refreshTree} />
            )}
            {activeView === "favorites" && (
              <FavoritesView onOpenSession={openSession} />
            )}
            {activeView === "blocked" && (
              <BlockedView onRefreshTree={sync.refreshTree} />
            )}
            {activeView === "usage" && (
              <div
                style={{
                  display: "flex",
                  flex: "1",
                  minWidth: "0",
                }}
              >
                <UsagePanel />
              </div>
            )}
          </Suspense>
          {showExplorer && (
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
              tree={filteredTree}
              onOpenSession={openSession}
            />
          )}
        </div>
        <StatusBar
          sessionCount={sessionCount}
          providerCount={filteredTree.length}
          isIndexing={isLoading}
          lastScanTime={lastScanTime}
          todayCost={todayCost}
          todayTokens={todayTokens}
        />
        <KeyboardOverlay
          show={showKeyboardOverlay}
          onClose={() => setShowKeyboardOverlay(false)}
        />
        <SearchOverlay
          show={showSearchOverlay}
          onClose={() => setShowSearchOverlay(false)}
          onOpenSession={(s) => {
            openSession(s);
            setShowSearchOverlay(false);
          }}
        />
        <Toaster position="bottom-right" />
      </div>
    </ErrorBoundary>
  );
}
