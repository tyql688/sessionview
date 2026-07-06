import { invoke } from "@tauri-apps/api/core";
import { errorMessage } from "@/lib/errors";
import { toastError } from "@/stores/toast";
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
} from "@/lib/types";

/// Sentinel returned by the backend when a load was cancelled mid-flight.
/// Frontend treats this as silent — no toast, no error UI.
const LOAD_CANCELED_SENTINEL = "__cc_session_load_canceled__";

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

export interface SessionTurnOutlineEntry {
  ordinal: number;
  message_index: number;
  user_text: string;
  reply_text: string;
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

type CommandSpec<Args, Result> = {
  args: Args;
  result: Result;
};

type BackendCommandMap = {
  reindex: CommandSpec<undefined, number>;
  reindex_providers: CommandSpec<
    { providers: string[]; aggressive: boolean },
    number
  >;
  sync_sources: CommandSpec<{ paths: string[] }, number>;
  get_tree: CommandSpec<undefined, TreeNode[]>;
  get_session_detail: CommandSpec<{ sessionId: string }, SessionDetail>;
  get_session_meta: CommandSpec<{ sessionId: string }, SessionMeta>;
  get_session_open_window: CommandSpec<
    { sessionId: string; offset: number; limit: number },
    SessionOpenWindow
  >;
  get_session_messages_window: CommandSpec<
    { sessionId: string; offset: number; limit: number },
    SessionMessagesWindow
  >;
  get_session_turn_outline: CommandSpec<
    { sessionId: string },
    SessionTurnOutlineEntry[]
  >;
  cancel_session_load: CommandSpec<{ sessionId: string }, void>;
  resolve_persisted_output: CommandSpec<{ path: string }, string>;
  search_sessions: CommandSpec<{ filters: SearchFilters }, SearchResult[]>;
  rename_session: CommandSpec<{ sessionId: string; newTitle: string }, void>;
  get_session_count: CommandSpec<undefined, number>;
  export_session: CommandSpec<
    { sessionId: string; format: string; outputPath: string },
    void
  >;
  get_child_sessions: CommandSpec<{ parentId: string }, SessionMeta[]>;
  get_child_session_counts: CommandSpec<
    { parentIds: string[] },
    Record<string, number>
  >;
  get_index_stats: CommandSpec<undefined, IndexStats>;
  get_pricing_catalog_status: CommandSpec<undefined, PricingCatalogStatus>;
  refresh_pricing_catalog: CommandSpec<undefined, PricingCatalogStatus>;
  start_rebuild_index: CommandSpec<undefined, boolean>;
  clear_index: CommandSpec<undefined, void>;
  start_refresh_usage: CommandSpec<undefined, boolean>;
  clear_usage_stats: CommandSpec<undefined, void>;
  detect_terminal: CommandSpec<undefined, string>;
  get_provider_snapshots: CommandSpec<undefined, ProviderSnapshot[]>;
  resume_session: CommandSpec<{ sessionId: string; terminalApp: string }, void>;
  get_resume_command: CommandSpec<{ sessionId: string }, string>;
  trash_session: CommandSpec<{ sessionId: string }, void>;
  list_trash: CommandSpec<undefined, TrashMeta[]>;
  restore_session: CommandSpec<{ trashId: string }, void>;
  empty_trash: CommandSpec<undefined, void>;
  permanent_delete_trash: CommandSpec<{ trashId: string }, void>;
  trash_sessions_batch: CommandSpec<{ items: string[] }, BatchResult>;
  restore_sessions_batch: CommandSpec<{ items: string[] }, BatchResult>;
  permanent_delete_trash_batch: CommandSpec<{ items: string[] }, BatchResult>;
  list_recent_sessions: CommandSpec<{ limit: number }, SessionMeta[]>;
  toggle_favorite: CommandSpec<{ sessionId: string }, boolean>;
  list_favorites: CommandSpec<undefined, SessionMeta[]>;
  is_favorite: CommandSpec<{ sessionId: string }, boolean>;
  read_image_base64: CommandSpec<{ path: string }, string>;
  read_tool_result_text: CommandSpec<{ path: string }, string>;
  open_in_folder: CommandSpec<{ path: string }, void>;
  export_sessions_batch: CommandSpec<
    { items: string[]; format: string; outputPath: string },
    void
  >;
  get_usage_stats: CommandSpec<
    {
      providers: string[];
      rangeDays: number | null;
      dateStart: string | null;
      dateEnd: string | null;
    },
    UsageStats
  >;
  get_activity_calendar: CommandSpec<
    { providers: string[]; dateStart: string; dateEnd: string },
    ActivityCalendar
  >;
  get_today_cost: CommandSpec<undefined, number>;
  get_today_tokens: CommandSpec<undefined, TodayTokens>;
};

type CommandArgs<Name extends keyof BackendCommandMap> =
  BackendCommandMap[Name]["args"];
type CommandResultFor<Name extends keyof BackendCommandMap> =
  BackendCommandMap[Name]["result"];

function invokeCommand<Name extends keyof BackendCommandMap>(
  name: Name,
  ...args: CommandArgs<Name> extends undefined ? [] : [CommandArgs<Name>]
): Promise<CommandResultFor<Name>> {
  if (args.length === 0) {
    return invoke<CommandResultFor<Name>>(name);
  }
  return invoke<CommandResultFor<Name>>(name, args[0]);
}

export async function reindex(): Promise<number> {
  return invokeCommand("reindex");
}

export async function reindexProviders(
  providers: string[],
  aggressive = false,
): Promise<number> {
  return invokeCommand("reindex_providers", { providers, aggressive });
}

export async function syncSources(paths: string[]): Promise<number> {
  return invokeCommand("sync_sources", { paths });
}

export async function getTree(): Promise<TreeNode[]> {
  return invokeCommand("get_tree");
}

export async function getSessionDetail(
  sessionId: string,
): Promise<SessionDetail> {
  return invokeCommand("get_session_detail", { sessionId });
}

/// Fetch session metadata plus the initial message window in one IPC.
export async function getSessionOpenWindow(
  sessionId: string,
  offset: number,
  limit: number,
): Promise<SessionOpenWindow> {
  return invokeCommand("get_session_open_window", {
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
  return invokeCommand("get_session_messages_window", {
    sessionId,
    offset,
    limit,
  });
}

export async function getSessionTurnOutline(
  sessionId: string,
): Promise<SessionTurnOutlineEntry[]> {
  return invokeCommand("get_session_turn_outline", {
    sessionId,
  });
}

export async function cancelSessionLoad(sessionId: string): Promise<void> {
  return invokeCommand("cancel_session_load", { sessionId });
}

export async function resolvePersistedOutput(path: string): Promise<string> {
  return invokeCommand("resolve_persisted_output", { path });
}

export async function searchSessions(
  filters: SearchFilters,
): Promise<SearchResult[]> {
  return invokeCommand("search_sessions", { filters });
}

export async function renameSession(
  sessionId: string,
  newTitle: string,
): Promise<void> {
  return invokeCommand("rename_session", { sessionId, newTitle });
}

export async function getSessionCount(): Promise<number> {
  return invokeCommand("get_session_count");
}

export async function exportSession(
  sessionId: string,
  format: string,
  outputPath: string,
): Promise<void> {
  return invokeCommand("export_session", {
    sessionId,
    format,
    outputPath,
  });
}

export async function getChildSessions(
  parentId: string,
): Promise<SessionMeta[]> {
  return invokeCommand("get_child_sessions", { parentId });
}

export async function getChildSessionCounts(
  parentIds: string[],
): Promise<Record<string, number>> {
  return invokeCommand("get_child_session_counts", {
    parentIds,
  });
}

export async function getIndexStats(): Promise<IndexStats> {
  return invokeCommand("get_index_stats");
}

export async function getPricingCatalogStatus(): Promise<PricingCatalogStatus> {
  return invokeCommand("get_pricing_catalog_status");
}

export async function refreshPricingCatalog(): Promise<PricingCatalogStatus> {
  return invokeCommand("refresh_pricing_catalog");
}

export async function startRebuildIndex(): Promise<boolean> {
  return invokeCommand("start_rebuild_index");
}

export async function clearIndex(): Promise<void> {
  return invokeCommand("clear_index");
}

export async function startRefreshUsage(): Promise<boolean> {
  return invokeCommand("start_refresh_usage");
}

export async function clearUsageStats(): Promise<void> {
  return invokeCommand("clear_usage_stats");
}

export async function detectTerminal(): Promise<string> {
  return invokeCommand("detect_terminal");
}

export async function getProviderSnapshots(): Promise<ProviderSnapshot[]> {
  return invokeCommand("get_provider_snapshots");
}

export async function resumeSession(
  sessionId: string,
  terminalApp: string,
): Promise<void> {
  return invokeCommand("resume_session", { sessionId, terminalApp });
}

export async function getResumeCommand(sessionId: string): Promise<string> {
  return invokeCommand("get_resume_command", { sessionId });
}

export async function trashSession(sessionId: string): Promise<void> {
  return invokeCommand("trash_session", { sessionId });
}

export async function listTrash(): Promise<TrashMeta[]> {
  return invokeCommand("list_trash");
}

export async function restoreSession(trashId: string): Promise<void> {
  return invokeCommand("restore_session", { trashId });
}

export async function emptyTrash(): Promise<void> {
  return invokeCommand("empty_trash");
}

export async function permanentDeleteTrash(trashId: string): Promise<void> {
  return invokeCommand("permanent_delete_trash", { trashId });
}

export async function trashSessionsBatch(
  items: string[],
): Promise<BatchResult> {
  return invokeCommand("trash_sessions_batch", { items });
}

export async function restoreSessionsBatch(
  items: string[],
): Promise<BatchResult> {
  return invokeCommand("restore_sessions_batch", { items });
}

export async function permanentDeleteTrashBatch(
  items: string[],
): Promise<BatchResult> {
  return invokeCommand("permanent_delete_trash_batch", { items });
}

export async function listRecentSessions(
  limit: number,
): Promise<SessionMeta[]> {
  return invokeCommand("list_recent_sessions", { limit });
}

export async function toggleFavorite(sessionId: string): Promise<boolean> {
  return invokeCommand("toggle_favorite", { sessionId });
}

export async function listFavorites(): Promise<SessionMeta[]> {
  return invokeCommand("list_favorites");
}

export async function isFavorite(sessionId: string): Promise<boolean> {
  return invokeCommand("is_favorite", { sessionId });
}

export async function readImageBase64(path: string): Promise<string> {
  return invokeCommand("read_image_base64", { path });
}

export async function readToolResultText(path: string): Promise<string> {
  return invokeCommand("read_tool_result_text", { path });
}

export async function openInFolder(path: string): Promise<void> {
  return invokeCommand("open_in_folder", { path });
}

export async function exportSessionsBatch(
  items: string[],
  format: string,
  outputPath: string,
): Promise<void> {
  return invokeCommand("export_sessions_batch", { items, format, outputPath });
}

export async function getUsageStats(
  providers: string[],
  rangeDays: number | null,
  dateStart: string | null = null,
  dateEnd: string | null = null,
): Promise<UsageStats> {
  return invokeCommand("get_usage_stats", {
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
  return invokeCommand("get_activity_calendar", {
    providers,
    dateStart,
    dateEnd,
  });
}

export async function getTodayCost(): Promise<number> {
  return invokeCommand("get_today_cost");
}

export interface TodayTokens {
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
}

export async function getTodayTokens(): Promise<TodayTokens> {
  return invokeCommand("get_today_tokens");
}
