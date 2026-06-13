import { invoke } from "@tauri-apps/api/core";
import { errorMessage } from "./errors";
import { toastError } from "../stores/toast";
import type {
  BatchResult,
  SessionDetail,
  SearchResult,
  SearchFilters,
  TreeNode,
  IndexStats,
  PricingCatalogStatus,
  ProviderSnapshot,
  TrashMeta,
  SessionMeta,
  TokenTotals,
  UsageStats,
  ActivityCalendar,
  Message,
} from "./types";

/// Sentinel returned by the backend when a load was cancelled mid-flight.
/// Frontend treats this as silent — no toast, no error UI.
export const LOAD_CANCELED_SENTINEL = "__cc_session_load_canceled__";

export function isLoadCanceledError(err: unknown): boolean {
  if (err == null) return false;
  const msg =
    typeof err === "string"
      ? err
      : ((err as { message?: string }).message ?? "");
  return msg.includes(LOAD_CANCELED_SENTINEL);
}

export interface SessionMessagesWindow {
  total: number;
  start: number;
  messages: Message[];
  parse_warning_count: number;
  token_totals: TokenTotals;
}

export interface SessionOpenWindow {
  meta: SessionMeta;
  window: SessionMessagesWindow;
}

/**
 * Wrap a Tauri invocation so failures surface to the user as a toast
 * (plus `console.error`) and then rethrow. Use for user-triggered
 * actions (rename, export, delete, resume…) where the caller needs
 * to know the call failed.
 *
 * The argument is an already-started `Promise<T>` (eager evaluation);
 * if the underlying call can throw **synchronously** before returning
 * the promise, wrap it in a thunk yourself:
 *   invokeWithToast(Promise.resolve().then(() => invoke(...)), "...")
 *
 * Rethrows the original error verbatim (preserves `instanceof` and
 * stack); the `context` prefix is embedded only in the log + toast,
 * not in the thrown error's message.
 *
 * @example
 *   await invokeWithToast(renameSession(id, title), "rename session");
 */
export async function invokeWithToast<T>(
  promise: Promise<T>,
  context: string,
): Promise<T> {
  try {
    return await promise;
  } catch (err) {
    const message = `${context}: ${errorMessage(err)}`;
    console.error(message);
    toastError(message);
    throw err;
  }
}

/**
 * Wrap a Tauri invocation used for background / status refreshes.
 * Failures are logged via `console.error` with `context` for
 * diagnosability but do not toast (to avoid noise when the backend
 * hiccups), and the fallback value is returned so callers can render
 * a safe default instead of propagating undefined.
 *
 * Like `invokeWithToast`, the argument is evaluated eagerly — wrap
 * in a thunk if the call can throw synchronously.
 *
 * @example
 *   const cost = await invokeWithFallback(getTodayCost(), undefined, "refresh today cost");
 */
export async function invokeWithFallback<T, D = T>(
  promise: Promise<T>,
  fallback: D,
  context: string,
): Promise<T | D> {
  try {
    return await promise;
  } catch (err) {
    console.error(`${context}: ${errorMessage(err)}`);
    return fallback;
  }
}

export async function reindex(): Promise<number> {
  return invoke<number>("reindex");
}

export async function reindexProviders(
  providers: string[],
  aggressive = false,
): Promise<number> {
  return invoke<number>("reindex_providers", { providers, aggressive });
}

export async function syncSources(paths: string[]): Promise<number> {
  return invoke<number>("sync_sources", { paths });
}

export async function getTree(): Promise<TreeNode[]> {
  return invoke<TreeNode[]>("get_tree");
}

export async function getSessionDetail(
  sessionId: string,
): Promise<SessionDetail> {
  return invoke<SessionDetail>("get_session_detail", { sessionId });
}

export async function getSessionMeta(sessionId: string): Promise<SessionMeta> {
  return invoke<SessionMeta>("get_session_meta", { sessionId });
}

/// Fetch session metadata plus the initial message window in one IPC.
export async function getSessionOpenWindow(
  sessionId: string,
  offset: number,
  limit: number,
): Promise<SessionOpenWindow> {
  return invoke<SessionOpenWindow>("get_session_open_window", {
    sessionId,
    offset,
    limit,
  });
}

/// Fetch a window of messages from the cached parsed session.
/// `offset < 0` indexes from the end (e.g., -1 selects "last `limit`").
export async function getSessionMessagesWindow(
  sessionId: string,
  offset: number,
  limit: number,
): Promise<SessionMessagesWindow> {
  return invoke<SessionMessagesWindow>("get_session_messages_window", {
    sessionId,
    offset,
    limit,
  });
}

export async function cancelSessionLoad(sessionId: string): Promise<void> {
  return invoke<void>("cancel_session_load", { sessionId });
}

export async function resolvePersistedOutput(path: string): Promise<string> {
  return invoke<string>("resolve_persisted_output", { path });
}

export async function searchSessions(
  filters: SearchFilters,
): Promise<SearchResult[]> {
  return invoke<SearchResult[]>("search_sessions", { filters });
}

export async function renameSession(
  sessionId: string,
  newTitle: string,
): Promise<void> {
  return invoke<void>("rename_session", { sessionId, newTitle });
}

