import { useEffect } from "react";
import { listenBackendEvent, type UnlistenFn } from "../../lib/backend-events";
import {
  getProviderWatchConfig,
  loadProviderWatchSnapshots,
} from "../../lib/provider-watch";
import { useProviderSnapshotVersion } from "../../stores/providerSnapshots";
import type { Provider } from "../../lib/types";

export interface UseLiveWatchOptions {
  watching: boolean;
  provider: Provider;
  sourcePath: string;
  reload: () => Promise<void>;
}

/**
 * Manages the live-watch subscription for a session. When `watching` is true,
 * either polls (for DB-backed providers like OpenCode) or subscribes to the
 * `sessions-changed` FS event. All timers and unlisten fns are owned here so
 * the parent component doesn't juggle them inline.
 *
 * Each effect iteration captures its own `cancelled` flag: if a newer
 * iteration (or cleanup) fires while `await listen(..)` is still in
 * flight, the awaited `unlisten` is invoked immediately on resolution so the
 * stale subscription doesn't outlive the iteration that spawned it.
 */
export function useLiveWatch(opts: UseLiveWatchOptions): void {
  // Reactive read so the effect re-runs when provider snapshots finish loading
  // (mirrors the Solid `getProviderWatchVersion()` dep in the original `on(...)`).
  const snapshotVersion = useProviderSnapshotVersion();

  useEffect(() => {
    let cancelled = false;
    let unwatchFn: UnlistenFn | undefined;
    let pollTimer: ReturnType<typeof setInterval> | undefined;
    let watchDebounce: ReturnType<typeof setTimeout> | undefined;

    const run = async () => {
      if (!opts.watching) return;

      void loadProviderWatchSnapshots();

      const activeSourcePath = opts.sourcePath;
      const watchConfig = getProviderWatchConfig(opts.provider);

      if (watchConfig.strategy === "poll") {
        pollTimer = setInterval(
          () => void opts.reload(),
          watchConfig.debounceMs,
        );
        return;
      }

      const unlisten = await listenBackendEvent(
        "sessions-changed",
        (payload) => {
          const changedPaths = payload ?? [];
          if (!activeSourcePath) return;
          if (!changedPaths.includes(activeSourcePath)) return;

          clearTimeout(watchDebounce);
          watchDebounce = setTimeout(
            () => void opts.reload(),
            watchConfig.debounceMs,
          );
        },
      );

      if (cancelled) {
        // A newer iteration (or onCleanup) ran while listen() was in
        // flight; drop the now-stale subscription immediately rather than
        // letting it leak until the next flip.
        unlisten();
        return;
      }
      unwatchFn = unlisten;
    };

    void run();

    // Cancel the in-flight listen() if any, then clean up whatever this
    // iteration already installed. React runs this before the next effect
    // iteration (dep change) and on unmount.
    return () => {
      cancelled = true;
      clearTimeout(watchDebounce);
      clearInterval(pollTimer);
      pollTimer = undefined;
      unwatchFn?.();
    };
    // Deps mirror the Solid `on([watching, provider, sourcePath, watchVersion])`
    // array exactly; `reload` is intentionally read but not a dependency so a
    // fresh `reload` identity each render doesn't tear down the subscription.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opts.watching, opts.provider, opts.sourcePath, snapshotVersion]);
}
