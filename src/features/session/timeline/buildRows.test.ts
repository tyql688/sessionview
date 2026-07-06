import { describe, expect, it } from "vitest";
import type { Message } from "../../../lib/types";
import { buildRows } from "./buildRows";
import type { TimelineItem } from "./types";
import { rowKey } from "./types";

const toolMessage: Message = {
  role: "tool",
  content: "out",
  timestamp: null,
  tool_name: "Bash",
  tool_input: null,
  token_usage: null,
};

function user(index: number, ts: number | null = null): TimelineItem {
  return { kind: "user", index, markdown: "q", images: [], ts, command: null };
}
function assistant(index: number, ts: number | null = null): TimelineItem {
  return {
    kind: "assistantText",
    index,
    markdown: "a",
    images: [],
    ts,
    usage: null,
    model: null,
    command: null,
  };
}
function thinking(index: number, ts: number | null = null): TimelineItem {
  return { kind: "thinking", index, text: "hmm", ts };
}
function tool(index: number, ts: number | null = null): TimelineItem {
  return { kind: "toolStep", index, message: toolMessage, ts };
}
function marker(index: number): TimelineItem {
  return {
    kind: "systemMarker",
    index,
    subtype: "turn_duration",
    detail: "3s",
    ts: null,
  };
}

describe("buildRows", () => {
  it("groups thinking and tool steps between user and assistant", () => {
    const rows = buildRows(
      [user(0, 100), thinking(1, 110), tool(2, 120), assistant(3, 130)],
      { live: false },
    );
    expect(rows.map((r) => r.kind)).toEqual(["user", "activity", "assistant"]);
    const activity = rows[1];
    if (activity?.kind !== "activity") throw new Error("expected activity");
    expect(activity.items).toHaveLength(2);
    expect(activity.startTs).toBe(100);
    expect(activity.endTs).toBe(120);
  });

  it("keeps a pure tool turn as an activity with no assistant row", () => {
    const rows = buildRows([user(0), tool(1), tool(2)], { live: false });
    expect(rows.map((r) => r.kind)).toEqual(["user", "activity"]);
  });

  it("system markers and unknown items break the running group", () => {
    const weird: TimelineItem = {
      kind: "unknown",
      index: 3,
      raw: toolMessage,
    };
    const rows = buildRows([user(0), tool(1), marker(2), weird, tool(4)], {
      live: false,
    });
    expect(rows.map((r) => r.kind)).toEqual([
      "user",
      "activity",
      "marker",
      "unknown",
      "activity",
    ]);
  });

  it("leaves timestamps null instead of inventing a duration", () => {
    const rows = buildRows([user(0), tool(1)], { live: false });
    const activity = rows[1];
    if (activity?.kind !== "activity") throw new Error("expected activity");
    expect(activity.startTs).toBeNull();
    expect(activity.endTs).toBeNull();
  });

  it("marks only the trailing activity as running under live watch", () => {
    const rows = buildRows([user(0), tool(1), assistant(2), user(3), tool(4)], {
      live: true,
    });
    const groups = rows.filter((r) => r.kind === "activity");
    expect(groups.map((g) => g.kind === "activity" && g.running)).toEqual([
      false,
      true,
    ]);
  });

  it("derives stable row keys from absolute indices", () => {
    const rows = buildRows([user(7), tool(8)], { live: false });
    expect(rows.map(rowKey)).toEqual(["user-7", "activity-8"]);
  });
});
