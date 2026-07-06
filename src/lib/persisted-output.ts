import { resolvePersistedOutput } from "@/lib/tauri";

const TAG_START = "<persisted-output>";
const TAG_END = "</persisted-output>";
const PATH_PREFIX_PRIMARY = "Full output saved to: ";
const PATH_PREFIX_FALLBACK = "saved to: ";

/// Per-process cache of resolved persisted-output paths so the same
/// content rendered in multiple message rows hits the network at most
/// once. Keyed by absolute path; values are the file contents.
///
/// The Rust backend keeps its own LRU; this layer avoids the IPC round
/// trip on hot reads (e.g., scrolling between adjacent visible bubbles).
const cache = new Map<string, string>();
const inflight = new Map<string, Promise<string>>();

function extractPathFromInner(inner: string): string | null {
  for (const raw of inner.split(/\r?\n/)) {
    const line = raw.trim();
    if (line.startsWith(PATH_PREFIX_PRIMARY)) {
      return line.slice(PATH_PREFIX_PRIMARY.length).trim();
    }
    const idx = line.indexOf(PATH_PREFIX_FALLBACK);
    if (idx >= 0) {
      return line.slice(idx + PATH_PREFIX_FALLBACK.length).trim();
    }
  }
  return null;
}

/// Find the unique paths referenced by `<persisted-output>` blocks in
/// `content`. Returns an empty array when the content has no tags.
export function extractPersistedOutputPaths(content: string): string[] {
  if (!content.includes(TAG_START)) return [];
  const out = new Set<string>();
  let cursor = 0;
  while (true) {
    const i = content.indexOf(TAG_START, cursor);
    if (i < 0) break;
    const j = content.indexOf(TAG_END, i + TAG_START.length);
    if (j < 0) break;
    const inner = content.slice(i + TAG_START.length, j);
    const path = extractPathFromInner(inner);
    if (path) out.add(path);
    cursor = j + TAG_END.length;
  }
  return [...out];
}

/// Replace each `<persisted-output>` tag block whose embedded path is
/// present in `replacements` with the corresponding resolved content.
/// Tags without a matched replacement are left untouched, so partially
/// resolved content stays readable.
export function substitutePersistedOutputs(
  content: string,
  replacements: Map<string, string>,
): string {
  if (!content.includes(TAG_START)) return content;
  let result = "";
  let cursor = 0;
  while (true) {
    const i = content.indexOf(TAG_START, cursor);
    if (i < 0) {
      result += content.slice(cursor);
      break;
    }
    result += content.slice(cursor, i);
    const j = content.indexOf(TAG_END, i + TAG_START.length);
    if (j < 0) {
      // Unclosed tag — keep verbatim.
      result += content.slice(i);
      break;
    }
    const inner = content.slice(i + TAG_START.length, j);
    const path = extractPathFromInner(inner);
    const resolved = path !== null ? replacements.get(path) : undefined;
    if (resolved !== undefined) {
      result += resolved;
    } else {
      result += content.slice(i, j + TAG_END.length);
    }
    cursor = j + TAG_END.length;
  }
  return result;
}

/// Resolve a single persisted-output file. Returns the cached value when
/// available; deduplicates concurrent requests for the same path.
export async function loadPersistedOutput(path: string): Promise<string> {
  const hit = cache.get(path);
  if (hit !== undefined) return hit;
  const pending = inflight.get(path);
  if (pending) return pending;

  const promise = resolvePersistedOutput(path)
    .then((content) => {
      cache.set(path, content);
      inflight.delete(path);
      return content;
    })
    .catch((error) => {
      inflight.delete(path);
      throw error;
    });
  inflight.set(path, promise);
  return promise;
}
