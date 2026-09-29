import { describe, expect, it } from "vitest";
import type { MessageRole } from "@/lib/types";
import { buildMatchLocations, type SearchableMessage } from "@/features/session/search-utils";

function searchable(messageIndex: number, role: MessageRole, content: string): SearchableMessage {
  return { messageIndex, role, haystack: content.toLocaleLowerCase() };
}

describe("session search utilities", () => {
  it("locates matches by absolute message index across the whole session", () => {
    const messages = [
      searchable(0, "user", "英文内容"),
      searchable(40, "assistant", "第一条中文命中"),
      searchable(912, "user", "最新中文命中"),
    ];
    expect(buildMatchLocations(messages, "中文", new Set())).toEqual([40, 912]);
  });

  it("skips roles hidden in the filter toolbar", () => {
    const messages = [searchable(3, "user", "needle"), searchable(4, "assistant", "Needle")];
    expect(buildMatchLocations(messages, "needle", new Set<MessageRole>(["user"]))).toEqual([4]);
  });
});
