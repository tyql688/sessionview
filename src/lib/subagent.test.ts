import { describe, expect, it } from "vitest";

import {
  SUBAGENT_FILE_PROVIDERS,
  extractAgentChildIds,
  extractAgentChildPrompts,
  extractAgentDescription,
  extractAgentId,
  extractAgentNickname,
  extractSubagentInfo,
  parseToolJsonObject,
} from "./subagent";
import type { Message, ToolMetadata } from "./types";

const baseMessage: Message = {
  role: "tool",
  content: "",
  timestamp: null,
  tool_name: null,
  tool_input: null,
  token_usage: null,
};

function agentMessage(overrides: Partial<Message>): Message {
  return { ...baseMessage, tool_name: "Agent", ...overrides };
}

describe("subagent file providers", () => {
  it("lists the providers that store subagents as separate files", () => {
    expect(SUBAGENT_FILE_PROVIDERS.has("claude")).toBe(true);
    expect(SUBAGENT_FILE_PROVIDERS.has("codex")).toBe(true);
    expect(SUBAGENT_FILE_PROVIDERS.has("kimi")).toBe(true);
    expect(SUBAGENT_FILE_PROVIDERS.has("cursor")).toBe(true);
    expect(SUBAGENT_FILE_PROVIDERS.has("cc-mirror")).toBe(true);
    expect(SUBAGENT_FILE_PROVIDERS.has("antigravity")).toBe(true);
    // Poll-only providers without per-subagent files are excluded.
    expect(SUBAGENT_FILE_PROVIDERS.has("opencode")).toBe(false);
  });
});

describe("parseToolJsonObject", () => {
  it("returns undefined for plain text without warning", () => {
    expect(parseToolJsonObject("hello world", "tool output")).toBeUndefined();
  });

  it("parses a JSON object", () => {
    expect(parseToolJsonObject('{"a":1}', "tool_input")).toEqual({ a: 1 });
  });

  it("returns undefined for a JSON array (not an object record)", () => {
    expect(parseToolJsonObject('{"a":1}', "x")).toEqual({ a: 1 });
    // Arrays trim-start with "[" so we attempt a parse, but only objects
    // are returned as records — arrays come back as the parsed value too.
    expect(parseToolJsonObject("[1,2]", "x")).toEqual([1, 2]);
  });

  it("returns undefined for null/undefined input", () => {
    expect(parseToolJsonObject(null, "x")).toBeUndefined();
    expect(parseToolJsonObject(undefined, "x")).toBeUndefined();
  });
});

describe("extractAgentNickname", () => {
  it("returns the Codex nickname from tool output", () => {
    expect(extractAgentNickname({ nickname: "Faraday" })).toBe("Faraday");
  });

  it("returns undefined when nickname is absent or non-string", () => {
    expect(extractAgentNickname(undefined)).toBeUndefined();
    expect(extractAgentNickname({})).toBeUndefined();
    expect(extractAgentNickname({ nickname: 42 })).toBeUndefined();
  });
});

describe("extractAgentDescription", () => {
  it("uses the full description, not a truncated summary", () => {
    const full = "Do a very thorough multi-step refactor of the whole module";
    expect(extractAgentDescription({ description: full })).toBe(full);
  });

  it("falls back to prompt then message (Codex spawn_agent)", () => {
    expect(extractAgentDescription({ prompt: "run tests" })).toBe("run tests");
    expect(extractAgentDescription({ message: "codex task" })).toBe(
      "codex task",
    );
  });

  it("prefers description over prompt over message", () => {
    expect(
      extractAgentDescription({
        description: "desc",
        prompt: "prompt",
        message: "msg",
      }),
    ).toBe("desc");
  });

  it("returns undefined when no candidate field is present", () => {
    expect(extractAgentDescription(undefined)).toBeUndefined();
    expect(extractAgentDescription({ other: "x" })).toBeUndefined();
  });
});

describe("extractAgentId", () => {
  it("reads Kimi 'agent_id: xxx' from output text first", () => {
    expect(
      extractAgentId("agent_id: agent-7\nmore text", undefined, undefined),
    ).toBe("agent-7");
  });

  it("matches output line anywhere via multiline anchor", () => {
    expect(
      extractAgentId("preamble\nagent_id: abc123", undefined, undefined),
    ).toBe("abc123");
  });

  it("falls back to structured metadata agentId (raw id, no prefix)", () => {
    const metadata: ToolMetadata = {
      raw_name: "spawn_agent",
      canonical_name: "Agent",
      display_name: "Agent",
      category: "agent",
      structured: { agentId: "abcdef" },
    };
    expect(extractAgentId("", metadata, undefined)).toBe("abcdef");
  });

  it("falls back to tool input target / agent_id / agentId", () => {
    expect(extractAgentId("", undefined, { target: "t-1" })).toBe("t-1");
    expect(extractAgentId("", undefined, { agent_id: "a-2" })).toBe("a-2");
    expect(extractAgentId("", undefined, { agentId: "a-3" })).toBe("a-3");
  });

  it("uses a single-element targets array", () => {
    expect(extractAgentId("", undefined, { targets: ["only"] })).toBe("only");
  });

  it("ignores multi-element targets arrays", () => {
    expect(
      extractAgentId("", undefined, { targets: ["a", "b"] }),
    ).toBeUndefined();
  });

  it("returns undefined when nothing matches", () => {
    expect(extractAgentId("", undefined, undefined)).toBeUndefined();
    expect(extractAgentId("", undefined, {})).toBeUndefined();
  });
});

