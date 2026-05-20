import { describe, expect, it } from "vitest";

import type { Message } from "../../lib/types";
import { isRenderableMessage, processMessages } from "./hooks";

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
    expect(processMessages([usageOnly, visibleAssistant])).toEqual([
      {
        key: "msg-0-assistant-2026-04-11T02:25:17.000Z",
        type: "message",
        msg: visibleAssistant,
        searchHaystack: "visible reply",
      },
    ]);
  });
});
