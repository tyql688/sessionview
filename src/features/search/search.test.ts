import { describe, expect, it } from "vitest";

import { parseSearchQuery } from "@/features/search/search";

describe("parseSearchQuery", () => {
  it("extracts provider, project, and free text", () => {
    expect(
      parseSearchQuery("provider:claude project:web fix flaky test"),
    ).toEqual({
      query: "fix flaky test",
      provider: "claude",
      project: "web",
      after: undefined,
      before: undefined,
    });
  });

  it("parses date filters and ignores invalid dates", () => {
    expect(
      parseSearchQuery("after:2025-01-01 before:not-a-date search me"),
    ).toEqual({
      query: "search me",
      provider: undefined,
      project: undefined,
      after: 1735689600,
      before: undefined,
    });
  });
});
