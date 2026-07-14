import type { Message, ToolMetadata } from "@/lib/types";

/** Providers where subagents are stored as separate session files (can be opened). */
export const SUBAGENT_FILE_PROVIDERS = new Set([
  "claude",
  "codex",
  "kimi",
  "cursor",
  "cc-mirror",
  "antigravity",
  "grok",
]);

/**
 * Subagent metadata extracted from an Agent tool message. Each field is
 * optional/empty when the underlying source doesn't carry it; the "Open"
 * button(s) in ToolMessage resolve a child session from whichever of these
 * are present (priority: agentId → nickname → description).
 */
export interface SubagentInfo {
  /** Codex agent nickname from tool output ({"nickname":"Faraday"}). */
  nickname?: string;
  /** Full (untruncated) task description from tool input JSON. */
  description?: string;
  /** Resolved agent id (Kimi output line / structured metadata / tool input). */
  agentId?: string;
  /** Multi-spawn child ids (one "Open" per entry). */
  childIds?: string[];
  /** Positional prompts aligned with childIds (empty string when absent). */
  childPrompts: string[];
}

export interface SubagentMatchRequest {
  description?: string;
  nickname?: string;
  agentId?: string;
}

export interface SubagentMatchCandidate {
  id: string;
  title: string;
}

export function isAgentToolMessage(message: Pick<Message, "tool_name" | "tool_metadata">): boolean {
  return message.tool_name === "Agent" || message.tool_metadata?.canonical_name === "Agent";
}

/** Narrow `structured` metadata to a plain object record (not array/null). */
function structuredRecord(metadata: ToolMetadata | undefined): Record<string, unknown> | null {
  const structured = metadata?.structured;
  return structured && typeof structured === "object" && !Array.isArray(structured)
    ? (structured as Record<string, unknown>)
    : null;
}

/** Parse a possibly-JSON tool payload into an object record. Tool payloads are
 *  rendered on a hot path and many real sessions contain plain text, truncated
 *  JSON, or partial streamed values, so parse misses fall back quietly. */
export function parseToolJsonObject(raw: string | undefined | null): Record<string, unknown> | undefined {
  if (typeof raw !== "string") return undefined;
  const trimmed = raw.trimStart();
  if (!trimmed.startsWith("{")) return undefined;
  try {
    const parsed: unknown = JSON.parse(trimmed);
    return typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : undefined;
  } catch {
    return undefined;
  }
}

/** Extract nickname from Agent tool output (Codex: {"nickname":"Faraday"}). */
export function extractAgentNickname(output: Record<string, unknown> | undefined): string | undefined {
  if (typeof output?.nickname === "string") return output.nickname;
  for (const key of ["name", "teammate_id"]) {
    const value = output?.[key];
    if (typeof value === "string" && value.length > 0) {
      return stripAgentSessionSuffix(value);
    }
  }
  return undefined;
}

/** Full description from Agent tool input (not truncated, for subagent matching).
 *  Codex spawn_agent carries the task text in `message`, not `description`/`prompt`. */
export function extractAgentDescription(input: Record<string, unknown> | undefined): string | undefined {
  if (!input) return undefined;
  const candidate = input.description ?? input.prompt ?? input.message;
  return typeof candidate === "string" ? candidate : undefined;
}

/** Extract agent_id from Agent tool output text / structured metadata / input.
 *  Priority:
 *    1. Kimi output format: "agent_id: xxx"
 *    2. Structured metadata agentId (set by successful spawn_agent)
 *    3. Tool input target / agent_id (codex wait_agent / send_input / close_agent) */
export function extractAgentId(
  outputText: string | undefined,
  metadata: ToolMetadata | undefined,
  input: Record<string, unknown> | undefined,
): string | undefined {
  if (typeof outputText === "string" && outputText.length > 0) {
    const m = outputText.match(/^agent_id:\s*(\S+)/m);
    if (m) return m[1];
  }
  const structured = metadata?.structured;
  if (structured && typeof structured === "object" && !Array.isArray(structured) && "agentId" in structured) {
    return String((structured as Record<string, unknown>).agentId);
  }
  if (input) {
    const single = input.target ?? input.agent_id ?? input.agentId;
    if (typeof single === "string") return single;
    const targets = input.targets;
    if (Array.isArray(targets) && targets.length === 1 && typeof targets[0] === "string") {
      return targets[0];
    }
  }
  return undefined;
}

