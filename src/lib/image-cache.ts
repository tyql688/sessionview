/**
 * Global URL-based image cache.
 *
 * Deduplicates image loading across component instances so that the same
 * source (local path or remote URL) is only fetched once.  The cache stores
 * the resolved display string (data-URL for local images, the original URL
 * for remote images) keyed by the original source identifier.
 *
 * No eviction -- the map lives for the lifetime of the page which is fine
 * for a desktop app where the working set is bounded.
 */

const cache = new Map<string, Promise<string>>();

/**
 * Return a cached result for `key`, or call `loader` exactly once to produce
 * one, cache the resulting promise, and return it.
 *
 * If the loader rejects the entry is removed so a later call can retry.
 */
export function cachedLoad(key: string, loader: () => Promise<string>): Promise<string> {
  const existing = cache.get(key);
  if (existing) return existing;

  const pending = loader().catch((err: unknown) => {
    cache.delete(key);
    throw err;
  });

  cache.set(key, pending);
  return pending;
}
