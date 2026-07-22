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
const UsagePanel = lazy(() => import("@/features/usage").then((m) => ({ default: m.UsagePanel })));
const FolderAnalyticsPanel = lazy(() =>
  import("@/features/usage/FolderAnalyticsPanel").then((m) => ({ default: m.FolderAnalyticsPanel })),
);
import { KeyboardOverlay } from "@/app/KeyboardOverlay";
import { Button } from "@/components/ui/button";
import { Toaster } from "@/components/ui/sonner";
import {
  getChildSessions,
  startRebuildIndex,
  getIndexStats,
  getTodayCost,
  getTodayTokens,
  invokeWithFallback,
} from "@/lib/tauri";
import { isMac, isWindows } from "@/lib/platform";
import { isTauriRuntime } from "@/lib/runtime";
import { useIsCompact } from "@/stores/viewport";
import { useAutoIndexInterval, useDisabledProviders } from "@/stores/settings";
import { loadProviderSnapshots } from "@/stores/providerSnapshots";
import { toast, toastError, toastInfo } from "@/stores/toast";
import { checkForUpdate } from "@/features/updater/updater";
import { UpdateDialog } from "@/features/updater/UpdateDialog";
import { autoIndexIntervalMs } from "@/lib/auto-index";
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
  reopenClosedTab,
  splitToRight,
  setActiveTabInGroup,
  focusGroup,
  focusAdjacentGroup,
  syncAllTabTitles,
} from "@/features/editor/editorGroups";
import type { TreeNode, Provider, SessionRef } from "@/lib/types";
import { useI18n } from "@/i18n";
import { createKeyboardHandler } from "@/app/KeyboardShortcuts";
import { createSyncManager } from "@/app/SyncManager";
import { createOpenSubagentHandler } from "@/app/SubagentOpen";
import { TitleBar } from "@/app/TitleBar";
import "@/styles/index.css";

