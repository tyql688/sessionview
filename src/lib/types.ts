/** Every supported provider, as the single runtime source of truth — the
 * `Provider` type derives from this array, so a hand-maintained list
 * elsewhere (which is how "pi" once went missing from tree grouping) can't
 * silently drift again. */
export const PROVIDERS = ["claude", "codex", "antigravity", "opencode", "kimi", "cursor", "cc-mirror", "pi"] as const;

export type Provider = (typeof PROVIDERS)[number];

export interface SessionMeta {
  id: string;
  provider: Provider;
  title: string;
  project_path: string;
  project_name: string;
  created_at: number;
  updated_at: number;
  message_count: number;
  file_size_bytes: number;
  source_path: string;
  is_sidechain: boolean;
  variant_name?: string;
  model?: string;
  cc_version?: string;
  git_branch?: string;
  parent_id?: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
}

export interface TokenTotals {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
}

/** Lightweight reference for opening sessions from the tree.
 * SessionMeta satisfies this interface via structural typing. */
export interface SessionRef {
  id: string;
  provider: Provider;
  title: string;
  project_name: string;
  is_sidechain: boolean;
  source_path?: string;
  project_path?: string;
}

export type MessageRole = "user" | "assistant" | "tool" | "system";
type MessageKind = "command_input" | "command_output";

export interface TokenUsage {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens: number;
  cache_read_input_tokens: number;
}

interface McpToolMetadata {
  server: string;
  tool: string;
  display: string;
}

type RawOutputPolicy = "keep" | "suppress_terminal" | "suppress_patch_when_diff_present";

export type ToolDiffLineType = "context" | "add" | "remove" | "skip";

export interface ToolDiffLine {
  type: ToolDiffLineType;
  oldLine: number | null;
  newLine: number | null;
  text: string;
}

interface ToolLine {
  label: string;
  value: string;
}

interface ToolDetail {
  lines: ToolLine[];
  diff?: { old: string; new: string };
  patchDiff?: ToolDiffLine[];
  persistedOutputPath?: string;
}

interface ToolPresentation {
  icon: string;
  inputDetail?: ToolDetail;
  resultDetail?: ToolDetail;
  rawOutputPolicy: RawOutputPolicy;
}

export interface ToolMetadata {
  raw_name: string;
  canonical_name: string;
  display_name: string;
  category: string;
  summary?: string;
  status?: string;
  ids?: Record<string, string>;
  mcp?: McpToolMetadata;
  result_kind?: string;
  structured?: unknown;
  presentation?: ToolPresentation;
}

export interface Message {
  role: MessageRole;
  message_kind?: MessageKind;
  content: string;
  timestamp: string | null;
  tool_name: string | null;
  tool_input: string | null;
  tool_metadata?: ToolMetadata;
  token_usage: TokenUsage | null;
  model?: string;
}

export interface SessionDetail {
  meta: SessionMeta;
  messages: Message[];
  /** Count of per-line parse warnings surfaced while loading this session
   *  (malformed JSONL lines the provider parser had to skip). Omitted when
   *  zero; triggers the ⚠ badge in SessionToolbar when > 0. */
  parse_warning_count?: number;
}

type TreeNodeType = "provider" | "project" | "session";

export interface TreeNode {
  id: string;
  label: string;
  node_type: TreeNodeType;
  children: TreeNode[];
  count: number;
  provider: Provider | null;
  updated_at?: number;
  is_sidechain?: boolean;
  project_path?: string;
}

export interface SearchResult {
  session: SessionMeta;
  snippet: string;
}

export interface SearchFilters {
  query: string;
  provider?: string;
  project?: string;
  after?: number;
  before?: number;
}

export interface IndexStats {
  session_count: number;
  db_size_bytes: number;
  last_index_time: string;
  usage_last_refreshed_at: string;
}

export interface PricingCatalogStatus {
  updated_at: string | null;
  model_count: number;
}

export type MaintenanceJob = "rebuild_index" | "refresh_usage";
type MaintenancePhase = "started" | "finished" | "failed";

export interface MaintenanceEvent {
  job: MaintenanceJob;
  phase: MaintenancePhase;
  message?: string;
}

export interface ProviderSnapshot {
  key: Provider;
  label: string;
  color: string;
  sort_order: number;
  path: string;
  exists: boolean;
  session_count: number;
}

export interface TrashMeta {
  id: string;
  provider: string;
  title: string;
  original_path: string;
  trashed_at: number;
  trash_file: string;
  project_name: string;
  variant_name?: string;
}

export interface BatchResult {
  succeeded: number;
  failed: number;
  errors: string[];
}

export interface PrevPeriodTotals {
  total_sessions: number;
  total_turns: number;
  total_tokens: number;
  total_cost: number;
}

export interface UsageStats {
  total_sessions: number;
  total_turns: number;
  total_input_tokens: number;
  total_output_tokens: number;
  total_cache_read_tokens: number;
  total_cache_write_tokens: number;
  total_cost: number;
  cache_hit_rate: number;
  daily_usage: DailyUsage[];
  model_costs: ModelCost[];
  project_costs: ProjectCost[];
  recent_sessions: SessionCostRow[];
  provider_session_counts: { provider: string; count: number }[];
  prev_period?: PrevPeriodTotals;
}

export interface DailyUsage {
  date: string;
  provider: string;
  tokens: number;
  cost: number;
}

export interface ActivityDay {
  date: string;
  sessions: number;
  turns: number;
  tokens: number;
  cost: number;
}

export interface ActivityCalendar {
  /** Days with activity, ascending; gaps are filled client-side. */
  days: ActivityDay[];
  /** Years with data (descending) for the year selector. */
  available_years: number[];
}

export interface ModelCost {
  model: string;
  turns: number;
  input_tokens: number;
  output_tokens: number;
  cache_tokens: number;
  cost: number;
}

interface ProjectProviderUsage {
  provider: string;
  sessions: number;
  turns: number;
  tokens: number;
  cost: number;
}

export interface ProjectCost {
  project: string;
  project_path: string;
  /** Every provider that contributed usage to this project (sorted). */
  providers: string[];
  /** Per-provider breakdown (sorted by cost desc) for the expandable row. */
  by_provider: ProjectProviderUsage[];
  sessions: number;
  turns: number;
  tokens: number;
  cost: number;
}

export interface SessionCostRow {
  id: string;
  project: string;
  project_path: string;
  provider: string;
  model: string;
  updated_at: number;
  turns: number;
  tokens: number;
  cost: number;
}
