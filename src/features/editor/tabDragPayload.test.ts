import { describe, expect, it } from "vitest";

import { parseTabDragPayload, serializeTabDragPayload } from "@/features/editor/tabDragPayload";

describe("tab drag payload", () => {
  it("round-trips a tab drag payload", () => {
    const serialized = serializeTabDragPayload({
      sessionId: "session-1",
      sourceGroupId: "group-left",
    });

    expect(parseTabDragPayload(serialized)).toEqual({
      sessionId: "session-1",
      sourceGroupId: "group-left",
    });
  });

  it("rejects empty payloads", () => {
    expect(() => parseTabDragPayload("")).toThrow("Missing tab drag payload");
  });

  it("rejects invalid JSON", () => {
    expect(() => parseTabDragPayload("{")).toThrow(
      "Invalid tab drag payload JSON",
    );
  });

  it("rejects non-object JSON", () => {
    expect(() => parseTabDragPayload("[]")).toThrow(
      "Tab drag payload must be an object",
    );
  });

  it("rejects payloads without a session id", () => {
    expect(() =>
      parseTabDragPayload(JSON.stringify({ sourceGroupId: "group-left" })),
    ).toThrow("Tab drag payload is missing sessionId");
  });

  it("rejects payloads without a source group id", () => {
    expect(() =>
      parseTabDragPayload(JSON.stringify({ sessionId: "session-1" })),
    ).toThrow("Tab drag payload is missing sourceGroupId");
  });
});
