import { describe, expect, it } from "vitest";
import { buildSessionAnalytics } from "@/features/session/sessionAnalytics";
import type { Message, SessionMeta } from "@/lib/types";

function message(overrides: Partial<Message>): Message {
  return {
    role: "assistant",
    content: "",
    timestamp: null,
    tool_name: null,
    tool_input: null,
    token_usage: null,
    ...overrides,
  };
}

const meta: SessionMeta = {
  id: "session-1",
  provider: "codex",
  title: "Synthetic",
  project_path: "/tmp/project",
  project_name: "project",
  created_at: 1,
  updated_at: 2,
  message_count: 5,
  file_size_bytes: 100,
  source_path: "/tmp/session.jsonl",
  is_sidechain: false,
  input_tokens: 1000,
  output_tokens: 2000,
  cache_read_tokens: 300,
  cache_write_tokens: 40,
};

describe("buildSessionAnalytics", () => {
  it("groups tools and token timeline points", () => {
    const analytics = buildSessionAnalytics(
      [
        message({ role: "user", content: "hi", timestamp: "2026-01-01T00:00:00Z" }),
        message({
          role: "assistant",
          timestamp: "2026-01-01T00:00:03Z",
          token_usage: {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: 2,
            cache_read_input_tokens: 3,
          },
        }),
        message({
          role: "tool",
          tool_name: "Read",
          tool_input: "{\"file_path\":\"/tmp/a.ts\"}",
          timestamp: "2026-01-01T00:00:04Z",
        }),
        message({
          role: "tool",
          tool_name: "Read",
          tool_input: "{\"file_path\":\"/tmp/b.ts\"}",
          timestamp: "2026-01-01T00:00:05Z",
        }),
        message({
          role: "tool",
          tool_name: "Bash",
          tool_input: "{\"command\":\"npm test\"}",
          timestamp: "2026-01-01T00:00:06Z",
        }),
      ],
      meta,
    );

    expect(analytics.roleCounts).toEqual({
      user: 1,
      assistant: 1,
      tool: 3,
      system: 0,
    });
    expect(analytics.toolCalls).toBe(3);
    expect(analytics.toolTypes).toBe(2);
    expect(analytics.toolDistribution.map((item) => [item.label, item.count])).toEqual([
      ["Read", 2],
      ["Bash", 1],
    ]);
    expect(analytics.tokenTotals.total).toBe(3340);
    expect(analytics.tokenPoints).toHaveLength(1);
    expect(analytics.tokenBuckets[0]?.total).toBe(35);
    expect(analytics.toolsPerUserTurn).toBe(3);
  });
});
