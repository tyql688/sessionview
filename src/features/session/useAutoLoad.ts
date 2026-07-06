import { useEffect } from "react";

export interface UseAutoLoadOptions<T> {
  /** Tracked: re-run when the visible entry list changes. */
  visibleEntries: T[];
  loading: boolean;
  hasMore: boolean;
  /** Lazy ref getter — the DOM node may not exist on first run. */
  getMessagesRef: () => HTMLDivElement | undefined;
  loadMore: () => void;
  threshold: number;
}

/**
 * Automatically calls `loadMore` when the scroll container's content doesn't
 * fill the viewport and more entries are available — covers the initial-mount
 * case where the default window is smaller than the viewport height.
 */
export function useAutoLoad<T>(opts: UseAutoLoadOptions<T>): void {
  useEffect(() => {
    const ref = opts.getMessagesRef();
    if (opts.loading || !opts.hasMore || !ref) return;

    if (ref.scrollHeight <= ref.clientHeight + opts.threshold) {
      requestAnimationFrame(() => {
        opts.loadMore();
      });
    }
    // Re-run when the visible entry list / loading / hasMore change, matching the
    // Solid effect's tracked reads. getMessagesRef/loadMore/threshold are read but
    // not deps (stable getters), mirroring the original's non-reactive access.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opts.visibleEntries, opts.loading, opts.hasMore]);
}