// Linux derived locally (platform.ts is intentionally minimal). On Linux the
// app hides native decorations like Windows, so it needs the same custom
// min/max/close window controls.
const isLinux = typeof navigator !== "undefined" && /Linux/.test(navigator.platform);
// Window chrome only exists in the Tauri shell; a plain browser tab has its own.
const showWindowControls = (isWindows || isLinux) && isTauriRuntime;

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
  const [lastScanTime, setLastScanTime] = useState<number | undefined>(undefined);
  const [nextAutoIndexTime, setNextAutoIndexTime] = useState<number | undefined>(undefined);
  const [initialIndexReady, setInitialIndexReady] = useState(false);
  const [isMaintenanceRunning, setIsMaintenanceRunning] = useState(false);
  const [todayCost, setTodayCost] = useState<number | undefined>(undefined);
  const [todayTokens, setTodayTokens] = useState<
    { input: number; output: number; cache_read: number; cache_write: number } | undefined
  >(undefined);

  const disabledProviders = useDisabledProviders();
  const autoIndexInterval = useAutoIndexInterval();
  const activeGrp = useActiveGroup();
  const isCompact = useIsCompact();
  // Compact layout is a single-pane stack: within the explorer view the user
  // is either on the session list ("nav") or reading a session ("content").
  // Opening a session flips to content; the bottom-nav explorer button flips
  // back. Desktop ignores this entirely.
  const [compactPane, setCompactPane] = useState<"nav" | "content">("nav");

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
  const tRef = useRef(t);
  const autoIndexStartPendingRef = useRef(false);
  const autoIndexActiveRef = useRef(false);
  useEffect(() => {
    tRef.current = t;
  });

  useEffect(() => {
    const intervalMs = autoIndexIntervalMs(autoIndexInterval);
    if (!initialIndexReady || intervalMs === null) {
      setNextAutoIndexTime(undefined);
      return;
    }

    let disposed = false;
    let timer: number | undefined;

    const schedule = (baseTime: number) => {
      const now = Date.now();
      const next = Math.max(baseTime + intervalMs, now);
      setNextAutoIndexTime(next);
      timer = window.setTimeout(
        () => {
          void run();
        },
        Math.max(0, next - Date.now()),
      );
    };

    const run = async () => {
      if (disposed) return;
      setNextAutoIndexTime(undefined);
      autoIndexStartPendingRef.current = true;
      try {
        const started = await startRebuildIndex();
        if (!started) {
          autoIndexStartPendingRef.current = false;
          console.info("Scheduled index rebuild skipped because another maintenance task is running");
        }
      } catch (error) {
        autoIndexStartPendingRef.current = false;
        console.error("Failed to start scheduled index rebuild:", error);
      }
      if (!disposed) {
        schedule(Date.now());
      }
    };

    const now = Date.now();
    const recentLastScanTime = lastScanTime !== undefined && now - lastScanTime < intervalMs ? lastScanTime : now;
    schedule(recentLastScanTime);

    return () => {
      disposed = true;
      if (timer !== undefined) {
        window.clearTimeout(timer);
      }
    };
  }, [autoIndexInterval, initialIndexReady, lastScanTime]);

  useEffect(() => {
    const sync = syncRef.current;
    if (!sync) return;

    let disposed = false;
    let unlistenMaintenance: UnlistenFn | undefined;
    let unlistenResized: UnlistenFn | undefined;

    async function refreshStatusBarStats() {
      const [stats, cost, tokens] = await Promise.all([
        invokeWithFallback(getIndexStats(), undefined, "refresh status bar index stats"),
        invokeWithFallback(getTodayCost(), undefined, "refresh today cost"),
        invokeWithFallback(getTodayTokens(), undefined, "refresh today tokens"),
      ]);

      const ts = stats?.last_index_time ? Number(stats.last_index_time) : undefined;
      setLastScanTime(ts);
      setTodayCost(cost);
      setTodayTokens(tokens);
    }

    const handleUsageChanged = () => void refreshStatusBarStats();

    const handleGlobalKeyDown = createKeyboardHandler({
      activeTabId: () => activeGroup()?.activeTabId ?? null,
      openTabs: () => activeGroup()?.tabs ?? [],
      setActiveTabId: (id: string | null) => {
        const g = activeGroup();
        if (g && id) setActiveTabInGroup(g.id, id);
      },
      setShowKeyboardOverlay,
      setShowSearchOverlay,
      setActiveView,
      closeTab,
      closeAllTabs,
      reopenClosedTab,
      toggleSidebar: () => setSidebarCollapsed((prev) => !prev),
      splitToRight,
      focusAdjacentGroup,
      startRebuildIndex: () => {
        void startRebuildIndex()
          .then((started) => {
            if (!started) toastInfo(tRef.current("toast.maintenanceBusy"));
          })
          .catch(() => toastError(tRef.current("toast.rebuildFailed")));
      },
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
        console.error(`Failed to load child sessions for parent ${parentId}:`, error);
      },
    });

    if (isMac) {
      document.documentElement.style.setProperty("--titlebar-inset", "78px");
    }

    window.addEventListener("usage-data-changed", handleUsageChanged);
    window.addEventListener("open-subagent", handleOpenSubagent);
    document.addEventListener("keydown", handleGlobalKeyDown);

    void loadProviderSnapshots();
    void sync.coldStart().finally(() => {
      if (!disposed) {
        setInitialIndexReady(true);
        void refreshStatusBarStats();
      }
    });
    // Warm the markdown engine while the shell is idle,
    // so the first session open doesn't pay the chunk-load + highlighter
    // initialization on the critical path.
    // WKWebView (Safari engine) has never shipped requestIdleCallback,
    // despite lib.dom typing it — feature-detect and fall back to a timer.
    // Both delays just keep startup work off the first-paint critical path.
    const WARMUP_FALLBACK_MS = 1500;
    const UPDATE_CHECK_DELAY_MS = 2000;
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
            const handle = window.setTimeout(warmMarkdown, WARMUP_FALLBACK_MS);
            return () => window.clearTimeout(handle);
          })();
    // Headless shell: updates ship via npm/the server binary, not the in-app updater.
    const updateTimer = isTauriRuntime ? setTimeout(() => void checkForUpdate(), UPDATE_CHECK_DELAY_MS) : null;

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
          console.error("Failed to initialize window maximize tracking:", error);
        }
      }

      const um = await listenBackendEvent("maintenance-status", (payload) => {
        const autoRebuildEvent =
          payload.job === "rebuild_index" && (autoIndexStartPendingRef.current || autoIndexActiveRef.current);

        if (payload.phase === "started") {
          setIsMaintenanceRunning(true);
          if (autoRebuildEvent) {
            autoIndexStartPendingRef.current = false;
            autoIndexActiveRef.current = true;
            return;
          }
          const message =
            payload.job === "refresh_usage"
              ? tRef.current("toast.refreshUsageStarted")
              : tRef.current("toast.rebuildStarted");
          toastInfo(message);
          return;
        }

        if (payload.phase === "failed") {
          setIsMaintenanceRunning(false);
          if (autoIndexActiveRef.current && payload.job === "rebuild_index") {
            autoIndexActiveRef.current = false;
            console.error(payload.message || "Scheduled index rebuild failed");
            return;
          }
          toastError(payload.message || tRef.current("toast.rebuildFailed"));
          return;
        }

        if (payload.phase === "finished") {
          setIsMaintenanceRunning(false);
          // sync non-null (guarded at effect top); assertion needed inside this
          // event-listener callback where TS drops the guard's narrowing.
          void sync!.refreshTree();
          void loadProviderSnapshots(true);
          if (autoIndexActiveRef.current && payload.job === "rebuild_index") {
            autoIndexActiveRef.current = false;
            return;
          }
          const message =
            payload.job === "refresh_usage" ? tRef.current("toast.refreshUsageOk") : tRef.current("toast.rebuildOk");
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
      unlistenMaintenance?.();
      unlistenResized?.();
      cancelWarmup();
      if (updateTimer !== null) clearTimeout(updateTimer);
    };
  }, []);

  const filteredTree = tree.filter((node) => !disabledProviders.includes(node.id as Provider));
  const showExplorer = activeView !== "settings" && activeView !== "usage" && activeView !== "folderAnalytics";
  const showExplorerTree =
    !sidebarCollapsed &&
    activeView !== "settings" &&
    activeView !== "favorites" &&
    activeView !== "blocked" &&
    activeView !== "usage" &&
    activeView !== "folderAnalytics";

  // Compact single-pane stack: the session list and the session content never
  // share the screen. With no open tabs there is nothing to read, so the list
  // wins regardless of the pane flag.
  const compactOnList = compactPane === "nav" || (activeGrp?.tabs.length ?? 0) === 0;
  const showTree = isCompact ? activeView === "explorer" && compactOnList : showExplorerTree;
  const showEditor = isCompact ? activeView === "explorer" && !compactOnList : showExplorer;

  // In compact mode the session content only renders inside the explorer
  // view, so opening from favorites/search must also switch there.
  const showOpenedSession = () => {
    if (!isCompact) return;
    setActiveView("explorer");
    setCompactPane("content");
  };
  const handleOpenSession = (s: SessionRef) => {
    openSession(s);
    showOpenedSession();
  };
  const handlePreviewSession = (s: SessionRef) => {
    openPreview(s);
    showOpenedSession();
  };

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
          <p style={{ color: "var(--text-secondary)", maxWidth: "500px" }}>{err?.message || t("error.message")}</p>
          <Button
            variant="outline"
            className="active:translate-y-0"
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
          </Button>
        </div>
      )}
    >
      <div className="app-layout">
        {isTauriRuntime && (
          <TitleBar
            showWindowControls={showWindowControls}
            isMaximized={isMaximized}
            onMinimize={() => {
              if (isTauriRuntime) void getCurrentWindow().minimize();
            }}
            onToggleMaximize={() => {
              if (isTauriRuntime) void getCurrentWindow().toggleMaximize();
            }}
            onClose={() => {
              if (isTauriRuntime) void getCurrentWindow().close();
            }}
            onStartDragging={() => {
              if (isTauriRuntime) void getCurrentWindow().startDragging();
            }}
          />
        )}
        <div className="main-layout">
          {!isCompact && (
            <ActivityBar
              activeView={activeView}
              onViewChange={(v) => {
                setActiveView(v);
                if (v === "explorer") setSidebarCollapsed(false);
              }}
            />
          )}
          {showTree && (
            <Explorer
              tree={filteredTree}
              isLoading={isLoading}
              activeSessionId={activeGrp?.activeTabId ?? null}
              onOpenSession={handleOpenSession}
              onPreviewSession={handlePreviewSession}
              onRefreshTree={sync.refreshTree}
              onRefreshProvider={(provider) => {
                void sync.syncProviders([provider]).then(() => void loadProviderSnapshots(true));
              }}
              onCollapse={isCompact ? undefined : () => setSidebarCollapsed(true)}
            />
          )}
          <Suspense fallback={null}>
            {activeView === "settings" && <SettingsPanel />}
            {activeView === "favorites" && <FavoritesView onOpenSession={handleOpenSession} />}
            {activeView === "blocked" && <BlockedView onRefreshTree={sync.refreshTree} />}
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
            {activeView === "folderAnalytics" && (
              <div
                style={{
                  display: "flex",
                  flex: "1",
                  minWidth: "0",
                }}
              >
                <FolderAnalyticsPanel />
              </div>
            )}
          </Suspense>
          {showEditor && (
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
              tree={filteredTree}
              onOpenSession={handleOpenSession}
            />
          )}
        </div>
        {isCompact && (
          <ActivityBar
            orientation="horizontal"
            activeView={activeView}
            onViewChange={(v) => {
              // Tapping the explorer item always returns to the session list;
              // the open tabs stay put for when a session is tapped again.
              if (v === "explorer") setCompactPane("nav");
              setActiveView(v);
            }}
          />
        )}
        <StatusBar
          sessionCount={sessionCount}
          providerCount={filteredTree.length}
          isIndexing={isLoading || isMaintenanceRunning}
          lastScanTime={lastScanTime}
          nextAutoIndexTime={nextAutoIndexTime}
          todayCost={todayCost}
          todayTokens={todayTokens}
        />
        <KeyboardOverlay show={showKeyboardOverlay} onClose={() => setShowKeyboardOverlay(false)} />
        <UpdateDialog />
        <SearchOverlay
          show={showSearchOverlay}
          onClose={() => setShowSearchOverlay(false)}
          onOpenSession={(s) => {
            handleOpenSession(s);
            setShowSearchOverlay(false);
          }}
        />
        <Toaster position="bottom-right" />
      </div>
    </ErrorBoundary>
  );
}
