import { parseTimestamp } from "@/lib/formatters";
import type { Message, SessionMeta, ToolMetadata } from "@/lib/types";
import { toolDisplayName } from "@/lib/tools";
import { toolVisualKind } from "@/features/session/ToolGlyph";

export interface ToolDistributionItem {
  key: string;
  label: string;
  count: number;
  share: number;
  category: string;
  canonicalName: string;
}

export interface TokenTimelinePoint {
  index: number;
  timestamp: number | null;
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  total: number;
  cumulative: number;
}

export interface TokenTimelineBucket {
  key: string;
  startIndex: number;
  endIndex: number;
  startTime: number | null;
  endTime: number | null;
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  total: number;
  cumulative: number;
}

export interface RoleCounts {
  user: number;
  assistant: number;
  tool: number;
  system: number;
}

export interface SessionAnalytics {
  roleCounts: RoleCounts;
  toolCalls: number;
  toolTypes: number;
  toolDistribution: ToolDistributionItem[];
  tokenPoints: TokenTimelinePoint[];
  tokenBuckets: TokenTimelineBucket[];
  tokenTotals: {
    input: number;
    output: number;
    cacheRead: number;
    cacheWrite: number;
    total: number;
  };
  firstTimestamp: number | null;
  lastTimestamp: number | null;
  peakTokenPoint: TokenTimelinePoint | null;
  averageTokensPerAssistantTurn: number | null;
  toolsPerUserTurn: number | null;
  cacheShare: number | null;
}

const MAX_TOKEN_BUCKETS = 44;

function messageToolName(message: Message): string | null {
  const name = message.tool_name ?? message.tool_metadata?.canonical_name ?? null;
  if (name !== null && name.trim().length > 0) return name;
  if (message.tool_input !== null && message.tool_input.trim().length > 0) return "Tool";
  return null;
}

function toolKey(name: string, metadata: ToolMetadata | undefined, label: string): string {
  const visual = toolVisualKind(name, metadata);
  return `${visual.category}:${visual.canonicalName}:${metadata?.mcp?.server ?? ""}:${label}`;
}

function buildTokenBuckets(points: TokenTimelinePoint[]): TokenTimelineBucket[] {
  if (points.length === 0) return [];
  const bucketSize = Math.max(1, Math.ceil(points.length / MAX_TOKEN_BUCKETS));
  const buckets: TokenTimelineBucket[] = [];

  for (let i = 0; i < points.length; i += bucketSize) {
    const slice = points.slice(i, i + bucketSize);
    const first = slice[0];
    const last = slice[slice.length - 1];
    if (!first || !last) continue;
    const input = slice.reduce((sum, point) => sum + point.input, 0);
    const output = slice.reduce((sum, point) => sum + point.output, 0);
    const cacheRead = slice.reduce((sum, point) => sum + point.cacheRead, 0);
    const cacheWrite = slice.reduce((sum, point) => sum + point.cacheWrite, 0);
    const total = input + output + cacheRead + cacheWrite;
    buckets.push({
      key: `${first.index}-${last.index}`,
      startIndex: first.index,
      endIndex: last.index,
      startTime: first.timestamp,
      endTime: last.timestamp,
      input,
      output,
      cacheRead,
      cacheWrite,
      total,
      cumulative: last.cumulative,
    });
  }

  return buckets;
}

function metaTokenTotals(meta: SessionMeta | null): SessionAnalytics["tokenTotals"] | null {
  if (meta === null) return null;
  const input = meta.input_tokens;
  const output = meta.output_tokens;
  const cacheRead = meta.cache_read_tokens;
  const cacheWrite = meta.cache_write_tokens;
  const total = input + output + cacheRead + cacheWrite;
  return total > 0 ? { input, output, cacheRead, cacheWrite, total } : null;
}

export function buildSessionAnalytics(messages: Message[], meta: SessionMeta | null = null): SessionAnalytics {
  const roleCounts: RoleCounts = {
    user: 0,
    assistant: 0,
    tool: 0,
    system: 0,
  };
  const tools = new Map<string, ToolDistributionItem>();
  const tokenPoints: TokenTimelinePoint[] = [];
  let firstTimestamp: number | null = null;
  let lastTimestamp: number | null = null;
  let cumulative = 0;
  let assistantTokenTurns = 0;

  for (const [index, message] of messages.entries()) {
    roleCounts[message.role] += 1;
    const timestamp = parseTimestamp(message.timestamp);
    if (timestamp !== null) {
      firstTimestamp = firstTimestamp === null ? timestamp : Math.min(firstTimestamp, timestamp);
      lastTimestamp = lastTimestamp === null ? timestamp : Math.max(lastTimestamp, timestamp);
    }

    const name = message.role === "tool" ? messageToolName(message) : null;
    if (name !== null) {
      const label = toolDisplayName(name, message.tool_metadata);
      const visual = toolVisualKind(name, message.tool_metadata);
      const key = toolKey(name, message.tool_metadata, label);
      const existing = tools.get(key);
      if (existing) {
        existing.count += 1;
      } else {
        tools.set(key, {
          key,
          label,
          count: 1,
          share: 0,
          category: visual.category,
          canonicalName: visual.canonicalName,
        });
      }
    }

    const usage = message.token_usage;
    if (usage !== null) {
      const input = usage.input_tokens;
      const output = usage.output_tokens;
      const cacheRead = usage.cache_read_input_tokens;
      const cacheWrite = usage.cache_creation_input_tokens;
      const total = input + output + cacheRead + cacheWrite;
      cumulative += total;
      if (message.role === "assistant") assistantTokenTurns += 1;
      tokenPoints.push({
        index,
        timestamp,
        input,
        output,
        cacheRead,
        cacheWrite,
        total,
        cumulative,
      });
    }
  }

  const toolCalls = [...tools.values()].reduce((sum, item) => sum + item.count, 0);
  const toolDistribution = [...tools.values()]
    .map((item) => ({
      ...item,
      share: toolCalls > 0 ? item.count / toolCalls : 0,
    }))
    .sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
  const loadedTotals = tokenPoints.reduce(
    (totals, point) => ({
      input: totals.input + point.input,
      output: totals.output + point.output,
      cacheRead: totals.cacheRead + point.cacheRead,
      cacheWrite: totals.cacheWrite + point.cacheWrite,
      total: totals.total + point.total,
    }),
    { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  );
  const tokenTotals = metaTokenTotals(meta) ?? loadedTotals;
  const peakTokenPoint =
    tokenPoints.length > 0
      ? tokenPoints.reduce((peak, point) => (point.total > peak.total ? point : peak), tokenPoints[0])
      : null;
  const inputLikeTokens = tokenTotals.input + tokenTotals.cacheRead + tokenTotals.cacheWrite;

  return {
    roleCounts,
    toolCalls,
    toolTypes: toolDistribution.length,
    toolDistribution,
    tokenPoints,
    tokenBuckets: buildTokenBuckets(tokenPoints),
    tokenTotals,
    firstTimestamp,
    lastTimestamp,
    peakTokenPoint,
    averageTokensPerAssistantTurn: assistantTokenTurns > 0 ? loadedTotals.total / assistantTokenTurns : null,
    toolsPerUserTurn: roleCounts.user > 0 ? toolCalls / roleCounts.user : null,
    cacheShare: inputLikeTokens > 0 ? tokenTotals.cacheRead / inputLikeTokens : null,
  };
}
