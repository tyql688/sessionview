import { describe, expect, it } from "vitest";
import type { Message } from "../../lib/types";
import type { ProcessedEntry } from "./hooks";
import {
  countMatchingEntries,
  findNewestMatchingEntryIndex,
  searchWindowBounds,
} from "./search-utils";

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

describe("session search utilities", () => {
  it("matches short Chinese terms in messages and tool groups", () => {
    const entries: ProcessedEntry[] = [
      {
        key: "m1",
        type: "message",
        msg: message("英文内容"),
        searchHaystack: "英文内容".toLocaleLowerCase(),
      },
      {
        key: "tools",
        type: "merged-tools",
        tools: ["Bash"],
        messages: [
          {
            role: "tool",
            content: "工具输出里有中文搜索",
            timestamp: null,
            tool_name: "Bash",
            tool_input: null,
            token_usage: null,
          },
        ],
        searchHaystack: "Bash\n工具输出里有中文搜索".toLocaleLowerCase(),
      },
      {
        key: "m2",
        type: "message",
        msg: message("最新中文命中"),
        searchHaystack: "最新中文命中".toLocaleLowerCase(),
      },
    ];

    expect(countMatchingEntries(entries, "中文")).toBe(2);
    expect(findNewestMatchingEntryIndex(entries, "中文")).toBe(2);
  });

  it("builds a bounded render window around the nearest match", () => {
    expect(searchWindowBounds(1000, 950)).toEqual({ start: 860, end: 1000 });
    expect(searchWindowBounds(1000, 100)).toEqual({ start: 80, end: 220 });
  });
});
