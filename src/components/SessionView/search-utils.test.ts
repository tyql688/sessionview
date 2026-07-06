import { describe, expect, it } from "vitest";
import type { Message } from "../../lib/types";
import type { ProcessedEntry } from "./hooks";
import { findFirstMatchingEntryIndex } from "./search-utils";

function message(content: string): Message {
  return {
    role: "assistant",
    content,
    timestamp: null,
    tool_name: null,
    tool_input: null,
    token_usage: null,
  };
}

function entry(index: number, content: string): ProcessedEntry {
  return {
    key: `msg-${index}`,
    type: "message",
    msg: message(content),
    messageIndex: index,
    searchHaystack: content.toLocaleLowerCase(),
  };
}

describe("session search utilities", () => {
  it("finds the first matching entry across searchable messages", () => {
    const entries = [
      entry(0, "英文内容"),
      entry(1, "第一条中文命中"),
      entry(2, "最新中文命中"),
    ];
    expect(findFirstMatchingEntryIndex(entries, "中文")).toBe(1);
  });

  it("returns -1 for a blank query", () => {
    expect(findFirstMatchingEntryIndex([entry(0, "anything")], "  ")).toBe(-1);
  });
});
