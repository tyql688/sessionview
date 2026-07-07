import type { TreeNode } from "@/lib/types";
import {
  reindex,
  getTree,
  getSessionCount,
  reindexProviders,
  getPricingCatalogStatus,
  refreshPricingCatalog,
  clearUsageStats,
} from "@/lib/tauri";
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

  async function refreshTree() {
    const [treeData, count] = await Promise.all([getTree(), getSessionCount()]);
    callbacks.setTree(treeData);
    callbacks.setSessionCount(count);
    // Sync open tab titles with latest tree data
    callbacks.syncTabsWithTree(treeData);
    window.dispatchEvent(new CustomEvent("usage-data-changed"));
  }

  async function syncFromDisk(options?: { showSpinner?: boolean }) {
    const showSpinner = options?.showSpinner ?? false;

    if (syncInFlight) {
      pendingFullSync = true;
      return;
    }

    syncInFlight = true;
    if (showSpinner) {
      callbacks.setIsLoading(true);
    }

    try {
      await reindex();
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
        void syncFromDisk({ showSpinner });
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
        void syncFromDisk({ showSpinner });
      }
    }
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
    // common; retry with linear backoff before deferring to the next launch.
    const PRICING_FETCH_ATTEMPTS = 3;
    const RETRY_BACKOFF_MS = 2000;
    for (let attempt = 1; attempt <= PRICING_FETCH_ATTEMPTS; attempt++) {
      try {
        await refreshPricingCatalog();
        await clearUsageStats();
        return;
      } catch (error) {
        if (attempt === PRICING_FETCH_ATTEMPTS) {
          toastError(`${t("usage.firstUseBootstrapFailed")}: ${String(error)}`);
          return;
        }
        await new Promise((resolve) => setTimeout(resolve, RETRY_BACKOFF_MS * attempt));
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
  }

  return {
    syncFromDisk,
    syncProviders,
    refreshTree,
    coldStart,
  };
}
