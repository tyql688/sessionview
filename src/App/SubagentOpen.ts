import type { SessionRef } from "../lib/types";
import { matchesSubagentSession } from "../lib/subagent";

export interface OpenSubagentDetail {
  description?: string;
  nickname?: string;
  agentId?: string;
  parentSessionId?: string;
}

export interface OpenSubagentDeps {
  getActiveParentSessionIds: () => string[];
  getChildSessions: (parentId: string) => Promise<SessionRef[]>;
  openSession: (session: SessionRef) => void;
  onLoadFailed: () => void;
  onNotFound: () => void;
  onChildSessionLoadError?: (parentId: string, error: unknown) => void;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function optionalString(
  detail: Record<string, unknown>,
  key: keyof OpenSubagentDetail,
): string | undefined {
  const value = detail[key];
  return typeof value === "string" ? value : undefined;
}

export function openSubagentDetailFromEvent(event: Event): OpenSubagentDetail {
  if (!("detail" in event) || !isRecord(event.detail)) {
    return {};
  }

  return {
    description: optionalString(event.detail, "description"),
    nickname: optionalString(event.detail, "nickname"),
    agentId: optionalString(event.detail, "agentId"),
    parentSessionId: optionalString(event.detail, "parentSessionId"),
  };
}

export function candidateParentSessionIds(
  detail: OpenSubagentDetail,
  activeParentSessionIds: string[],
): string[] {
  if (detail.parentSessionId) return [detail.parentSessionId];
  return activeParentSessionIds.filter((id) => id.length > 0);
}

export async function openSubagent(
  detail: OpenSubagentDetail,
  deps: OpenSubagentDeps,
): Promise<void> {
  const parentIds = detail.parentSessionId
    ? [detail.parentSessionId]
    : candidateParentSessionIds(detail, deps.getActiveParentSessionIds());

  let anyParentResolved = false;
  for (const parentId of parentIds) {
    try {
      const children = await deps.getChildSessions(parentId);
      anyParentResolved = true;
      const match = children.find((candidate) =>
        matchesSubagentSession(candidate, parentId, detail),
      );
      if (match) {
        deps.openSession(match);
        return;
      }
    } catch (error) {
      deps.onChildSessionLoadError?.(parentId, error);
    }
  }

  if (!anyParentResolved && parentIds.length > 0) {
    deps.onLoadFailed();
    return;
  }

  deps.onNotFound();
}

export function createOpenSubagentHandler(
  deps: OpenSubagentDeps,
): (event: Event) => void {
  return (event) => {
    void openSubagent(openSubagentDetailFromEvent(event), deps);
  };
}
