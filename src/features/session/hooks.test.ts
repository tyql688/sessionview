import { describe, expect, it } from "vitest";

import type { Message } from "@/lib/types";
import { isRenderableMessage, processMessages } from "@/features/session/hooks";

const baseMessage: Message = {
  role: "assistant",
  content: "",
  timestamp: null,
  tool_name: null,
  tool_input: null,
  token_usage: null,
};

describe("SessionView message processing", () => {
  it("hides usage-only assistant placeholders from the rendered timeline", () => {
    const usageOnly: Message = {
      ...baseMessage,
      timestamp: "2026-04-11T02:25:16.628Z",
      token_usage: {
        input_tokens: 1,
        output_tokens: 2,
        cache_creation_input_tokens: 3,
        cache_read_input_tokens: 4,
      },
    };
    const visibleAssistant: Message = {
      ...baseMessage,
      content: "Visible reply",
      timestamp: "2026-04-11T02:25:17.000Z",
    };

    expect(isRenderableMessage(usageOnly)).toBe(false);
    expect(processMessages([usageOnly, visibleAssistant], 0)).toEqual([
      {
        key: "msg-1-assistant-2026-04-11T02:25:17.000Z",
        type: "message",
        msg: visibleAssistant,
        messageIndex: 1,
        searchHaystack: "visible reply",
      },
    ]);
  });

  // Regression: entries must carry ABSOLUTE session indices. When the loaded
  // window starts mid-session (tail-first load), window-relative indices
  // desynced data-turn anchors and minimap jumps from the outline's absolute
  // message_index values.
  it("offsets message indices and keys by the window start", () => {
    const first: Message = {
      ...baseMessage,
      content: "old reply",
      timestamp: "2026-04-11T02:25:17.000Z",
    };
    const second: Message = {
      ...baseMessage,
      role: "user",
      content: "next question",
      timestamp: "2026-04-11T02:25:18.000Z",
    };

    const entries = processMessages([first, second], 700);
    expect(
      entries.map((e) => (e.type === "message" ? e.messageIndex : null)),
    ).toEqual([700, 701]);
    expect(entries[0]?.key).toBe("msg-700-assistant-2026-04-11T02:25:17.000Z");
  });

  it("keeps agent tool messages out of collapsed tool groups", () => {
    const readTool: Message = {
      ...baseMessage,
      role: "tool",
      tool_name: "Read",
      tool_input: '{"path":"a.ts"}',
      content: "",
      timestamp: "2026-04-11T02:25:17.000Z",
    };
    const agentTool: Message = {
      ...baseMessage,
      role: "tool",
      tool_name: "Agent",
      tool_input: '{"prompt":"inspect"}',
      content: "",
      timestamp: "2026-04-11T02:25:18.000Z",
    };
    const swarmTool: Message = {
      ...baseMessage,
      role: "tool",
      tool_name: "AgentSwarm",
      tool_input: '{"description":"review swarm"}',
      content: "",
      timestamp: "2026-04-11T02:25:18.500Z",
      tool_metadata: {
        raw_name: "AgentSwarm",
        canonical_name: "Agent",
        display_name: "AgentSwarm",
        category: "agent",
      },
    };
    const bashTool: Message = {
      ...baseMessage,
      role: "tool",
      tool_name: "Bash",
      tool_input: '{"command":"pwd"}',
      content: "",
      timestamp: "2026-04-11T02:25:19.000Z",
    };

    expect(
      processMessages([readTool, agentTool, swarmTool, bashTool], 0),
    ).toEqual([
      {
        key: "msg-0-tool-2026-04-11T02:25:17.000Z",
        type: "message",
        msg: readTool,
        messageIndex: 0,
        searchHaystack: "",
      },
      {
        key: "msg-1-tool-2026-04-11T02:25:18.000Z",
        type: "message",
        msg: agentTool,
        messageIndex: 1,
        searchHaystack: "",
      },
      {
        key: "msg-2-tool-2026-04-11T02:25:18.500Z",
        type: "message",
        msg: swarmTool,
        messageIndex: 2,
        searchHaystack: "",
      },
      {
        key: "msg-3-tool-2026-04-11T02:25:19.000Z",
        type: "message",
        msg: bashTool,
        messageIndex: 3,
        searchHaystack: "",
      },
    ]);
  });
});
