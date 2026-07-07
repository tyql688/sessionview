export const TAB_DRAG_MIME = "application/x-cc-session-tab";
export const TAB_DRAG_FALLBACK_MIME = "text/plain";

export interface TabDragPayload {
  sessionId: string;
  sourceGroupId: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function serializeTabDragPayload(payload: TabDragPayload): string {
  return JSON.stringify(payload);
}

export function parseTabDragPayload(raw: string): TabDragPayload {
  if (raw.trim().length === 0) {
    throw new Error("Missing tab drag payload");
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    throw new Error("Invalid tab drag payload JSON", { cause: error });
  }
  if (!isRecord(parsed)) {
    throw new Error("Tab drag payload must be an object");
  }

  if (typeof parsed.sessionId !== "string" || parsed.sessionId.length === 0) {
    throw new Error("Tab drag payload is missing sessionId");
  }

  if (typeof parsed.sourceGroupId !== "string" || parsed.sourceGroupId.length === 0) {
    throw new Error("Tab drag payload is missing sourceGroupId");
  }

  return {
    sessionId: parsed.sessionId,
    sourceGroupId: parsed.sourceGroupId,
  };
}