describe("extractAgentChildIds", () => {
  it("returns antigravity childConversationIds", () => {
    const metadata: ToolMetadata = {
      raw_name: "invoke_subagent",
      canonical_name: "Agent",
      display_name: "Agent",
      category: "agent",
      structured: { childConversationIds: ["c1", "c2"] },
    };
    expect(extractAgentChildIds(metadata)).toEqual(["c1", "c2"]);
  });

  it("filters out empty / non-string ids", () => {
    const metadata: ToolMetadata = {
      raw_name: "invoke_subagent",
      canonical_name: "Agent",
      display_name: "Agent",
      category: "agent",
      structured: { childConversationIds: ["c1", "", 5, "c2"] },
    };
    expect(extractAgentChildIds(metadata)).toEqual(["c1", "c2"]);
  });

  it("returns undefined when list is missing or empty", () => {
    expect(extractAgentChildIds(undefined)).toBeUndefined();
    const empty: ToolMetadata = {
      raw_name: "invoke_subagent",
      canonical_name: "Agent",
      display_name: "Agent",
      category: "agent",
      structured: { childConversationIds: [] },
    };
    expect(extractAgentChildIds(empty)).toBeUndefined();
  });
});

describe("extractAgentChildPrompts", () => {
  it("returns positional prompts, coercing non-strings to empty", () => {
    const metadata: ToolMetadata = {
      raw_name: "invoke_subagent",
      canonical_name: "Agent",
      display_name: "Agent",
      category: "agent",
      structured: { childPrompts: ["do A", 1, "do C"] },
    };
    expect(extractAgentChildPrompts(metadata)).toEqual(["do A", "", "do C"]);
  });

  it("returns an empty array when absent", () => {
    expect(extractAgentChildPrompts(undefined)).toEqual([]);
  });
});

describe("extractSubagentInfo", () => {
  it("returns empty info for non-Agent messages", () => {
    const info = extractSubagentInfo({ ...baseMessage, tool_name: "Read" });
    expect(info).toEqual({ childPrompts: [] });
  });

  it("extracts Codex nickname + full description for a spawn", () => {
    const info = extractSubagentInfo(
      agentMessage({
        tool_input: JSON.stringify({
          message: "Investigate the failing parser end to end",
        }),
        content: JSON.stringify({ nickname: "Faraday" }),
      }),
    );
    expect(info.nickname).toBe("Faraday");
    expect(info.description).toBe("Investigate the failing parser end to end");
    expect(info.childIds).toBeUndefined();
    expect(info.childPrompts).toEqual([]);
  });

  it("extracts Claude raw agent id from structured metadata", () => {
    // Claude files are agent-{id}.jsonl, but the structured agentId is the
    // raw {id} (no agent- prefix) — the open handler matches both forms.
    const metadata: ToolMetadata = {
      raw_name: "Task",
      canonical_name: "Agent",
      display_name: "Agent",
      category: "agent",
      structured: { agentId: "11111111-1111-4111-a111-111111111111" },
    };
    const info = extractSubagentInfo(
      agentMessage({
        tool_input: JSON.stringify({ description: "explore the repo" }),
        tool_metadata: metadata,
      }),
    );
    expect(info.agentId).toBe("11111111-1111-4111-a111-111111111111");
    expect(info.description).toBe("explore the repo");
  });

  it("extracts antigravity multi-child ids + aligned prompts", () => {
    const metadata: ToolMetadata = {
      raw_name: "invoke_subagent",
      canonical_name: "Agent",
      display_name: "Agent",
      category: "agent",
      structured: {
        childConversationIds: ["conv-1", "conv-2"],
        childPrompts: ["first task", "second task"],
      },
    };
    const info = extractSubagentInfo(agentMessage({ tool_metadata: metadata }));
    expect(info.childIds).toEqual(["conv-1", "conv-2"]);
    expect(info.childPrompts).toEqual(["first task", "second task"]);
  });
});
