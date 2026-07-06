import { describe, it, expect } from "vitest";
import { collectSessionIds, collectSessionNodes } from "@/lib/tree-utils";
import type { TreeNode } from "@/lib/types";

function makeSessionNode(id: string): TreeNode {
  return {
    id,
    label: `Session ${id}`,
    node_type: "session",
    children: [],
    count: 0,
    provider: "claude",
  };
}

function makeGroupNode(
  id: string,
  nodeType: "provider" | "project",
  children: TreeNode[],
): TreeNode {
  return {
    id,
    label: id,
    node_type: nodeType,
    children,
    count: children.length,
    provider: null,
  };
}

describe("collectSessionIds", () => {
  it("returns [id] for a session node", () => {
    const node = makeSessionNode("s1");
    expect(collectSessionIds(node)).toEqual(["s1"]);
  });

  it("returns all ids from a nested tree", () => {
    const tree = makeGroupNode("provider", "provider", [
      makeGroupNode("project-a", "project", [
        makeSessionNode("s1"),
        makeSessionNode("s2"),
      ]),
      makeGroupNode("project-b", "project", [makeSessionNode("s3")]),
    ]);
    expect(collectSessionIds(tree)).toEqual(["s1", "s2", "s3"]);
  });

  it("returns [] for an empty group", () => {
    const tree = makeGroupNode("empty", "provider", []);
    expect(collectSessionIds(tree)).toEqual([]);
  });
});

describe("collectSessionNodes", () => {
  it("returns [node] for a session node", () => {
    const node = makeSessionNode("s1");
    const result = collectSessionNodes(node);
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe("s1");
  });

  it("returns all session nodes from a nested tree", () => {
    const tree = makeGroupNode("provider", "provider", [
      makeGroupNode("project-a", "project", [
        makeSessionNode("s1"),
        makeSessionNode("s2"),
      ]),
    ]);
    const result = collectSessionNodes(tree);
    expect(result).toHaveLength(2);
    expect(result.map((n) => n.id)).toEqual(["s1", "s2"]);
  });
});
