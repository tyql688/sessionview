import type { TreeNode, SessionRef, Provider } from "../../lib/types";
import { getBlockedFolders, isPathBlocked } from "../../stores/settings";

/** Filter out projects whose path matches a blocked folder. */
export function filterBlockedFolders(tree: TreeNode[]): TreeNode[] {
  if (getBlockedFolders().length === 0) {
    return tree;
  }

  function prune(nodes: TreeNode[]): TreeNode[] {
    return nodes.flatMap((node) => {
      const path = node.project_path ?? "";
      if (node.node_type === "project" && path && isPathBlocked(path)) {
        return [];
      }

      if (node.node_type === "session") {
        return [node];
      }

      const children = prune(node.children);
      if (children.length === 0) {
        return [];
      }

      return [{ ...node, children }];
    });
  }

  return prune(tree);
}

function countSessions(nodes: TreeNode[]): number {
  let n = 0;
  for (const node of nodes) {
    if (node.node_type === "session") n++;
    else n += countSessions(node.children);
  }
  return n;
}

/** Remove sidechain subagents and update counts. */
export function filterOrphanSubagents(tree: TreeNode[]): TreeNode[] {
  function prune(nodes: TreeNode[]): TreeNode[] {
    return nodes
      .map((node) => {
        const children = prune(node.children);
        // Strip sidechain children from session nodes
        const filtered =
          node.node_type === "session"
            ? children.filter((c) => !c.is_sidechain)
            : children;
        return {
          ...node,
          children: filtered,
          count: node.node_type !== "session" ? countSessions(filtered) : 0,
        };
      })
      .filter((node) => {
        // Remove sidechain sessions at project level
        if (node.node_type === "session" && node.is_sidechain) {
          return false;
        }
        // Remove empty non-session containers
        if (node.node_type !== "session" && node.children.length === 0) {
          return false;
        }
        return true;
      });
  }
  return prune(tree);
}

export function buildSessionRef(
  node: TreeNode,
  parentProjectLabel: string,
): SessionRef {
  return {
    id: node.id,
    provider: (node.provider ?? "claude") as Provider,
    title: node.label,
    project_name: parentProjectLabel,
    is_sidechain: node.is_sidechain ?? false,
  };
}
