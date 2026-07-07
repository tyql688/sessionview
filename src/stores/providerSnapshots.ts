import { create } from "zustand";
import { getProviderSnapshots } from "@/lib/tauri";
import type { Provider, ProviderSnapshot } from "@/lib/types";

type ProviderSnapshotMap = Partial<Record<Provider, ProviderSnapshot>>;

interface ProviderSnapshotState {
  snapshotMap: ProviderSnapshotMap;
  version: number;
}

const useProviderSnapshotStore = create<ProviderSnapshotState>(() => ({
  snapshotMap: {},
  version: 0,
}));

const FALLBACK_PROVIDER_SNAPSHOTS: Record<Provider, ProviderSnapshot> = {
  claude: {
    key: "claude",
    label: "Claude Code",
    color: "var(--claude)",
    sort_order: 0,
    path: "",
    exists: false,
    session_count: 0,
  },
  "cc-mirror": {
    key: "cc-mirror",
    label: "CC-Mirror",
    color: "var(--cc-mirror)",
    sort_order: 1,
    path: "",
    exists: false,
    session_count: 0,
  },
  codex: {
    key: "codex",
    label: "Codex",
    color: "var(--codex)",
    sort_order: 2,
    path: "",
    exists: false,
    session_count: 0,
  },
  antigravity: {
    key: "antigravity",
    label: "Antigravity",
    color: "var(--antigravity)",
    sort_order: 3,
    path: "",
    exists: false,
    session_count: 0,
  },
  opencode: {
    key: "opencode",
    label: "OpenCode",
    color: "var(--opencode)",
    sort_order: 5,
    path: "",
    exists: false,
    session_count: 0,
  },
  kimi: {
    key: "kimi",
    label: "Kimi Code",
    color: "var(--kimi)",
    sort_order: 6,
    path: "",
    exists: false,
    session_count: 0,
  },
  cursor: {
    key: "cursor",
    label: "Cursor CLI",
    color: "var(--cursor)",
    sort_order: 7,
    path: "",
    exists: false,
    session_count: 0,
  },
  pi: {
    key: "pi",
    label: "Pi",
    color: "var(--pi)",
    sort_order: 8,
    path: "",
    exists: false,
    session_count: 0,
  },
};

let loadPromise: Promise<void> | null = null;

function activeProviderSnapshotMap(): Record<Provider, ProviderSnapshot> {
  return {
    ...FALLBACK_PROVIDER_SNAPSHOTS,
    ...useProviderSnapshotStore.getState().snapshotMap,
  };
}

export async function loadProviderSnapshots(force = false) {
  if (!force && Object.keys(useProviderSnapshotStore.getState().snapshotMap).length > 0) {
    return;
  }

  if (loadPromise) return loadPromise;

  loadPromise = getProviderSnapshots()
    .then((snapshots) => {
      const next: ProviderSnapshotMap = {};
      for (const snapshot of snapshots) {
        next[snapshot.key] = snapshot;
      }
      useProviderSnapshotStore.setState((state) => ({
        snapshotMap: next,
        version: state.version + 1,
      }));
    })
    .catch((error) => {
      console.warn("failed to load provider snapshots:", error);
    })
    .finally(() => {
      loadPromise = null;
    });

  return loadPromise;
}

export function refreshProviderSnapshots() {
  return loadProviderSnapshots(true);
}

export function listProviderSnapshots(): ProviderSnapshot[] {
  return Object.values(activeProviderSnapshotMap()).sort((left, right) => left.sort_order - right.sort_order);
}

export function getProviderSnapshot(provider: Provider): ProviderSnapshot {
  return activeProviderSnapshotMap()[provider];
}

export function getProviderLabel(provider: Provider, variantName?: string): string {
  if (provider === "cc-mirror") {
    return variantName || getProviderSnapshot(provider).label;
  }
  return getProviderSnapshot(provider).label;
}

export function getProviderColor(provider: Provider): string {
  return getProviderSnapshot(provider).color;
}

export function getProviderSortOrder(provider: Provider): number {
  return getProviderSnapshot(provider).sort_order;
}

export function getProviderSnapshotVersion(): number {
  return useProviderSnapshotStore.getState().version;
}

/**
 * Reactive subscription for React components that render provider metadata:
 * re-renders when snapshots finish loading (bumps `version`). Imperative
 * callers (lib/*) keep using the plain getters above.
 */
export function useProviderSnapshotVersion(): number {
  return useProviderSnapshotStore((state) => state.version);
}
