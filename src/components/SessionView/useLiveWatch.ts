import { createEffect, on, onCleanup } from "solid-js";
import type { Accessor } from "solid-js";
import { listenBackendEvent, type UnlistenFn } from "../../lib/backend-events";
import {
  getProviderWatchConfig,
  getProviderWatchVersion,
  loadProviderWatchSnapshots,
} from "../../lib/provider-watch";
import type { Provider } from "../../lib/types";

export interface UseLiveWatchOptions {
  watching: Accessor<boolean>;
  provider: Accessor<Provider>;
  sourcePath: Accessor<string>;
  reload: () => Promise<void>;
}

/**
 * Manages the live-watch subscription for a session. When `watching` is true,
 * either polls (for DB-backed providers like OpenCode) or subscribes to the
 * `sessions-changed` FS event. All timers and unlisten fns are owned here so
 * the parent component doesn't juggle them inline.
 *
 * Each effect iteration captures its own `cancelled` flag: if a newer
 * iteration (or `onCleanup`) fires while `await listen(..)` is still in
 * flight, the awaited `unlisten` is invoked immediately on resolution so the
 * stale subscription doesn't outlive the iteration that spawned it.
 */
export function useLiveWatch(opts: UseLiveWatchOptions): void {
  let unwatchFn: UnlistenFn | undefined;
  let pollTimer: ReturnType<typeof setInterval> | undefined;
  let watchDebounce: ReturnType<typeof setTimeout> | undefined;
  let currentIteration: { cancelled: boolean } | undefined;

  createEffect(
    on(
      () =>
        [
          opts.watching(),
          opts.provider(),
          opts.sourcePath(),
          getProviderWatchVersion(),
        ] as const,
      async ([isWatching]) => {
        // Cancel the previous iteration's in-flight listen() if any, then
        // clean up whatever it already installed.
        if (currentIteration) currentIteration.cancelled = true;
        clearTimeout(watchDebounce);
        clearInterval(pollTimer);
        pollTimer = undefined;
        unwatchFn?.();
        unwatchFn = undefined;

        if (!isWatching) {
          currentIteration = undefined;
          return;
        }

        const iteration = { cancelled: false };
        currentIteration = iteration;

        void loadProviderWatchSnapshots();

        const activeSourcePath = opts.sourcePath();
        const watchConfig = getProviderWatchConfig(opts.provider());

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

        if (iteration.cancelled) {
          // A newer iteration (or onCleanup) ran while listen() was in
          // flight; drop the now-stale subscription immediately rather than
          // letting it leak until the next flip.
          unlisten();
          return;
        }
        unwatchFn = unlisten;
      },
    ),
  );

  onCleanup(() => {
    if (currentIteration) currentIteration.cancelled = true;
    clearTimeout(watchDebounce);
    clearInterval(pollTimer);
    pollTimer = undefined;
    unwatchFn?.();
  });
}
