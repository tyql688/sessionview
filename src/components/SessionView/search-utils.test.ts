import { describe, expect, it } from "vitest";
import type { Message } from "../../lib/types";
import type { ProcessedEntry } from "./hooks";
import {
  findFirstMatchingEntryIndex,
  getMarksInVisualOrder,
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
  it("finds the first matching entry across searchable messages", () => {
    const entries: ProcessedEntry[] = [
      {
        key: "m1",
        type: "message",
        msg: message("英文内容"),
        messageIndex: 0,
        searchHaystack: "英文内容".toLocaleLowerCase(),
      },
      {
        key: "m2",
        type: "message",
        msg: message("第一条中文命中"),
        messageIndex: 1,
        searchHaystack: "第一条中文命中".toLocaleLowerCase(),
      },
      {
        key: "m3",
        type: "message",
        msg: message("最新中文命中"),
        messageIndex: 2,
        searchHaystack: "最新中文命中".toLocaleLowerCase(),
      },
    ];

    expect(findFirstMatchingEntryIndex(entries, "中文")).toBe(1);
  });

  it("returns an empty list when the mark container is missing", () => {
    // getMarksInVisualOrder is the single source of truth shared by the counter
    // total and Next/Prev navigation; with no container both must yield zero.
    // DOM-backed counting is covered in SessionSearch.test.tsx (happy-dom).
    expect(getMarksInVisualOrder(undefined)).toEqual([]);
  });
});
