import {
  getProvidersForWatchStrategy,
  getProviderWatchStrategy,
  loadProviderSnapshots,
} from "@/stores/providerSnapshots";
import type { Provider } from "@/lib/types";

export interface ProviderWatchBehavior {
  debounceMs: number;
}

export interface ProviderWatchConfig extends ProviderWatchBehavior {
  strategy: ReturnType<typeof getProviderWatchStrategy>;
}

const WATCH_BEHAVIORS: Record<Provider, ProviderWatchBehavior> = {
  claude: { debounceMs: 300 },
  codex: { debounceMs: 300 },
  antigravity: { debounceMs: 300 },
  opencode: { debounceMs: 2000 },
  kimi: { debounceMs: 300 },
  cursor: { debounceMs: 300 },
  "cc-mirror": { debounceMs: 300 },
  pi: { debounceMs: 300 },
};

export function getProviderWatchConfig(
  provider: Provider,
): ProviderWatchConfig {
  return {
    ...WATCH_BEHAVIORS[provider],
    strategy: getProviderWatchStrategy(provider),
  };
}

export function getPollWatchProviders(): Provider[] {
  return getProvidersForWatchStrategy("poll");
}

export function loadProviderWatchSnapshots(): Promise<void> | undefined {
  return loadProviderSnapshots();
}
