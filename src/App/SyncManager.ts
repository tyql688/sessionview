import type { TreeNode } from "@/lib/types";
import {
  reindex,
  syncSources,
  getTree,
  getSessionCount,
  reindexProviders,
  getPricingCatalogStatus,
  refreshPricingCatalog,
  clearUsageStats,
} from "@/lib/tauri";
import {
  getPollWatchProviders,
  loadProviderWatchSnapshots,
} from "@/lib/provider-watch";
import { i18next } from "@/i18n/index";
import { toastError, toastInfo } from "@/stores/toast";

export interface SyncCallbacks {
  setTree: (tree: TreeNode[]) => void;
  setSessionCount: (count: number) => void;
  setIsLoading: (loading: boolean) => void;
  syncTabsWithTree: (treeData: TreeNode[]) => void;
}

export function createSyncManager(callbacks: SyncCallbacks) {
  let syncInFlight = false;
  let pendingFullSync = false;
  const pendingChangedPaths = new Set<string>();
  let pollTimer: ReturnType<typeof setInterval> | undefined;
  let pollConfigToken = 0;

  async function refreshTree() {
    const [treeData, count] = await Promise.all([getTree(), getSessionCount()]);
    callbacks.setTree(treeData);
    callbacks.setSessionCount(count);
    // Sync open tab titles with latest tree data
    callbacks.syncTabsWithTree(treeData);
    window.dispatchEvent(new CustomEvent("usage-data-changed"));
  }

  async function syncFromDisk(options?: {
    changedPaths?: string[];
    showSpinner?: boolean;
  }) {
    const changedPaths =
      options?.changedPaths?.filter((path) => path.length > 0) ?? [];
    const showSpinner = options?.showSpinner ?? false;

    if (syncInFlight) {
      if (changedPaths.length > 0 && !pendingFullSync) {
        for (const path of changedPaths) pendingChangedPaths.add(path);
      } else {
        pendingFullSync = true;
      }
      return;
    }

    syncInFlight = true;
    if (showSpinner) {
      callbacks.setIsLoading(true);
    }

    try {
      if (changedPaths.length > 0) {
        await syncSources(changedPaths);
      } else {
        await reindex();
      }
      await refreshTree();
    } catch (e) {
      toastError(String(e));
    } finally {
      syncInFlight = false;
      if (showSpinner) {
        callbacks.setIsLoading(false);
      }
      if (pendingFullSync) {
        pendingFullSync = false;
        pendingChangedPaths.clear();
        void syncFromDisk({ showSpinner });
      } else if (pendingChangedPaths.size > 0) {
        const queuedPaths = [...pendingChangedPaths];
        pendingChangedPaths.clear();
        void syncFromDisk({ changedPaths: queuedPaths });
      }
    }
  }

  async function syncProviders(providers: string[], showSpinner = true) {
    if (providers.length === 0) return;

    if (syncInFlight) {
      pendingFullSync = true;
      return;
    }

    syncInFlight = true;
    if (showSpinner) callbacks.setIsLoading(true);

    try {
      await reindexProviders(providers, true);
      await refreshTree();
    } catch (e) {
      toastError(String(e));
    } finally {
      syncInFlight = false;
      if (showSpinner) callbacks.setIsLoading(false);
      if (pendingFullSync) {
        pendingFullSync = false;
        pendingChangedPaths.clear();
        void syncFromDisk({ showSpinner });
      } else if (pendingChangedPaths.size > 0) {
        const queuedPaths = [...pendingChangedPaths];
        pendingChangedPaths.clear();
        void syncFromDisk({ changedPaths: queuedPaths });
      }
    }
  }

  /** Poll sync — serialized with FS-event sync via syncInFlight guard. */
  async function pollSync(providers: string[]) {
    if (syncInFlight) return;

    syncInFlight = true;
    try {
      const indexedCount = await reindexProviders(providers);
      if (indexedCount > 0) {
        await refreshTree();
      }
    } catch (e) {
      // Polling failures are transient — log for diagnosis, don't toast
      console.debug("poll sync failed:", e);
    } finally {
      syncInFlight = false;
      // Drain pending work (FS events queued during poll take priority)
      if (pendingFullSync) {
        pendingFullSync = false;
        pendingChangedPaths.clear();
        void syncFromDisk();
      } else if (pendingChangedPaths.size > 0) {
        const queuedPaths = [...pendingChangedPaths];
        pendingChangedPaths.clear();
        void syncFromDisk({ changedPaths: queuedPaths });
      }
    }
  }

  function applyPolling(providers: string[]) {
    clearInterval(pollTimer);
    pollTimer = undefined;

    if (providers.length === 0) return;

    pollTimer = setInterval(() => {
      void pollSync(providers);
    }, 5000);
  }

  function providersKey(providers: string[]): string {
    return [...providers].sort().join("|");
  }

  function startPolling() {
    const token = ++pollConfigToken;
    let activeProviders = getPollWatchProviders();
    applyPolling(activeProviders);

    const catalogLoad = loadProviderWatchSnapshots();
    void catalogLoad?.then(() => {
      if (token !== pollConfigToken) return;

      const nextProviders = getPollWatchProviders();
      if (providersKey(nextProviders) === providersKey(activeProviders)) return;

      activeProviders = nextProviders;
      applyPolling(activeProviders);
    });
  }

  function stopPolling() {
    pollConfigToken += 1;
    clearInterval(pollTimer);
    pollTimer = undefined;
  }

  /**
   * First use: the pricing catalog has never been fetched, so the index pass
   * would cost every session at $0. Fetch the catalog up front, then clear any
   * stats a previous catalog-less run left behind so the reindex that follows
   * re-parses everything with real prices. Once the fetch succeeds the catalog
   * timestamp is set and this never runs again; on failure (e.g. offline) it
   * retries on the next launch.
   */
  async function bootstrapPricingIfNeeded() {
    const t = (key: string): string => i18next.t(key);
    const pricing = await getPricingCatalogStatus().catch((error: unknown) => {
      toastError(String(error));
      return null;
    });
    if (!pricing || pricing.updated_at) return;

    toastInfo(t("usage.firstUseBootstrap"));
    // The catalog fetch is a single short HTTP request but flaky networks are
    // common; retry a couple of times before deferring to the next launch.
    const attempts = 3;
    for (let attempt = 1; attempt <= attempts; attempt++) {
      try {
        await refreshPricingCatalog();
        await clearUsageStats();
        return;
      } catch (error) {
        if (attempt === attempts) {
          toastError(`${t("usage.firstUseBootstrapFailed")}: ${String(error)}`);
          return;
        }
        await new Promise((resolve) => setTimeout(resolve, 2000 * attempt));
      }
    }
  }

  /** Load cached tree immediately, then reindex in background. */
  async function coldStart() {
    // Show cached data instantly so the user doesn't stare at a spinner
    let cacheHit = false;
    try {
      await refreshTree();
      cacheHit = true;
    } catch (error) {
      console.warn("Cold start tree refresh failed before reindex:", error);
    }
    // Only dismiss spinner early on cache hit; keep it up on cache miss
    if (cacheHit) callbacks.setIsLoading(false);

    await bootstrapPricingIfNeeded();

    // Reindex in background
    try {
      await reindex();
      await refreshTree();
    } catch (e) {
      toastError(String(e));
    } finally {
      if (!cacheHit) callbacks.setIsLoading(false);
    }

    startPolling();
  }

  return {
    syncFromDisk,
    syncProviders,
    refreshTree,
    coldStart,
    stopPolling,
  };
}
