import { describe, expect, it } from "vitest";
import type { Message, MessageRole } from "../../../lib/types";
import { extractImages, normalizeMessages } from "./normalize";

const base: Message = {
  role: "user",
  content: "",
  timestamp: null,
  tool_name: null,
  tool_input: null,
  token_usage: null,
};

function msg(overrides: Partial<Message>): Message {
  return { ...base, ...overrides };
}

describe("extractImages", () => {
  it("pulls sourced and bare placeholders out of the markdown", () => {
    const { markdown, images } = extractImages(
      "look at [Image #1: source: /tmp/a.png] and [Image] here",
    );
    expect(markdown).toBe("look at  and  here");
    expect(images).toEqual([{ source: "/tmp/a.png" }, { source: null }]);
  });

  it("leaves image-free content untouched", () => {
    expect(extractImages("plain text")).toEqual({
      markdown: "plain text",
      images: [],
    });
  });
});

describe("normalizeMessages", () => {
  it("emits absolute indices from the window start", () => {
    const { items } = normalizeMessages(
      [msg({ content: "q" }), msg({ role: "assistant", content: "a" })],
      { windowStart: 500 },
    );
    expect(items.map((i) => i.index)).toEqual([500, 501]);
  });

  it("parses thinking and tagged system markers exactly once", () => {
    const { items } = normalizeMessages(
      [
        msg({ role: "system", content: "[thinking]\ndeep thought" }),
        msg({ role: "system", content: "[turn_duration] 12s" }),
        msg({ role: "system", content: "[not_a_known_tag] hello" }),
        msg({ role: "system", content: "free-form note" }),
      ],
      { windowStart: 0 },
    );
    expect(items).toEqual([
      { kind: "thinking", index: 0, text: "deep thought", ts: null },
      {
        kind: "systemMarker",
        index: 1,
        subtype: "turn_duration",
        detail: "12s",
        ts: null,
      },
      {
        kind: "systemMarker",
        index: 2,
        subtype: "plain",
        detail: "[not_a_known_tag] hello",
        ts: null,
      },
      {
        kind: "systemMarker",
        index: 3,
        subtype: "plain",
        detail: "free-form note",
        ts: null,
      },
    ]);
  });

  it("classifies command messages and strips the legacy prefix", () => {
    const { items } = normalizeMessages(
      [
        msg({ content: "/resume", message_kind: "command_input" }),
        msg({ content: "[local_command] ls -la" }),
      ],
      { windowStart: 0 },
    );
    expect(items[0]).toMatchObject({ kind: "user", command: "input" });
    expect(items[1]).toMatchObject({
      kind: "user",
      command: "input",
      markdown: "ls -la",
    });
  });

  it("counts deliberate suppressions instead of dropping silently", () => {
    const { items, skipped } = normalizeMessages(
      [
        // usage-only assistant placeholder
        msg({
          role: "assistant",
          content: "",
          token_usage: {
            input_tokens: 1,
            output_tokens: 2,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
          },
        }),
        // injected template noise
        msg({ content: "<system-reminder>injected</system-reminder>" }),
        // orphaned tool result
        msg({ role: "tool", tool_name: "toulu_01abc", content: "x" }),
        // fully empty tool
        msg({ role: "tool", content: "" }),
      ],
      { windowStart: 0 },
    );
    expect(items).toEqual([]);
    expect(skipped).toEqual({ empty: 2, template: 1, orphanTool: 1 });
  });

  it("keeps real tool messages with their full source message", () => {
    const tool = msg({
      role: "tool",
      tool_name: "Bash",
      tool_input: '{"command":"pwd"}',
      content: "/home",
      timestamp: "2026-04-11T02:25:17.000Z",
    });
    const { items } = normalizeMessages([tool], { windowStart: 3 });
    expect(items).toEqual([
      {
        kind: "toolStep",
        index: 3,
        message: tool,
        ts: Date.parse("2026-04-11T02:25:17.000Z"),
      },
    ]);
  });

  it("maps unrecognized roles to visible unknown items", () => {
    const weird = msg({
      role: "supervisor" as MessageRole,
      content: "??",
    });
    const { items } = normalizeMessages([weird], { windowStart: 0 });
    expect(items).toEqual([{ kind: "unknown", index: 0, raw: weird }]);
  });

  it("propagates missing timestamps as null, never fabricating one", () => {
    const { items } = normalizeMessages(
      [msg({ content: "no time recorded" })],
      { windowStart: 0 },
    );
    expect(items[0]?.ts).toBeNull();
  });
});