export async function getSessionCount(): Promise<number> {
  return invoke<number>("get_session_count");
}

export async function exportSession(
  sessionId: string,
  format: string,
  outputPath: string,
): Promise<void> {
  return invoke<void>("export_session", {
    sessionId,
    format,
    outputPath,
  });
}

export async function getChildSessions(
  parentId: string,
): Promise<SessionMeta[]> {
  return invoke<SessionMeta[]>("get_child_sessions", { parentId });
}

export async function getChildSessionCounts(
  parentIds: string[],
): Promise<Record<string, number>> {
  return invoke<Record<string, number>>("get_child_session_counts", {
    parentIds,
  });
}

export async function getIndexStats(): Promise<IndexStats> {
  return invoke<IndexStats>("get_index_stats");
}

export async function getPricingCatalogStatus(): Promise<PricingCatalogStatus> {
  return invoke<PricingCatalogStatus>("get_pricing_catalog_status");
}

export async function refreshPricingCatalog(): Promise<PricingCatalogStatus> {
  return invoke<PricingCatalogStatus>("refresh_pricing_catalog");
}

export async function startRebuildIndex(): Promise<boolean> {
  return invoke<boolean>("start_rebuild_index");
}

export async function clearIndex(): Promise<void> {
  return invoke<void>("clear_index");
}

export async function startRefreshUsage(): Promise<boolean> {
  return invoke<boolean>("start_refresh_usage");
}

export async function clearUsageStats(): Promise<void> {
  return invoke<void>("clear_usage_stats");
}

export async function detectTerminal(): Promise<string> {
  return invoke<string>("detect_terminal");
}

export async function getProviderSnapshots(): Promise<ProviderSnapshot[]> {
  return invoke<ProviderSnapshot[]>("get_provider_snapshots");
}

export async function resumeSession(
  sessionId: string,
  terminalApp: string,
): Promise<void> {
  return invoke<void>("resume_session", { sessionId, terminalApp });
}

export async function getResumeCommand(sessionId: string): Promise<string> {
  return invoke<string>("get_resume_command", { sessionId });
}

export async function trashSession(sessionId: string): Promise<void> {
  return invoke<void>("trash_session", { sessionId });
}

export async function listTrash(): Promise<TrashMeta[]> {
  return invoke<TrashMeta[]>("list_trash");
}

export async function restoreSession(trashId: string): Promise<void> {
  return invoke<void>("restore_session", { trashId });
}

export async function emptyTrash(): Promise<void> {
  return invoke<void>("empty_trash");
}

export async function permanentDeleteTrash(trashId: string): Promise<void> {
  return invoke<void>("permanent_delete_trash", { trashId });
}

export async function trashSessionsBatch(
  items: string[],
): Promise<BatchResult> {
  return invoke<BatchResult>("trash_sessions_batch", { items });
}

export async function restoreSessionsBatch(
  items: string[],
): Promise<BatchResult> {
  return invoke<BatchResult>("restore_sessions_batch", { items });
}

export async function permanentDeleteTrashBatch(
  items: string[],
): Promise<BatchResult> {
  return invoke<BatchResult>("permanent_delete_trash_batch", { items });
}

export async function listRecentSessions(
  limit: number,
): Promise<SessionMeta[]> {
  return invoke<SessionMeta[]>("list_recent_sessions", { limit });
}

export async function toggleFavorite(sessionId: string): Promise<boolean> {
  return invoke<boolean>("toggle_favorite", { sessionId });
}

export async function listFavorites(): Promise<SessionMeta[]> {
  return invoke<SessionMeta[]>("list_favorites");
}

export async function isFavorite(sessionId: string): Promise<boolean> {
  return invoke<boolean>("is_favorite", { sessionId });
}

export async function readImageBase64(path: string): Promise<string> {
  return invoke<string>("read_image_base64", { path });
}

export async function readToolResultText(path: string): Promise<string> {
  return invoke<string>("read_tool_result_text", { path });
}

export async function openInFolder(path: string): Promise<void> {
  return invoke<void>("open_in_folder", { path });
}

export async function exportSessionsBatch(
  items: string[],
  format: string,
  outputPath: string,
): Promise<void> {
  return invoke<void>("export_sessions_batch", { items, format, outputPath });
}

export async function getUsageStats(
  providers: string[],
  rangeDays: number | null,
  dateStart: string | null = null,
  dateEnd: string | null = null,
): Promise<UsageStats> {
  return invoke<UsageStats>("get_usage_stats", {
    providers,
    rangeDays,
    dateStart,
    dateEnd,
  });
}

/** GitHub-style activity calendar over an inclusive [dateStart, dateEnd] window.
 *  The window is independent of the usage panel's range filter. */
export async function getActivityCalendar(
  providers: string[],
  dateStart: string,
  dateEnd: string,
): Promise<ActivityCalendar> {
  return invoke<ActivityCalendar>("get_activity_calendar", {
    providers,
    dateStart,
    dateEnd,
  });
}

export async function getTodayCost(): Promise<number> {
  return invoke<number>("get_today_cost");
}

export interface TodayTokens {
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
}

export async function getTodayTokens(): Promise<TodayTokens> {
  return invoke<TodayTokens>("get_today_tokens");
}
