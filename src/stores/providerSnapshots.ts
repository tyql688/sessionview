import { createSignal } from "solid-js";
import { getProviderSnapshots } from "../lib/tauri";
import type { Provider, ProviderSnapshot } from "../lib/types";

type ProviderSnapshotMap = Partial<Record<Provider, ProviderSnapshot>>;
type ProviderWatchStrategy = ProviderSnapshot["watch_strategy"];

const [providerSnapshotMap, setProviderSnapshotMap] =
  createSignal<ProviderSnapshotMap>({});
const [providerSnapshotVersion, setProviderSnapshotVersion] = createSignal(0);

const FALLBACK_PROVIDER_SNAPSHOTS: Record<Provider, ProviderSnapshot> = {
  claude: {
    key: "claude",
    label: "Claude Code",
    color: "var(--claude)",
    sort_order: 0,
    watch_strategy: "fs",
    path: "",
    exists: false,
    session_count: 0,
  },
  "cc-mirror": {
    key: "cc-mirror",
    label: "CC-Mirror",
    color: "var(--cc-mirror)",
    sort_order: 1,
    watch_strategy: "fs",
    path: "",
    exists: false,
    session_count: 0,
  },
  codex: {
    key: "codex",
    label: "Codex",
    color: "var(--codex)",
    sort_order: 2,
    watch_strategy: "fs",
    path: "",
    exists: false,
    session_count: 0,
  },
  gemini: {
    key: "gemini",
    label: "Gemini",
    color: "var(--gemini)",
    sort_order: 3,
    watch_strategy: "poll",
    path: "",
    exists: false,
    session_count: 0,
  },
  opencode: {
    key: "opencode",
    label: "OpenCode",
    color: "var(--opencode)",
    sort_order: 5,
    watch_strategy: "poll",
    path: "",
    exists: false,
    session_count: 0,
  },
  kimi: {
    key: "kimi",
    label: "Kimi CLI",
    color: "var(--kimi)",
    sort_order: 6,
    watch_strategy: "fs",
    path: "",
    exists: false,
    session_count: 0,
  },
  qwen: {
    key: "qwen",
    label: "Qwen Code",
    color: "var(--qwen)",
    sort_order: 7,
    watch_strategy: "fs",
    path: "",
    exists: false,
    session_count: 0,
  },
};

let loadPromise: Promise<void> | null = null;

function activeProviderSnapshotMap(): Record<Provider, ProviderSnapshot> {
  const loaded = providerSnapshotMap();
  return {
    ...FALLBACK_PROVIDER_SNAPSHOTS,
    ...loaded,
  };
}

export async function loadProviderSnapshots(force = false) {
  if (!force && Object.keys(providerSnapshotMap()).length > 0) {
    return;
  }

  if (loadPromise) return loadPromise;

  loadPromise = getProviderSnapshots()
    .then((snapshots) => {
      const next: ProviderSnapshotMap = {};
      for (const snapshot of snapshots) {
        next[snapshot.key] = snapshot;
      }
      setProviderSnapshotMap(next);
      setProviderSnapshotVersion((version) => version + 1);
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
  return Object.values(activeProviderSnapshotMap()).sort(
    (left, right) => left.sort_order - right.sort_order,
  );
}

export function getProviderSnapshot(provider: Provider): ProviderSnapshot {
  return activeProviderSnapshotMap()[provider];
}

export function getProviderLabel(
  provider: Provider,
  variantName?: string,
): string {
  if (provider === "cc-mirror") {
    return variantName || getProviderSnapshot(provider).label;
  }
  return getProviderSnapshot(provider).label;
}

export function getProviderColor(provider: Provider): string {
  return getProviderSnapshot(provider).color;
}

export function getProviderWatchStrategy(
  provider: Provider,
): ProviderWatchStrategy {
  return getProviderSnapshot(provider).watch_strategy;
}

export function getProvidersForWatchStrategy(
  strategy: ProviderWatchStrategy,
): Provider[] {
  return (
    Object.entries(activeProviderSnapshotMap()) as [
      Provider,
      ProviderSnapshot,
    ][]
  )
    .filter(([, snapshot]) => snapshot.watch_strategy === strategy)
    .map(([provider]) => provider);
}

export function getProviderSortOrder(provider: Provider): number {
  return getProviderSnapshot(provider).sort_order;
}

export function getProviderSnapshotVersion(): number {
  return providerSnapshotVersion();
}