/**
 * Some providers spawn one or many subagents in a single call; the child ids are written to
 * `tool_metadata.structured.childConversationIds`. When this list is present we
 * render one "Open" link per child instead of the single-button path used by
 * single-spawn providers.
 */
export function extractAgentChildIds(metadata: ToolMetadata | undefined): string[] | undefined {
  const structured = structuredRecord(metadata);
  if (!structured) return undefined;
  const raw = structured.childConversationIds;
  if (!Array.isArray(raw)) return undefined;
  const ids = raw.filter((v): v is string => typeof v === "string" && v.length > 0);
  return ids.length > 0 ? ids : undefined;
}

/**
 * Positional list of subagent prompts (one per `extractAgentChildIds` entry).
 * The parser pulls these from the parent's `invoke_subagent` tool input so each
 * "Open" button can display *what* the subagent was asked to do instead of an
 * opaque "Open #2".
 */
export function extractAgentChildPrompts(metadata: ToolMetadata | undefined): string[] {
  const structured = structuredRecord(metadata);
  if (!structured) return [];
  const raw = structured.childPrompts;
  if (!Array.isArray(raw)) return [];
  return raw.map((v) => (typeof v === "string" ? v : ""));
}

/**
 * Extract all subagent metadata for an Agent tool message. Non-Agent messages
 * yield an empty info (`childPrompts: []`, all others undefined). Pure: takes
 * the message and returns derived data with no side effects.
 */
export function extractSubagentInfo(message: Message): SubagentInfo {
  if (!isAgentToolMessage(message)) {
    return { childPrompts: [] };
  }
  const input = parseToolJsonObject(message.tool_input);
  const output = parseToolJsonObject(message.content);
  return {
    nickname: extractAgentNickname(output),
    description: extractAgentDescription(input),
    agentId: extractAgentId(message.content, message.tool_metadata, input),
    childIds: extractAgentChildIds(message.tool_metadata),
    childPrompts: extractAgentChildPrompts(message.tool_metadata),
  };
}

function stripTitleTruncation(value: string): string {
  return value.trim().replace(/(\.\.\.|…)$/, "");
}

function stripAgentSessionSuffix(value: string): string {
  const marker = "@session-";
  const index = value.indexOf(marker);
  return index > 0 ? value.slice(0, index) : value;
}

function agentIdAliases(agentId: string): string[] {
  const trimmed = agentId.trim();
  const withoutSession = stripAgentSessionSuffix(trimmed);
  const withoutAgentPrefix = withoutSession.startsWith("agent-")
    ? withoutSession.slice("agent-".length)
    : withoutSession;
  return [...new Set([trimmed, withoutSession, withoutAgentPrefix])].filter((value) => value.length > 0);
}

export function matchesSubagentSession(
  candidate: SubagentMatchCandidate,
  parentId: string,
  request: SubagentMatchRequest,
): boolean {
  const agentId = request.agentId?.trim();
  if (agentId) {
    for (const alias of agentIdAliases(agentId)) {
      if (
        candidate.id === alias ||
        candidate.id === `agent-${alias}` ||
        candidate.id === `${parentId}:${alias}` ||
        candidate.id === `${parentId}:agent-${alias}` ||
        candidate.title === alias
      ) {
        return true;
      }
    }
  }

  const nickname = request.nickname?.trim();
  if (nickname && candidate.title === nickname) {
    return true;
  }

  const description = request.description?.trim();
  if (!description) return false;
  const title = candidate.title.trim();
  if (title === description || title.startsWith(description)) {
    return true;
  }

  const titlePrefix = stripTitleTruncation(title);
  return titlePrefix.length > 0 && description.startsWith(titlePrefix);
}
