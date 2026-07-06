import type { TreeNode } from "@/lib/types";

/** Collect all session-leaf IDs from a tree node recursively. */
export function collectSessionIds(node: TreeNode): string[] {
  if (node.node_type === "session") {
    return [node.id];
  }
  const ids: string[] = [];
  for (const child of node.children) {
    ids.push(...collectSessionIds(child));
  }
  return ids;
}

/** Collect session nodes (with metadata) from a tree node. */
export function collectSessionNodes(node: TreeNode): TreeNode[] {
  if (node.node_type === "session") {
    return [node];
  }
  const nodes: TreeNode[] = [];
  for (const child of node.children) {
    nodes.push(...collectSessionNodes(child));
  }
  return nodes;
}
