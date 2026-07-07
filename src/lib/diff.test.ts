import { describe, expect, it } from "vitest";

import { buildToolLineDiff, inlineSegments, pairChangedLines } from "@/lib/diff";
import type { ToolDiffLine, ToolDiffLineType } from "@/lib/types";

describe("buildToolLineDiff", () => {
  it("renders unchanged lines as context and changed lines as remove/add", () => {
    expect(buildToolLineDiff("a\nold\nc\n", "a\nnew\nc\n")).toEqual([
      { type: "context", oldLine: 1, newLine: 1, text: "a" },
      { type: "remove", oldLine: 2, newLine: null, text: "old" },
      { type: "add", oldLine: null, newLine: 2, text: "new" },
      { type: "context", oldLine: 3, newLine: 3, text: "c" },
    ]);
  });

  it("tracks inserted and deleted line numbers", () => {
    expect(buildToolLineDiff("a\nc\n", "a\nb\nc\n")).toEqual([
      { type: "context", oldLine: 1, newLine: 1, text: "a" },
      { type: "add", oldLine: null, newLine: 2, text: "b" },
      { type: "context", oldLine: 2, newLine: 3, text: "c" },
    ]);

    expect(buildToolLineDiff("a\nb\nc\n", "a\nc\n")).toEqual([
      { type: "context", oldLine: 1, newLine: 1, text: "a" },
      { type: "remove", oldLine: 2, newLine: null, text: "b" },
      { type: "context", oldLine: 3, newLine: 2, text: "c" },
    ]);
  });

  it("keeps very large diffs complete", () => {
    const oldText = Array.from({ length: 220 }, (_, i) => `old ${i}`).join(
      "\n",
    );
    const newText = Array.from({ length: 220 }, (_, i) => `new ${i}`).join(
      "\n",
    );
    const lines = buildToolLineDiff(oldText, newText);

    expect(lines.length).toBe(440);
    expect(lines.some((line) => line.type === "skip")).toBe(false);
  });
});

describe("inlineSegments", () => {
  it("highlights the differing middle, keeps prefix/suffix quiet", () => {
    const { from, to } = inlineSegments(
      'const x = "old-value";',
      'const x = "new-value";',
    );
    expect(from).toEqual([
      { text: 'const x = "', changed: false },
      { text: "old", changed: true },
      { text: '-value";', changed: false },
    ]);
    expect(to[1]).toEqual({ text: "new", changed: true });
  });

  it("degrades to full-changed for disjoint rewrites", () => {
    const { from, to } = inlineSegments("alpha", "omega");
    expect(from.some((s) => s.changed)).toBe(true);
    expect(to.map((s) => s.text).join("")).toBe("omega");
  });

  it("handles pure insertion", () => {
    const { to } = inlineSegments("ab", "aXb");
    expect(to).toEqual([
      { text: "a", changed: false },
      { text: "X", changed: true },
      { text: "b", changed: false },
    ]);
  });
});

describe("pairChangedLines", () => {
  it("pairs equal-position remove/add runs", () => {
    const mk = (type: ToolDiffLineType, text: string): ToolDiffLine => ({
      type,
      oldLine: null,
      newLine: null,
      text,
    });
    const lines = [
      mk("context", "a"),
      mk("remove", "old1"),
      mk("remove", "old2"),
      mk("add", "new1"),
      mk("context", "z"),
    ];
    const pairs = pairChangedLines(lines);
    expect(pairs.get(1)).toBe(3);
    expect(pairs.get(3)).toBe(1);
    expect(pairs.has(2)).toBe(false);
  });
});
